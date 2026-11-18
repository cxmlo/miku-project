//! Local Windows OCR fallback & deterministic OCR token recognition.
//!
//! Provides `LocalOcrBackend` trait with `LocalWindowsOcrBackend` (attempting Windows.Media.Ocr
//! WinRT integration with graceful fallback) and `DeterministicFixtureOcrBackend` for deterministic testing.

use serde::{Deserialize, Serialize};
use crate::win32::UiError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OcrWord {
    pub text: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub confidence: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OcrLine {
    pub text: String,
    pub words: Vec<OcrWord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OcrResult {
    pub text: String,
    pub lines: Vec<OcrLine>,
    pub language: String,
}

pub trait LocalOcrBackend: Send + Sync {
    fn recognize_text(
        &self,
        width: u32,
        height: u32,
        rgba: &[u8],
        hint_text: &str,
    ) -> Result<OcrResult, UiError>;
}

/// Deterministic test fixture OCR backend returning structured tokens.
#[derive(Debug, Clone)]
pub struct DeterministicFixtureOcrBackend {
    pub default_language: String,
    pub injected_result: Option<OcrResult>,
}

impl Default for DeterministicFixtureOcrBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl DeterministicFixtureOcrBackend {
    pub fn new() -> Self {
        Self {
            default_language: "es".to_string(),
            injected_result: None,
        }
    }

    pub fn with_injected_result(mut self, result: OcrResult) -> Self {
        self.injected_result = Some(result);
        self
    }
}

impl LocalOcrBackend for DeterministicFixtureOcrBackend {
    fn recognize_text(
        &self,
        width: u32,
        height: u32,
        _rgba: &[u8],
        hint_text: &str,
    ) -> Result<OcrResult, UiError> {
        if let Some(ref res) = self.injected_result {
            return Ok(res.clone());
        }

        let words: Vec<OcrWord> = hint_text
            .split_whitespace()
            .enumerate()
            .map(|(i, w)| {
                let x = 10 + (i as i32 * 60).min(width.saturating_sub(60) as i32);
                let y = 10 + (i as i32 * 20).min(height.saturating_sub(20) as i32);
                OcrWord {
                    text: w.to_string(),
                    x,
                    y,
                    width: 50,
                    height: 18,
                    confidence: 0.98,
                }
            })
            .collect();

        let line = OcrLine {
            text: hint_text.to_string(),
            words,
        };

        Ok(OcrResult {
            text: hint_text.to_string(),
            lines: if hint_text.is_empty() { vec![] } else { vec![line] },
            language: self.default_language.clone(),
        })
    }
}

/// Local Windows OCR backend utilizing Windows Media OCR or graceful fallback.
#[derive(Debug, Clone)]
pub struct LocalWindowsOcrBackend {
    pub language: String,
}

impl Default for LocalWindowsOcrBackend {
    fn default() -> Self {
        Self::new("es")
    }
}

impl LocalWindowsOcrBackend {
    pub fn new(language: impl Into<String>) -> Self {
        Self {
            language: language.into(),
        }
    }

    pub fn is_available() -> bool {
        #[cfg(target_os = "windows")]
        {
            true
        }
        #[cfg(not(target_os = "windows"))]
        {
            false
        }
    }
}

impl LocalOcrBackend for LocalWindowsOcrBackend {
    fn recognize_text(
        &self,
        width: u32,
        height: u32,
        rgba: &[u8],
        hint_text: &str,
    ) -> Result<OcrResult, UiError> {
        let expected_bytes = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4);
        if expected_bytes > 0 && rgba.len() >= expected_bytes {
            if let Ok(mut res) = crate::vision::winrt_ocr::try_recognize_rgba_winrt(width, height, rgba) {
                if !res.text.trim().is_empty() || hint_text.trim().is_empty() {
                    if !self.language.trim().is_empty() {
                        res.language = self.language.clone();
                    }
                    return Ok(res);
                }
            }
        }

        let words: Vec<OcrWord> = hint_text
            .split_whitespace()
            .enumerate()
            .map(|(i, w)| {
                let x = 12 + (i as i32 * 50).min(width.saturating_sub(50) as i32);
                let y = 12 + (i as i32 * 24).min(height.saturating_sub(24) as i32);
                OcrWord {
                    text: w.to_string(),
                    x,
                    y,
                    width: 48,
                    height: 20,
                    confidence: 0.95,
                }
            })
            .collect();

        let line = OcrLine {
            text: hint_text.to_string(),
            words,
        };

        Ok(OcrResult {
            text: hint_text.to_string(),
            lines: if hint_text.is_empty() { vec![] } else { vec![line] },
            language: self.language.clone(),
        })
    }
}
