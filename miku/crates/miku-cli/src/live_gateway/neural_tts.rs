//! Neural TTS synthesis & voice discovery (Gemini TTS, ElevenLabs, Edge/Browser fallback).

use miku_protocol::base64_encode;
use serde_json::{json, Value};

use super::api_handler::LiveMikuApiHandler;
use super::winhttp_https::{get_https, post_https};

pub(super) fn handle_tts_voices(handler: &LiveMikuApiHandler) -> (u16, Value) {
    let eleven_key = {
        let mut st = handler.state.lock().expect("state mutex");
        st.resolve_secret_by_alias("elevenlabs")
    };

    let mut eleven_voices = vec![
        json!({ "id": "21m00Tcm4TlvDq8ikWAM", "name": "Rachel (Femenina · Clara)" }),
        json!({ "id": "EXAVITQu4vr4xnSDxMaL", "name": "Bella (Femenina · Suave)" }),
        json!({ "id": "MF3mGyEYCl7XYWbV9V6O", "name": "Elli (Femenina · Juvenil)" }),
        json!({ "id": "AZnzlk1XvdvUeBnXmlld", "name": "Domi (Femenina · Expresiva)" }),
    ];

    if let Some(key) = eleven_key {
        let headers = vec![
            ("xi-api-key".to_string(), key),
            ("Accept".to_string(), "application/json".to_string()),
        ];
        if let Ok((200, bytes)) = get_https("api.elevenlabs.io", 443, "/v1/voices", &headers) {
            if let Ok(val) = serde_json::from_slice::<Value>(&bytes) {
                if let Some(arr) = val.get("voices").and_then(|v| v.as_array()) {
                    let live: Vec<Value> = arr
                        .iter()
                        .filter_map(|v| {
                            let id = v.get("voice_id").and_then(|s| s.as_str())?;
                            let name = v.get("name").and_then(|s| s.as_str()).unwrap_or(id);
                            let gender = v
                                .get("labels")
                                .and_then(|l| l.get("gender"))
                                .and_then(|g| g.as_str())
                                .unwrap_or("");
                            let tag = if gender.is_empty() {
                                String::new()
                            } else {
                                format!(" ({})", gender)
                            };
                            Some(json!({ "id": id, "name": format!("{}{}", name, tag) }))
                        })
                        .collect();
                    if !live.is_empty() {
                        eleven_voices = live;
                    }
                }
            }
        }
    }

    (
        200,
        json!({
            "ok": true,
            "engines": {
                "gemini": [
                    { "id": "Kore", "name": "Kore (Femenina · Firme y natural)" },
                    { "id": "Aoede", "name": "Aoede (Femenina · Cálida)" },
                    { "id": "Leda", "name": "Leda (Femenina · Juvenil)" },
                    { "id": "Zephyr", "name": "Zephyr (Femenina · Brillante)" }
                ],
                "elevenlabs": eleven_voices,
                "browser": [
                    { "id": "auto-female", "name": "Voz Femenina del Sistema / WebView" }
                ]
            }
        }),
    )
}

pub(super) fn handle_tts_speak(handler: &LiveMikuApiHandler, parsed: &Value) -> (u16, Value) {
    let text = parsed
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if text.is_empty() {
        return (400, json!({ "ok": false, "error": "text required" }));
    }

    let (engine, voice, gemini_key, eleven_key) = {
        let mut st = handler.state.lock().expect("state mutex");
        let eng = parsed
            .get("engine")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().to_ascii_lowercase())
            .or_else(|| {
                st.settings
                    .get("voice_tts_engine")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_ascii_lowercase())
            })
            .unwrap_or_else(|| "auto".to_string());
        let vc = parsed
            .get("voice")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().to_string())
            .or_else(|| {
                st.settings
                    .get("voice_tts_voice")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
            })
            .unwrap_or_default();
        let g_key = st.resolve_secret_by_alias("gemini");
        let e_key = st.resolve_secret_by_alias("elevenlabs");
        (eng, vc, g_key, e_key)
    };

    let resolved_engine = match engine.as_str() {
        "gemini" => "gemini",
        "elevenlabs" => "elevenlabs",
        "browser" => "browser",
        _ => {
            if gemini_key.is_some() {
                "gemini"
            } else if eleven_key.is_some() {
                "elevenlabs"
            } else {
                "browser"
            }
        }
    };

    match resolved_engine {
        "gemini" => {
            let Some(key) = gemini_key else {
                return (
                    200,
                    json!({ "ok": false, "fallback": "browser", "error": "Falta API key de Gemini en DPAPI" }),
                );
            };
            let voice_name = if voice.is_empty() || voice.len() > 20 {
                "Kore"
            } else {
                voice.as_str()
            };
            match synthesize_gemini_wav(&key, voice_name, text) {
                Ok(wav_b64) => (
                    200,
                    json!({
                        "ok": true,
                        "engine": "gemini",
                        "voice": voice_name,
                        "mime": "audio/wav",
                        "audio_b64": wav_b64
                    }),
                ),
                Err(e) => (
                    200,
                    json!({ "ok": false, "fallback": "browser", "error": e }),
                ),
            }
        }
        "elevenlabs" => {
            let Some(key) = eleven_key else {
                return (
                    200,
                    json!({ "ok": false, "fallback": "browser", "error": "Falta API key de ElevenLabs en DPAPI" }),
                );
            };
            let voice_id = if voice.is_empty() || matches!(voice.as_str(), "Kore" | "Aoede" | "Leda" | "Zephyr" | "auto-female") {
                "21m00Tcm4TlvDq8ikWAM"
            } else {
                voice.as_str()
            };
            match synthesize_elevenlabs_mp3(&key, voice_id, text) {
                Ok(mp3_b64) => (
                    200,
                    json!({
                        "ok": true,
                        "engine": "elevenlabs",
                        "voice": voice_id,
                        "mime": "audio/mpeg",
                        "audio_b64": mp3_b64
                    }),
                ),
                Err(e) => (
                    200,
                    json!({ "ok": false, "fallback": "browser", "error": e }),
                ),
            }
        }
        _ => (
            200,
            json!({ "ok": false, "fallback": "browser", "engine": "browser" }),
        ),
    }
}

fn synthesize_gemini_wav(api_key: &str, voice_name: &str, text: &str) -> Result<String, String> {
    let path = "/v1beta/models/gemini-2.5-flash-preview-tts:generateContent";
    let headers = vec![
        ("Content-Type".to_string(), "application/json".to_string()),
        ("x-goog-api-key".to_string(), api_key.to_string()),
    ];
    let payload = json!({
        "contents": [{ "parts": [{ "text": text }] }],
        "generationConfig": {
            "responseModalities": ["AUDIO"],
            "speechConfig": {
                "voiceConfig": {
                    "prebuiltVoiceConfig": { "voiceName": voice_name }
                }
            }
        }
    });
    let body = serde_json::to_vec(&payload).map_err(|e| e.to_string())?;
    let (status, resp_bytes) = post_https(
        "generativelanguage.googleapis.com",
        443,
        path,
        &headers,
        &body,
    )?;
    if !(200..300).contains(&status) {
        let msg = String::from_utf8_lossy(&resp_bytes);
        return Err(format!(
            "Gemini TTS HTTP {}: {}",
            status,
            msg.chars().take(180).collect::<String>()
        ));
    }
    let val: Value = serde_json::from_slice(&resp_bytes)
        .map_err(|e| format!("JSON Gemini TTS inválido: {}", e))?;
    let b64 = val
        .get("candidates")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("content"))
        .and_then(|c| c.get("parts"))
        .and_then(|p| p.get(0))
        .and_then(|p| p.get("inlineData"))
        .and_then(|d| d.get("data"))
        .and_then(|s| s.as_str())
        .ok_or_else(|| "Gemini TTS no devolvió inlineData.data".to_string())?;
    let pcm = decode_base64(b64)?;
    let wav = wrap_pcm_as_wav(&pcm, 24_000, 1, 16);
    Ok(base64_encode(&wav))
}

fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity((input.len() * 3) / 4);
    let mut buf: u32 = 0;
    let mut bits: u8 = 0;
    for b in input.bytes() {
        if b.is_ascii_whitespace() || b == b'=' {
            continue;
        }
        let val = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return Err("Carácter base64 inválido en audio Gemini".to_string()),
        };
        buf = (buf << 6) | (val as u32);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    Ok(out)
}

fn synthesize_elevenlabs_mp3(api_key: &str, voice_id: &str, text: &str) -> Result<String, String> {
    let clean_id: String = voice_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    let path = format!("/v1/text-to-speech/{}", clean_id);
    let headers = vec![
        ("Content-Type".to_string(), "application/json".to_string()),
        ("Accept".to_string(), "audio/mpeg".to_string()),
        ("xi-api-key".to_string(), api_key.to_string()),
    ];
    let payload = json!({
        "text": text,
        "model_id": "eleven_multilingual_v2",
        "voice_settings": { "stability": 0.45, "similarity_boost": 0.78 }
    });
    let body = serde_json::to_vec(&payload).map_err(|e| e.to_string())?;
    let (status, resp_bytes) = post_https("api.elevenlabs.io", 443, &path, &headers, &body)?;
    if !(200..300).contains(&status) {
        let msg = String::from_utf8_lossy(&resp_bytes);
        return Err(format!(
            "ElevenLabs HTTP {}: {}",
            status,
            msg.chars().take(180).collect::<String>()
        ));
    }
    Ok(base64_encode(&resp_bytes))
}

fn wrap_pcm_as_wav(pcm: &[u8], sample_rate: u32, channels: u16, bits_per_sample: u16) -> Vec<u8> {
    let byte_rate = sample_rate * (channels as u32) * (bits_per_sample as u32 / 8);
    let block_align = channels * (bits_per_sample / 8);
    let data_len = pcm.len() as u32;
    let riff_len = 36 + data_len;
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_len.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits_per_sample.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(pcm);
    out
}
