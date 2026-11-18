use std::ptr;
use std::sync::atomic::{compiler_fence, Ordering};

use miku_protocol::sha256_hex;
use serde::{Deserialize, Serialize};

use crate::win32::UiError;

pub const DEFAULT_VISION_MAX_DIMENSION_PX: u32 = 1024;
pub const DEFAULT_VISION_MAX_OUTPUT_BYTES: usize = 250 * 1024;
pub const DEFAULT_VISION_PREVIEW_MAX_WIDTH_PX: u32 = 256;
pub const DEFAULT_VISION_PREVIEW_MAX_HEIGHT_PX: u32 = 144;
pub const DEFAULT_VISION_PREVIEW_MAX_BYTES: usize = 160 * 1024;
pub const PNG_MAGIC_BYTES: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisionCaptureTarget {
    ActiveWindow,
    SelectedRegion {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },
}

impl VisionCaptureTarget {
    pub fn canonical_gatekeeper_target(&self) -> String {
        match self {
            VisionCaptureTarget::ActiveWindow => "screen_capture:active_window".to_string(),
            VisionCaptureTarget::SelectedRegion {
                x,
                y,
                width,
                height,
            } => format!("screen_capture:region:{},{},{},{}", x, y, width, height),
        }
    }

    pub fn validate(&self) -> Result<(), UiError> {
        match self {
            VisionCaptureTarget::ActiveWindow => Ok(()),
            VisionCaptureTarget::SelectedRegion { width, height, .. } => {
                if *width == 0 || *height == 0 {
                    return Err(UiError::VisionCaptureError(
                        "selected region width and height must be non-zero".to_string(),
                    ));
                }
                if *width > 16_384 || *height > 16_384 {
                    return Err(UiError::VisionCaptureError(
                        "selected region dimensions exceed safe 16K limit".to_string(),
                    ));
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisionCapturePolicy {
    pub max_dimension_px: u32,
    pub max_output_bytes: usize,
    pub preview_max_width_px: u32,
    pub preview_max_height_px: u32,
    pub preview_max_bytes: usize,
    pub continuous_recording_forbidden: bool,
    pub zero_disk_temp_files: bool,
}

impl Default for VisionCapturePolicy {
    fn default() -> Self {
        Self {
            max_dimension_px: DEFAULT_VISION_MAX_DIMENSION_PX,
            max_output_bytes: DEFAULT_VISION_MAX_OUTPUT_BYTES,
            preview_max_width_px: DEFAULT_VISION_PREVIEW_MAX_WIDTH_PX,
            preview_max_height_px: DEFAULT_VISION_PREVIEW_MAX_HEIGHT_PX,
            preview_max_bytes: DEFAULT_VISION_PREVIEW_MAX_BYTES,
            continuous_recording_forbidden: true,
            zero_disk_temp_files: true,
        }
    }
}

impl VisionCapturePolicy {
    pub fn validate(&self) -> Result<(), UiError> {
        if !self.continuous_recording_forbidden {
            return Err(UiError::VisionCaptureError(
                "continuous background recording must remain forbidden".to_string(),
            ));
        }
        if !self.zero_disk_temp_files {
            return Err(UiError::VisionCaptureError(
                "vision capture must operate strictly in memory without disk temp files"
                    .to_string(),
            ));
        }
        if self.max_dimension_px < 16 || self.preview_max_width_px < 16 || self.preview_max_height_px < 16 {
            return Err(UiError::VisionCaptureError(
                "vision dimension limits must be at least 16 px".to_string(),
            ));
        }
        if self.max_output_bytes < 128 || self.preview_max_bytes < 128 {
            return Err(UiError::VisionCaptureError(
                "vision byte limits must be at least 128 bytes".to_string(),
            ));
        }
        Ok(())
    }
}

pub fn zeroize_slice(buf: &mut [u8]) {
    for b in buf.iter_mut() {
        // SAFETY: `b` is a valid mutable pointer into `buf`.
        unsafe {
            ptr::write_volatile(b, 0u8);
        }
    }
    compiler_fence(Ordering::SeqCst);
}

#[derive(Debug, Clone)]
pub struct RawRgbaFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub window_title: String,
    pub captured_at_ms: u64,
    pub transform: miku_protocol::CaptureTransform,
}

impl RawRgbaFrame {
    pub fn new(
        width: u32,
        height: u32,
        rgba: Vec<u8>,
        window_title: String,
        captured_at_ms: u64,
    ) -> Self {
        Self {
            width,
            height,
            rgba,
            window_title,
            captured_at_ms,
            transform: miku_protocol::CaptureTransform {
                origin_virtual_x: 0,
                origin_virtual_y: 0,
                width_px: width,
                height_px: height,
                dpi_scale: 1.0,
            },
        }
    }
    pub fn validate(&self) -> Result<(), UiError> {
        if self.width == 0 || self.height == 0 {
            return Err(UiError::VisionCaptureError(
                "captured frame dimensions must be non-zero".to_string(),
            ));
        }
        let expected_len = (self.width as usize)
            .checked_mul(self.height as usize)
            .and_then(|px| px.checked_mul(4))
            .ok_or_else(|| {
                UiError::VisionCaptureError("captured frame byte size overflow".to_string())
            })?;
        if self.rgba.len() != expected_len {
            return Err(UiError::VisionCaptureError(format!(
                "invalid RGBA buffer length: expected {}, got {}",
                expected_len,
                self.rgba.len()
            )));
        }
        Ok(())
    }

    pub fn zeroize_and_clear(&mut self) {
        zeroize_slice(&mut self.rgba);
        self.rgba.clear();
        self.width = 0;
        self.height = 0;
    }
}

impl Drop for RawRgbaFrame {
    fn drop(&mut self) {
        zeroize_slice(&mut self.rgba);
    }
}

#[derive(Debug, Clone)]
pub struct BoundedPngFrame {
    pub width: u32,
    pub height: u32,
    pub png_bytes: Vec<u8>,
    pub sha256_hex: String,
    pub downscale_passes: u32,
}

impl BoundedPngFrame {
    pub fn byte_len(&self) -> usize {
        self.png_bytes.len()
    }

    pub fn zeroize_and_clear(&mut self) {
        zeroize_slice(&mut self.png_bytes);
        self.png_bytes.clear();
        self.width = 0;
        self.height = 0;
    }
}

impl Drop for BoundedPngFrame {
    fn drop(&mut self) {
        zeroize_slice(&mut self.png_bytes);
    }
}

pub fn downscale_rgba_box(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
) -> Result<Vec<u8>, UiError> {
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return Err(UiError::VisionCaptureError(
            "downscale dimensions must be non-zero".to_string(),
        ));
    }
    let expected = (src_w as usize) * (src_h as usize) * 4;
    if src.len() != expected {
        return Err(UiError::VisionCaptureError(
            "source buffer length does not match RGBA dimensions".to_string(),
        ));
    }
    if src_w == dst_w && src_h == dst_h {
        return Ok(src.to_vec());
    }

    let mut out = vec![0u8; (dst_w as usize) * (dst_h as usize) * 4];
    for dy in 0..dst_h {
        let sy0 = ((dy as u64) * (src_h as u64) / (dst_h as u64)) as u32;
        let sy1 = ((((dy + 1) as u64) * (src_h as u64)).div_ceil(dst_h as u64) as u32)
            .min(src_h)
            .max(sy0 + 1);
        for dx in 0..dst_w {
            let sx0 = ((dx as u64) * (src_w as u64) / (dst_w as u64)) as u32;
            let sx1 = ((((dx + 1) as u64) * (src_w as u64)).div_ceil(dst_w as u64) as u32)
                .min(src_w)
                .max(sx0 + 1);

            let mut acc = [0u64; 4];
            let mut count = 0u64;
            for sy in sy0..sy1 {
                let row_off = (sy as usize) * (src_w as usize) * 4;
                for sx in sx0..sx1 {
                    let off = row_off + (sx as usize) * 4;
                    acc[0] += src[off] as u64;
                    acc[1] += src[off + 1] as u64;
                    acc[2] += src[off + 2] as u64;
                    acc[3] += src[off + 3] as u64;
                    count += 1;
                }
            }
            let out_off = ((dy as usize) * (dst_w as usize) + (dx as usize)) * 4;
            let denom = count.max(1);
            out[out_off] = (acc[0] / denom) as u8;
            out[out_off + 1] = (acc[1] / denom) as u8;
            out[out_off + 2] = (acc[2] / denom) as u8;
            out[out_off + 3] = (acc[3] / denom) as u8;
        }
    }
    Ok(out)
}

fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320u32 & mask);
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    const MOD_ADLER: u32 = 65_521;
    let mut s1: u32 = 1;
    let mut s2: u32 = 0;
    for &byte in data {
        s1 = (s1 + (byte as u32)) % MOD_ADLER;
        s2 = (s2 + s1) % MOD_ADLER;
    }
    (s2 << 16) | s1
}

fn append_png_chunk(out: &mut Vec<u8>, tag: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    let crc_start = out.len();
    out.extend_from_slice(tag);
    out.extend_from_slice(payload);
    let crc = crc32_ieee(&out[crc_start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// Encodes an RGBA8 buffer into a valid standard PNG byte stream without external dependencies.
pub fn encode_rgba_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, UiError> {
    if width == 0 || height == 0 {
        return Err(UiError::VisionCaptureError(
            "cannot encode 0-dimension PNG".to_string(),
        ));
    }
    let expected = (width as usize) * (height as usize) * 4;
    if rgba.len() != expected {
        return Err(UiError::VisionCaptureError(
            "RGBA slice length does not match PNG dimensions".to_string(),
        ));
    }

    let row_bytes = (width as usize) * 4;
    let mut scanlines = Vec::with_capacity((row_bytes + 1) * (height as usize));
    for y in 0..(height as usize) {
        scanlines.push(0u8); // Filter type 0: None
        let start = y * row_bytes;
        scanlines.extend_from_slice(&rgba[start..start + row_bytes]);
    }

    // zlib header: CMF=0x78, FLG=0x01 (0x7801 % 31 == 0)
    let mut zlib_stream = Vec::with_capacity(scanlines.len() + 64);
    zlib_stream.push(0x78);
    zlib_stream.push(0x01);

    if scanlines.is_empty() {
        zlib_stream.extend_from_slice(&[0x01, 0x00, 0x00, 0xFF, 0xFF]);
    } else {
        let total = scanlines.len();
        let mut offset = 0usize;
        while offset < total {
            let chunk_len = (total - offset).min(65_535);
            let is_final = offset + chunk_len == total;
            zlib_stream.push(if is_final { 0x01 } else { 0x00 });
            let len16 = chunk_len as u16;
            let nlen16 = !len16;
            zlib_stream.extend_from_slice(&len16.to_le_bytes());
            zlib_stream.extend_from_slice(&nlen16.to_le_bytes());
            zlib_stream.extend_from_slice(&scanlines[offset..offset + chunk_len]);
            offset += chunk_len;
        }
    }
    let adler = adler32(&scanlines);
    zlib_stream.extend_from_slice(&adler.to_be_bytes());
    zeroize_slice(&mut scanlines);

    let mut png = Vec::with_capacity(zlib_stream.len() + 64);
    png.extend_from_slice(&PNG_MAGIC_BYTES);

    let mut ihdr = [0u8; 13];
    ihdr[0..4].copy_from_slice(&width.to_be_bytes());
    ihdr[4..8].copy_from_slice(&height.to_be_bytes());
    ihdr[8] = 8; // bit depth
    ihdr[9] = 6; // RGBA
    ihdr[10] = 0; // compression method
    ihdr[11] = 0; // filter method
    ihdr[12] = 0; // interlace
    append_png_chunk(&mut png, b"IHDR", &ihdr);
    append_png_chunk(&mut png, b"IDAT", &zlib_stream);
    append_png_chunk(&mut png, b"IEND", &[]);
    zeroize_slice(&mut zlib_stream);
    Ok(png)
}

pub fn decode_png_ihdr_dimensions(png: &[u8]) -> Result<(u32, u32), UiError> {
    if png.len() < 33 || png[0..8] != PNG_MAGIC_BYTES || &png[12..16] != b"IHDR" {
        return Err(UiError::VisionCaptureError(
            "invalid PNG header or missing IHDR chunk".to_string(),
        ));
    }
    let width = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
    let height = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
    if width == 0 || height == 0 {
        return Err(UiError::VisionCaptureError(
            "PNG IHDR reported zero dimensions".to_string(),
        ));
    }
    Ok((width, height))
}

fn fit_within_bounds(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if w <= max_w && h <= max_h {
        return (w, h);
    }
    let scale_w = (max_w as f64) / (w.max(1) as f64);
    let scale_h = (max_h as f64) / (h.max(1) as f64);
    let scale = scale_w.min(scale_h);
    let nw = ((w as f64) * scale).floor().max(1.0) as u32;
    let nh = ((h as f64) * scale).floor().max(1.0) as u32;
    (nw.min(max_w), nh.min(max_h))
}

pub fn compress_rgba_to_bounded_png(
    frame: &RawRgbaFrame,
    max_width_px: u32,
    max_height_px: u32,
    max_bytes: usize,
) -> Result<BoundedPngFrame, UiError> {
    frame.validate()?;
    if max_bytes < 128 {
        return Err(UiError::VisionCaptureError(
            "max_bytes budget is too small for a valid PNG".to_string(),
        ));
    }

    let (mut cur_w, mut cur_h) = fit_within_bounds(frame.width, frame.height, max_width_px, max_height_px);
    let initial_pass = u32::from(cur_w != frame.width || cur_h != frame.height);
    let mut cur_rgba = downscale_rgba_box(&frame.rgba, frame.width, frame.height, cur_w, cur_h)?;

    for downscale_passes in (initial_pass..).take(16) {
        let png = encode_rgba_png(&cur_rgba, cur_w, cur_h)?;
        if png.len() <= max_bytes {
            let digest = sha256_hex(&png);
            zeroize_slice(&mut cur_rgba);
            return Ok(BoundedPngFrame {
                width: cur_w,
                height: cur_h,
                png_bytes: png,
                sha256_hex: digest,
                downscale_passes,
            });
        }
        if cur_w <= 4 && cur_h <= 4 {
            let err_len = png.len();
            let mut png_drop = png;
            zeroize_slice(&mut png_drop);
            zeroize_slice(&mut cur_rgba);
            return Err(UiError::VisionCaptureError(format!(
                "cannot fit PNG within {} bytes (even at {}x{} needed {} bytes)",
                max_bytes, cur_w, cur_h, err_len
            )));
        }
        let next_w = ((cur_w as f64) * 0.72).floor().max(4.0) as u32;
        let next_h = ((cur_h as f64) * 0.72).floor().max(4.0) as u32;
        let (nw, nh) = if next_w == cur_w && next_h == cur_h {
            ((cur_w / 2).max(1), (cur_h / 2).max(1))
        } else {
            (next_w, next_h)
        };
        let next_rgba = downscale_rgba_box(&cur_rgba, cur_w, cur_h, nw, nh)?;
        zeroize_slice(&mut cur_rgba);
        cur_rgba = next_rgba;
        cur_w = nw;
        cur_h = nh;
    }

    zeroize_slice(&mut cur_rgba);
    Err(UiError::VisionCaptureError(format!(
        "exceeded max downscale iterations fitting PNG under {} bytes",
        max_bytes
    )))
}
