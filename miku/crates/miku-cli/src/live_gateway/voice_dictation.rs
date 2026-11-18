use std::sync::atomic::Ordering;
use std::sync::Arc;

use miku_protocol::CaptureTransform;
use serde_json::{json, Value};

use super::api_handler::LiveMikuApiHandler;
use super::llm_multimodal::LlmImageAttachment;

impl LiveMikuApiHandler {
    pub(super) fn set_turn_vision_attachment(&self, att: LlmImageAttachment) {
        let tr = att.transform;
        if let Ok(mut guard) = self.last_vision_attachment.lock() {
            if let Some(mut prev) = guard.take() {
                prev.zeroize();
            }
            *guard = Some(att);
        }
        if let Ok(mut tr_guard) = self.last_capture_transform.lock() {
            *tr_guard = tr;
        }
    }

    pub(super) fn take_pending_multimodal_attachment(&self) -> Option<LlmImageAttachment> {
        self.last_vision_attachment.lock().ok().and_then(|mut g| g.take())
    }

    pub(super) fn clear_turn_vision_state(&self) {
        if let Ok(mut guard) = self.last_vision_attachment.lock() {
            if let Some(mut prev) = guard.take() {
                prev.zeroize();
            }
        }
    }

    pub(super) fn last_capture_transform(&self) -> CaptureTransform {
        self.last_capture_transform
            .lock()
            .map(|g| *g)
            .unwrap_or_default()
    }

    pub(super) fn spawn_native_voice_dictation(&self, engine: &str) {
        self.voice_capture_stop.store(false, Ordering::SeqCst);
        let stop = Arc::clone(&self.voice_capture_stop);
        let events_tx = self.events_tx.clone();
        let req_engine = engine.to_string();
        let groq_key = if req_engine == "groq_whisper" {
            self.state
                .lock()
                .ok()
                .and_then(|mut st| st.resolve_chat_credential("groq").map(|(_, k)| k))
        } else {
            None
        };
        std::thread::spawn(move || {
            let mut samples =
                miku_ui::voice::capture_microphone_pcm_bounded(&stop, 8_000).unwrap_or_default();
            if !samples.is_empty() {
                let mut used_engine = "local_sapi";
                let mut text_out = String::new();
                if req_engine == "groq_whisper" {
                    if let Some(key) = groq_key {
                        let mut wav = miku_ui::voice::encode_riff_wav_pcm16_mono(&samples, 16_000);
                        let cfg = miku_ui::voice::GroqSttConfig::default().with_consent(true);
                        if let Ok(mut body) = miku_ui::voice::build_groq_multipart_body(&wav, &cfg) {
                            let hdrs = vec![
                                ("Authorization".to_string(), format!("Bearer {}", key)),
                                (
                                    "Content-Type".to_string(),
                                    format!(
                                        "multipart/form-data; boundary={}",
                                        miku_ui::voice::GROQ_MULTIPART_BOUNDARY
                                    ),
                                ),
                            ];
                            #[cfg(windows)]
                            if let Ok((200..=299, resp)) = super::winhttp_https::post_https(
                                "api.groq.com",
                                443,
                                "/openai/v1/audio/transcriptions",
                                &hdrs,
                                body.as_slice(),
                            ) {
                                if let Ok(tr) = miku_ui::voice::parse_groq_transcription_json(
                                    &resp,
                                    &cfg,
                                    samples.len(),
                                    16_000,
                                    &[key.as_bytes()],
                                ) {
                                    text_out = tr.text;
                                    used_engine = "groq_whisper";
                                }
                            }
                            #[cfg(not(windows))]
                            let _ = hdrs;
                            body.zeroize_now();
                        }
                        wav.fill(0);
                    }
                }
                if text_out.trim().is_empty() {
                    if let Ok(sapi) = miku_ui::voice::LocalSapiRecognizer::new("es-MX")
                        .recognize_pcm_live(&samples, 16_000, true)
                    {
                        text_out = sapi.transcript;
                        used_engine = "local_sapi";
                    }
                }
                samples.fill(0);
                let clean = text_out.trim();
                if !clean.is_empty() {
                    let _ = events_tx.send(json!({
                        "type": "voice_transcript",
                        "text": clean,
                        "is_final": true,
                        "engine": used_engine
                    }));
                }
            }
            stop.store(true, Ordering::SeqCst);
            let _ = events_tx.send(json!({ "type": "dictation", "active": false }));
        });
    }

    pub(super) fn handle_ws_command_inner(&self, cmd: &Value) -> Option<Value> {
        match cmd.get("type").and_then(|v| v.as_str()).unwrap_or("") {
            "chat" => {
                let text = cmd.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let agent_id = cmd
                    .get("agent_id")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or("miku");
                let file_name = cmd
                    .get("file")
                    .and_then(|f| f.get("name"))
                    .and_then(|v| v.as_str());
                let _ = self.process_chat(agent_id, text, file_name);
                None
            }
            "cancel" => Some(json!({ "type": "cancelled", "ok": true })),
            "dictate" => {
                let on = cmd.get("on").and_then(|v| v.as_bool()).unwrap_or(false);
                if !on {
                    self.voice_capture_stop.store(true, Ordering::SeqCst);
                    Some(json!({ "type": "dictation", "active": false }))
                } else if !self.is_voice_stt_enabled() {
                    let ev = json!({ "type": "voice_consent_required", "active": false });
                    let _ = self.events_tx.send(ev.clone());
                    Some(ev)
                } else {
                    let engine = cmd
                        .get("engine")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| self.current_voice_stt_engine());
                    if engine == "off" {
                        let ev = json!({ "type": "voice_consent_required", "active": false });
                        let _ = self.events_tx.send(ev.clone());
                        Some(ev)
                    } else {
                        if engine == "local_sapi" || engine == "groq_whisper" {
                            self.spawn_native_voice_dictation(&engine);
                        }
                        Some(json!({ "type": "dictation", "active": true, "engine": engine }))
                    }
                }
            }
            "agent_state" => {
                let st = cmd.get("state").and_then(|v| v.as_str()).unwrap_or("idle");
                self.broadcast_agent_state(st);
                Some(json!({ "type": "agent_state_ack", "state": st, "ok": true }))
            }
            "voice_transcript" => {
                let text = cmd.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let is_final = cmd.get("is_final").and_then(|v| v.as_bool()).unwrap_or(false);
                let engine = cmd.get("engine").and_then(|v| v.as_str()).unwrap_or("off");
                self.broadcast_voice_transcript(text, is_final, engine);
                Some(json!({ "type": "voice_transcript_ack", "ok": true }))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use miku_gateway::LoopbackApiHandler;
    use serde_json::json;

    use super::*;

    #[test]
    fn ws_dictate_requires_strict_opt_in_and_clears_vision_state() {
        let dir = std::env::temp_dir().join(format!(
            "miku-voice-optin-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let handler = LiveMikuApiHandler::new(&dir, 0, true).unwrap();
        assert!(!handler.is_voice_stt_enabled());

        let resp = handler
            .handle_ws_command(&json!({ "type": "dictate", "on": true }))
            .expect("dictate response");
        assert_eq!(resp["type"], "voice_consent_required");
        assert_eq!(resp["active"], false);

        handler.set_turn_vision_attachment(LlmImageAttachment {
            mime_type: "image/png".to_string(),
            base64_data: "iVBORw0KGgo=".to_string(),
            ocr_text: Some("Screen".to_string()),
            uia_summary: Some("[Window] Desktop".to_string()),
            window_title: "Desktop".to_string(),
            width: 3840,
            height: 1080,
            transform: CaptureTransform {
                origin_virtual_x: -1920,
                origin_virtual_y: 0,
                width_px: 3840,
                height_px: 1080,
                dpi_scale: 1.5,
            },
        });
        let tr = handler.last_capture_transform();
        assert_eq!(tr.origin_virtual_x, -1920);
        assert_eq!(tr.width_px, 3840);
        let (px, py) = tr.normalized_1000_to_virtual(500.0, 500.0);
        assert_eq!((px, py), (0, 540));

        handler.clear_turn_vision_state();
        assert!(handler.take_pending_multimodal_attachment().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
