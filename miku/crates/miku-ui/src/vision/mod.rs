pub mod ocr;
pub mod orchestrator;
pub mod types_and_codec;
pub mod win32_backend;
pub mod win32_dpi_guard;
mod winrt_ocr;

#[cfg(test)]
mod tests;

pub use ocr::{
    DeterministicFixtureOcrBackend, LocalOcrBackend, LocalWindowsOcrBackend, OcrLine, OcrResult,
    OcrWord,
};
pub use orchestrator::{
    ScreenVisionController, TurnVisionPayload, VisionCaptureOutcome, VisionConfirmationPreview,
};
pub use types_and_codec::{
    compress_rgba_to_bounded_png, decode_png_ihdr_dimensions, downscale_rgba_box, encode_rgba_png,
    zeroize_slice, BoundedPngFrame, RawRgbaFrame, VisionCapturePolicy, VisionCaptureTarget,
};
pub use win32_backend::{
    DeterministicFixtureVisionBackend, ScreenCaptureBackend, Win32GdiScreenVisionBackend,
};
