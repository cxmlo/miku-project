use std::collections::HashMap;

use miku_gatekeeper::{Capability, GatekeeperEngine, GatekeeperVerdict, PermissionTicket};
use miku_protocol::PermissionDecisionKind;
use serde::{Deserialize, Serialize};

use crate::vision::types_and_codec::{
    compress_rgba_to_bounded_png, zeroize_slice, BoundedPngFrame, VisionCapturePolicy,
    VisionCaptureTarget,
};
use crate::vision::win32_backend::ScreenCaptureBackend;
use crate::webview::{
    sanitize_for_text_content, WebView2IpcBridge, WebViewIpcEnvelopeV1, WebViewIpcPayloadV1,
};
use crate::win32::{UiError, OVERVIEW_VIRTUAL_INDEX_URL};

const DEFAULT_EXCLUDED_SENSITIVE_WINDOW_PATTERNS: &[&str] = &[
    "1password",
    "bitwarden",
    "keepass",
    "lastpass",
    "dashlane",
    "proton pass",
    "windows security",
    "seguridad de windows",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VisionConfirmationPreview {
    pub req_hash: String,
    pub canonical_target: String,
    pub sanitized_window_title: String,
    pub preview_width: u32,
    pub preview_height: u32,
    pub preview_byte_len: usize,
    pub preview_png_bytes: Vec<u8>,
    pub preview_sha256: String,
    pub dom_title_render: WebViewIpcPayloadV1,
    pub dom_meta_render: WebViewIpcPayloadV1,
    pub zero_disk_temp_files: bool,
}

impl VisionConfirmationPreview {
    pub fn zeroize_and_clear(&mut self) {
        zeroize_slice(&mut self.preview_png_bytes);
        self.preview_png_bytes.clear();
        self.preview_byte_len = 0;
    }
}

impl Drop for VisionConfirmationPreview {
    fn drop(&mut self) {
        zeroize_slice(&mut self.preview_png_bytes);
    }
}

#[derive(Debug)]
pub struct TurnVisionPayload {
    pub req_hash: String,
    pub canonical_target: String,
    pub sanitized_window_title: String,
    pub width: u32,
    pub height: u32,
    pub png_bytes: Vec<u8>,
    pub sha256_hex: String,
    pub downscale_passes: u32,
    pub confirmation_preview: VisionConfirmationPreview,
    pub bytes_zeroed_after_turn: bool,
    pub zero_disk_temp_files: bool,
    pub transform: miku_protocol::CaptureTransform,
}

impl TurnVisionPayload {
    pub fn retained_bytes(&self) -> usize {
        self.png_bytes.len() + self.confirmation_preview.preview_png_bytes.len()
    }

    pub fn clear_after_turn(&mut self) {
        zeroize_slice(&mut self.png_bytes);
        self.png_bytes.clear();
        self.confirmation_preview.zeroize_and_clear();
        self.bytes_zeroed_after_turn = true;
    }

    /// Serializes this vision turn into a multimodal LLM payload part (base64 PNG)
    /// compatible with OpenAI / Anthropic / DeepSeek multimodal vision schemas.
    pub fn to_multimodal_llm_payload(&self, prompt: &str) -> serde_json::Value {
        let b64 = miku_protocol::base64_encode(&self.png_bytes);
        serde_json::json!([
            {
                "type": "text",
                "text": prompt
            },
            {
                "type": "image_url",
                "image_url": {
                    "url": format!("data:image/png;base64,{}", b64)
                }
            }
        ])
    }

    /// Runs local OCR on the captured vision frame via the supplied OCR backend.
    pub fn run_ocr(
        &self,
        backend: &dyn crate::vision::ocr::LocalOcrBackend,
    ) -> Result<crate::vision::ocr::OcrResult, UiError> {
        backend.recognize_text(self.width, self.height, &[], &self.sanitized_window_title)
    }
}

impl Drop for TurnVisionPayload {
    fn drop(&mut self) {
        zeroize_slice(&mut self.png_bytes);
    }
}

#[derive(Debug)]
pub enum VisionCaptureOutcome {
    Captured(TurnVisionPayload),
    AwaitingApproval {
        ticket: PermissionTicket,
        prompt_render: WebViewIpcPayloadV1,
    },
    Denied {
        reason_code: String,
        detail: String,
    },
}

#[derive(Debug, Clone)]
struct PendingVisionRequest {
    agent: String,
    target: VisionCaptureTarget,
    canonical_target: String,
    raw_args: String,
    req_hash: String,
}

pub struct ScreenVisionController<B: ScreenCaptureBackend> {
    backend: B,
    policy: VisionCapturePolicy,
    pending_requests: HashMap<String, PendingVisionRequest>,
    next_ipc_seq: u64,
    completed_captures: u64,
    denied_requests: u64,
}

impl<B: ScreenCaptureBackend> ScreenVisionController<B> {
    pub fn new(backend: B, policy: VisionCapturePolicy) -> Result<Self, UiError> {
        policy.validate()?;
        if backend.is_continuous_stream_active() {
            return Err(UiError::VisionCaptureError(
                "backend reported active continuous video stream; forbidden by policy".to_string(),
            ));
        }
        Ok(Self {
            backend,
            policy,
            pending_requests: HashMap::new(),
            next_ipc_seq: 1,
            completed_captures: 0,
            denied_requests: 0,
        })
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn policy(&self) -> &VisionCapturePolicy {
        &self.policy
    }

    pub fn active_background_capture_streams(&self) -> usize {
        usize::from(self.backend.is_continuous_stream_active())
    }

    pub fn completed_captures(&self) -> u64 {
        self.completed_captures
    }

    pub fn denied_requests(&self) -> u64 {
        self.denied_requests
    }

    pub fn request_on_demand_capture(
        &mut self,
        agent: &str,
        target: VisionCaptureTarget,
        now_ms: u64,
        gatekeeper: &mut GatekeeperEngine,
        bridge: &mut WebView2IpcBridge,
    ) -> Result<VisionCaptureOutcome, UiError> {
        self.policy.validate()?;
        target.validate()?;
        if self.backend.is_continuous_stream_active() {
            return Err(UiError::VisionCaptureError(
                "continuous background capture is strictly forbidden".to_string(),
            ));
        }

        let canonical_target = target.canonical_gatekeeper_target();
        let raw_args = format!(
            "on_demand|max_dim={}|max_bytes={}",
            self.policy.max_dimension_px, self.policy.max_output_bytes
        );

        let verdict = gatekeeper.evaluate_request(
            agent,
            Capability::Vision,
            &canonical_target,
            &raw_args,
            now_ms,
        );

        match verdict {
            GatekeeperVerdict::Allow { .. } => {
                let req_hash = miku_protocol::sha256_hex(
                    format!("allow|{}|{}|{}", agent, canonical_target, now_ms).as_bytes(),
                );
                self.execute_capture_or_deny_sensitive(&target, &req_hash, now_ms, bridge)
            }
            GatekeeperVerdict::AskPending { ticket, .. } => {
                let prompt_render = sanitize_for_text_content(
                    "miku-vision-ask-prompt",
                    &format!(
                        "{} solicita captura puntual ({})",
                        ticket.agent, ticket.canonical_target
                    ),
                )?;
                self.emit_ipc(bridge, prompt_render.clone())?;
                self.pending_requests.insert(
                    ticket.req_hash.clone(),
                    PendingVisionRequest {
                        agent: agent.to_string(),
                        target,
                        canonical_target,
                        raw_args,
                        req_hash: ticket.req_hash.clone(),
                    },
                );
                Ok(VisionCaptureOutcome::AwaitingApproval {
                    ticket,
                    prompt_render,
                })
            }
            GatekeeperVerdict::Deny { reason, detail, .. } => {
                self.denied_requests += 1;
                Ok(VisionCaptureOutcome::Denied {
                    reason_code: reason.as_code().to_string(),
                    detail,
                })
            }
        }
    }

    pub fn resolve_pending_capture(
        &mut self,
        pending_req_hash: &str,
        decision: PermissionDecisionKind,
        expected_req_hash: &str,
        from_mobile_session: bool,
        biometric_verified: bool,
        now_ms: u64,
        gatekeeper: &mut GatekeeperEngine,
        bridge: &mut WebView2IpcBridge,
    ) -> Result<VisionCaptureOutcome, UiError> {
        let pending = self.pending_requests.remove(pending_req_hash).ok_or_else(|| {
            UiError::GatekeeperInterceptionError(format!(
                "unknown or already resolved vision permission ticket '{}'",
                pending_req_hash
            ))
        })?;

        if pending.req_hash != expected_req_hash {
            self.denied_requests += 1;
            return Ok(VisionCaptureOutcome::Denied {
                reason_code: "deny_toctou".to_string(),
                detail: "pending vision req_hash mismatch before capture".to_string(),
            });
        }

        let verdict = gatekeeper.resolve_permission_decision(
            expected_req_hash,
            decision,
            from_mobile_session,
            biometric_verified,
            &pending.agent,
            Capability::Vision,
            &pending.canonical_target,
            &pending.raw_args,
            now_ms,
        );

        match verdict {
            GatekeeperVerdict::Allow { .. } => self.execute_capture_or_deny_sensitive(
                &pending.target,
                expected_req_hash,
                now_ms,
                bridge,
            ),
            GatekeeperVerdict::Deny { reason, detail, .. } => {
                self.denied_requests += 1;
                Ok(VisionCaptureOutcome::Denied {
                    reason_code: reason.as_code().to_string(),
                    detail,
                })
            }
            GatekeeperVerdict::AskPending { .. } => {
                self.denied_requests += 1;
                Err(UiError::GatekeeperInterceptionError(
                    "unexpected AskPending after resolve_ticket".to_string(),
                ))
            }
        }
    }

    pub fn abort_pending_and_zeroize(&mut self) -> usize {
        let count = self.pending_requests.len();
        self.pending_requests.clear();
        count
    }

    pub fn is_excluded_sensitive_window(title: &str) -> bool {
        let lower = title.to_ascii_lowercase();
        DEFAULT_EXCLUDED_SENSITIVE_WINDOW_PATTERNS
            .iter()
            .any(|pat| lower.contains(pat))
    }

    fn execute_capture_or_deny_sensitive(
        &mut self,
        target: &VisionCaptureTarget,
        req_hash: &str,
        now_ms: u64,
        bridge: &mut WebView2IpcBridge,
    ) -> Result<VisionCaptureOutcome, UiError> {
        let mut raw = self.backend.capture_rgba(target, now_ms)?;
        if Self::is_excluded_sensitive_window(&raw.window_title) {
            let blocked_title = raw.window_title.clone();
            raw.zeroize_and_clear();
            self.denied_requests += 1;
            return Ok(VisionCaptureOutcome::Denied {
                reason_code: "deny_sensitive_window".to_string(),
                detail: format!(
                    "captured window '{}' matched sensitive exclusion list; buffer zeroed",
                    blocked_title
                ),
            });
        }
        let bounded: BoundedPngFrame = compress_rgba_to_bounded_png(
            &raw,
            self.policy.max_dimension_px,
            self.policy.max_dimension_px,
            self.policy.max_output_bytes,
        )?;
        let preview_frame: BoundedPngFrame = compress_rgba_to_bounded_png(
            &raw,
            self.policy.preview_max_width_px,
            self.policy.preview_max_height_px,
            self.policy.preview_max_bytes,
        )?;
        let raw_title = raw.window_title.clone();
        let raw_transform = raw.transform;
        raw.zeroize_and_clear();

        let canonical_target = target.canonical_gatekeeper_target();
        let dom_title_render = sanitize_for_text_content("miku-vision-preview-title", &raw_title)?;
        let dom_meta_render = sanitize_for_text_content(
            "miku-vision-preview-meta",
            &format!(
                "{}x{} PNG ({} B) · preview {}x{} ({} B)",
                bounded.width,
                bounded.height,
                bounded.byte_len(),
                preview_frame.width,
                preview_frame.height,
                preview_frame.byte_len()
            ),
        )?;

        self.emit_ipc(bridge, dom_title_render.clone())?;
        self.emit_ipc(bridge, dom_meta_render.clone())?;

        let sanitized_window_title = match &dom_title_render {
            WebViewIpcPayloadV1::DomSafeTextRender { safe_text, .. } => safe_text.clone(),
            _ => String::new(),
        };

        let preview = VisionConfirmationPreview {
            req_hash: req_hash.to_string(),
            canonical_target: canonical_target.clone(),
            sanitized_window_title: sanitized_window_title.clone(),
            preview_width: preview_frame.width,
            preview_height: preview_frame.height,
            preview_byte_len: preview_frame.byte_len(),
            preview_png_bytes: preview_frame.png_bytes.clone(),
            preview_sha256: preview_frame.sha256_hex.clone(),
            dom_title_render,
            dom_meta_render,
            zero_disk_temp_files: self.policy.zero_disk_temp_files,
        };

        self.completed_captures += 1;

        Ok(VisionCaptureOutcome::Captured(TurnVisionPayload {
            req_hash: req_hash.to_string(),
            canonical_target,
            sanitized_window_title,
            width: bounded.width,
            height: bounded.height,
            png_bytes: bounded.png_bytes.clone(),
            sha256_hex: bounded.sha256_hex.clone(),
            downscale_passes: bounded.downscale_passes,
            confirmation_preview: preview,
            bytes_zeroed_after_turn: false,
            zero_disk_temp_files: self.policy.zero_disk_temp_files,
            transform: raw_transform,
        }))
    }

    fn emit_ipc(
        &mut self,
        bridge: &mut WebView2IpcBridge,
        payload: WebViewIpcPayloadV1,
    ) -> Result<(), UiError> {
        let env_id = format!("vision-ipc-{}", self.next_ipc_seq);
        self.next_ipc_seq += 1;
        let envelope = WebViewIpcEnvelopeV1::new(env_id, None, OVERVIEW_VIRTUAL_INDEX_URL, payload);
        bridge.enqueue_outbound(envelope)
    }
}
