//! Multimodal attachment helpers & automatic local OCR + UIA fallback for text-only LLMs.

use miku_protocol::CaptureTransform;
use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct LlmImageAttachment {
    pub mime_type: String,
    pub base64_data: String,
    pub ocr_text: Option<String>,
    pub uia_summary: Option<String>,
    pub window_title: String,
    pub width: u32,
    pub height: u32,
    pub transform: CaptureTransform,
}

impl LlmImageAttachment {
    pub fn zeroize(&mut self) {
        unsafe {
            for b in self.base64_data.as_bytes_mut() {
                *b = 0;
            }
        }
        self.base64_data.clear();
    }
}

/// Determines whether the configured provider/model accepts native multimodal image parts.
pub fn model_supports_vision(_provider: &str, model: &str) -> bool {
    let m = model.trim().to_ascii_lowercase();
    if m.is_empty() {
        return false;
    }
    const TEXT_ONLY_MARKERS: &[&str] = &[
        "deepseek-chat",
        "deepseek-reasoner",
        "deepseek-coder",
        "deepseek-v",
        "deepseek-r",
        "qwen2.5-coder",
        "qwen-coder",
        "llama3",
        "llama-3",
        "mistral",
        "mixtral",
        "phi3",
        "phi-3",
        "codellama",
        "starcoder",
    ];
    if TEXT_ONLY_MARKERS.iter().any(|p| m.contains(p)) {
        return false;
    }
    const VISION_MARKERS: &[&str] = &[
        "gpt-4o",
        "gpt-4.1",
        "gpt-4.5",
        "gpt-4-turbo",
        "gpt-4-vision",
        "o1",
        "o3",
        "o4",
        "claude-3",
        "claude-sonnet-4",
        "claude-opus-4",
        "claude-haiku-4",
        "gemini",
        "llava",
        "qwen2.5-vl",
        "qwen-vl",
        "pixtral",
        "glm-4v",
    ];
    VISION_MARKERS.iter().any(|p| m.contains(p))
}

pub fn format_vision_fallback_text(att: &LlmImageAttachment, prompt: &str) -> String {
    let mut out = format!(
        "[Contexto Visual de Pantalla (Fallback OCR Local + Árbol UIA para modelo solo texto)]\n\
         Ventana: {} | Imagen: {}x{} | CaptureTransform: origin=({},{}) size={}x{} dpi_scale={:.2}\n\
         Coordenadas normalizadas: 0..1000.\n",
        att.window_title,
        att.width,
        att.height,
        att.transform.origin_virtual_x,
        att.transform.origin_virtual_y,
        att.transform.width_px,
        att.transform.height_px,
        att.transform.dpi_scale
    );
    if let Some(ref ocr) = att.ocr_text {
        let trimmed = ocr.trim();
        if !trimmed.is_empty() {
            let cut: String = trimmed.chars().take(1200).collect();
            out.push_str(&format!("[OCR Windows]:\n{}\n", cut));
        }
    }
    if let Some(ref uia) = att.uia_summary {
        let trimmed = uia.trim();
        if !trimmed.is_empty() {
            let cut: String = trimmed.chars().take(1200).collect();
            out.push_str(&format!("[Árbol UIA Acotado]:\n{}\n", cut));
        }
    }
    out.push_str(&format!("\n{}", prompt));
    out
}

pub fn build_openai_user_content(
    provider: &str,
    model: &str,
    prompt: &str,
    att: Option<&LlmImageAttachment>,
) -> Value {
    let Some(img) = att else {
        return json!(prompt);
    };
    if !model_supports_vision(provider, model) || img.base64_data.is_empty() {
        return json!(format_vision_fallback_text(img, prompt));
    }
    let hint = format!(
        "{}\n\n[CaptureTransform: {}x{}, origin=({},{}), dpi_scale={:.2}. Emite coordenadas normalizadas x_norm,y_norm en rango 0..1000 si interactúas con controles.]",
        prompt, img.width, img.height, img.transform.origin_virtual_x, img.transform.origin_virtual_y, img.transform.dpi_scale
    );
    json!([
        { "type": "text", "text": hint },
        { "type": "image_url", "image_url": { "url": format!("data:{};base64,{}", img.mime_type, img.base64_data) } }
    ])
}

pub fn build_anthropic_user_content(
    model: &str,
    prompt: &str,
    att: Option<&LlmImageAttachment>,
) -> Value {
    let Some(img) = att else {
        return json!(prompt);
    };
    if !model_supports_vision("anthropic", model) || img.base64_data.is_empty() {
        return json!(format_vision_fallback_text(img, prompt));
    }
    json!([
        {
            "type": "image",
            "source": { "type": "base64", "media_type": img.mime_type, "data": img.base64_data }
        },
        { "type": "text", "text": prompt }
    ])
}

pub fn build_gemini_user_parts(
    model: &str,
    prompt: &str,
    att: Option<&LlmImageAttachment>,
) -> Value {
    let Some(img) = att else {
        return json!([{ "text": prompt }]);
    };
    if !model_supports_vision("gemini", model) || img.base64_data.is_empty() {
        return json!([{ "text": format_vision_fallback_text(img, prompt) }]);
    }
    json!([
        { "text": prompt },
        { "inline_data": { "mime_type": img.mime_type, "data": img.base64_data } }
    ])
}

pub fn build_ollama_user_turn(
    model: &str,
    prompt: &str,
    att: Option<&LlmImageAttachment>,
) -> Value {
    let Some(img) = att else {
        return json!({ "role": "user", "content": prompt });
    };
    if !model_supports_vision("ollama", model) || img.base64_data.is_empty() {
        return json!({ "role": "user", "content": format_vision_fallback_text(img, prompt) });
    }
    json!({
        "role": "user",
        "content": prompt,
        "images": [img.base64_data]
    })
}
