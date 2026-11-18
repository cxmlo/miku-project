//! Pure-Rust ISO/IEC 18004 QR Code encoder (MIT, 0 external dependencies).
//! Supports Byte mode, ECC Levels L & M across Versions 1..=15 with GF(2^8) Reed-Solomon,
//! BCH(15,5) Format Info, BCH(18,6) Version Info, alignment tracks, and 8-mask penalty scoring.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QrEccLevel {
    L,
    M,
}

impl QrEccLevel {
    fn format_bits(self) -> u16 {
        match self {
            QrEccLevel::L => 0b01,
            QrEccLevel::M => 0b00,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QrEncodeError {
    PayloadTooLarge { byte_len: usize, max_bytes: usize },
}

impl fmt::Display for QrEncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QrEncodeError::PayloadTooLarge { byte_len, max_bytes } => {
                write!(
                    f,
                    "QR payload {} bytes exceeds Version 15 capacity ({} bytes)",
                    byte_len, max_bytes
                )
            }
        }
    }
}

impl std::error::Error for QrEncodeError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrMatrix {
    pub version: u8,
    pub size: usize,
    pub ecc: QrEccLevel,
    pub mask: u8,
    pub modules: Vec<bool>,
}

impl QrMatrix {
    pub fn get(&self, row: usize, col: usize) -> bool {
        self.modules[row * self.size + col]
    }

    pub fn to_svg(&self, cell_px: usize, quiet_modules: usize) -> String {
        let total = (self.size + quiet_modules * 2) * cell_px;
        let mut path = String::with_capacity(self.size * self.size * 6);
        for r in 0..self.size {
            for c in 0..self.size {
                if self.get(r, c) {
                    let x = (quiet_modules + c) * cell_px;
                    let y = (quiet_modules + r) * cell_px;
                    path.push_str(&format!("M{x},{y}h{cell_px}v{cell_px}h-{cell_px}z"));
                }
            }
        }
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {total} {total}\" width=\"{total}\" height=\"{total}\" shape-rendering=\"crispEdges\" role=\"img\" aria-label=\"Miku-Link Pairing QR\"><rect width=\"100%\" height=\"100%\" fill=\"#ffffff\" rx=\"10\"/><path d=\"{path}\" fill=\"#0f172a\"/></svg>"
        )
    }
}

/// (ec_codewords_per_block, num_blocks) indexed by `version - 1` (Versions 1..=15).
const ECC_TABLE_L: [(usize, usize); 15] = [
    (7, 1),
    (10, 1),
    (15, 1),
    (20, 1),
    (26, 1),
    (18, 2),
    (20, 2),
    (24, 2),
    (30, 2),
    (18, 4),
    (20, 4),
    (24, 4),
    (26, 4),
    (30, 4),
    (22, 6),
];

const ECC_TABLE_M: [(usize, usize); 15] = [
    (10, 1),
    (16, 1),
    (26, 1),
    (18, 2),
    (24, 2),
    (16, 4),
    (18, 4),
    (22, 4),
    (22, 5),
    (26, 5),
    (30, 5),
    (22, 8),
    (22, 9),
    (24, 9),
    (24, 10),
];

const ALIGNMENT_COORDS: [&[usize]; 15] = [
    &[],
    &[6, 18],
    &[6, 22],
    &[6, 26],
    &[6, 30],
    &[6, 34],
    &[6, 22, 38],
    &[6, 24, 42],
    &[6, 26, 46],
    &[6, 28, 50],
    &[6, 30, 54],
    &[6, 32, 58],
    &[6, 34, 62],
    &[6, 26, 46, 66],
    &[6, 26, 48, 70],
];

pub fn gf_mul(mut a: u8, mut b: u8) -> u8 {
    let mut r = 0u8;
    for _ in 0..8 {
        if (b & 1) != 0 {
            r ^= a;
        }
        let hi = (a & 0x80) != 0;
        a <<= 1;
        if hi {
            a ^= 0x1D;
        }
        b >>= 1;
    }
    r
}

pub fn rs_generator_poly(deg: usize) -> Vec<u8> {
    let mut gen = vec![0u8; deg];
    gen[deg - 1] = 1;
    let mut root = 1u8;
    for _ in 0..deg {
        for j in 0..deg {
            gen[j] = gf_mul(gen[j], root);
            if j + 1 < deg {
                gen[j] ^= gen[j + 1];
            }
        }
        root = gf_mul(root, 2);
    }
    gen
}

pub fn rs_remainder(data: &[u8], gen: &[u8]) -> Vec<u8> {
    let deg = gen.len();
    let mut rem = vec![0u8; deg];
    for &b in data {
        let factor = b ^ rem[0];
        rem.copy_within(1.., 0);
        rem[deg - 1] = 0;
        for (idx, &coeff) in gen.iter().enumerate() {
            rem[idx] ^= gf_mul(coeff, factor);
        }
    }
    rem
}

fn total_raw_codewords(version: u8) -> usize {
    let mut modules = (16 * (version as usize) + 128) * (version as usize) + 64;
    if version >= 2 {
        let align_count = (version as usize) / 7 + 2;
        modules -= (25 * align_count - 10) * align_count - 55;
        if version >= 7 {
            modules -= 36;
        }
    }
    modules / 8
}

fn ecc_params(version: u8, ecc: QrEccLevel) -> (usize, usize, usize) {
    let idx = (version as usize) - 1;
    let (ec_per_blk, num_blks) = match ecc {
        QrEccLevel::L => ECC_TABLE_L[idx],
        QrEccLevel::M => ECC_TABLE_M[idx],
    };
    let total_cw = total_raw_codewords(version);
    let data_cw = total_cw - ec_per_blk * num_blks;
    (data_cw, ec_per_blk, num_blks)
}

fn pick_version(byte_len: usize, ecc: QrEccLevel) -> Result<u8, QrEncodeError> {
    for ver in 1u8..=15 {
        let (data_cw, _, _) = ecc_params(ver, ecc);
        let cci_bits = if ver < 10 { 8 } else { 16 };
        let needed_bits = 4 + cci_bits + byte_len * 8;
        if needed_bits <= data_cw * 8 {
            return Ok(ver);
        }
    }
    let (max_cw, _, _) = ecc_params(15, ecc);
    Err(QrEncodeError::PayloadTooLarge {
        byte_len,
        max_bytes: max_cw.saturating_sub(3),
    })
}

fn encode_data_and_ecc(payload: &[u8], version: u8, ecc: QrEccLevel) -> Vec<u8> {
    let (data_cw, ec_per_blk, num_blks) = ecc_params(version, ecc);
    let total_cw = total_raw_codewords(version);
    let capacity_bits = data_cw * 8;
    let mut bits = Vec::with_capacity(capacity_bits);
    let push_bits = |out: &mut Vec<bool>, val: usize, width: usize| {
        for shift in (0..width).rev() {
            out.push(((val >> shift) & 1) != 0);
        }
    };

    push_bits(&mut bits, 0b0100, 4);
    let cci_bits = if version < 10 { 8 } else { 16 };
    push_bits(&mut bits, payload.len(), cci_bits);
    for &b in payload {
        push_bits(&mut bits, b as usize, 8);
    }

    let terminator = (capacity_bits - bits.len()).min(4);
    push_bits(&mut bits, 0, terminator);
    let align = (8 - (bits.len() % 8)) % 8;
    push_bits(&mut bits, 0, align);

    let mut pad_byte = 0xECusize;
    while bits.len() < capacity_bits {
        push_bits(&mut bits, pad_byte, 8);
        pad_byte ^= 0xEC ^ 0x11;
    }

    let mut data_bytes = Vec::with_capacity(data_cw);
    for chunk in bits.chunks_exact(8) {
        let mut byte = 0u8;
        for (idx, &bit) in chunk.iter().enumerate() {
            if bit {
                byte |= 1 << (7 - idx);
            }
        }
        data_bytes.push(byte);
    }

    let short_blks = num_blks - (total_cw % num_blks);
    let short_len = total_cw / num_blks;
    let gen = rs_generator_poly(ec_per_blk);
    let mut blocks: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(num_blks);
    let mut offset = 0usize;

    for blk_idx in 0..num_blks {
        let data_len = short_len - ec_per_blk + if blk_idx < short_blks { 0 } else { 1 };
        let slice = &data_bytes[offset..offset + data_len];
        offset += data_len;
        let rem = rs_remainder(slice, &gen);
        blocks.push((slice.to_vec(), rem));
    }

    let max_data_len = blocks.iter().map(|(d, _)| d.len()).max().unwrap_or(0);
    let mut result = Vec::with_capacity(total_cw);
    for i in 0..max_data_len {
        for (d, _) in &blocks {
            if i < d.len() {
                result.push(d[i]);
            }
        }
    }
    for i in 0..ec_per_blk {
        for (_, e) in &blocks {
            result.push(e[i]);
        }
    }
    result
}

struct MatrixBuilder {
    size: usize,
    modules: Vec<bool>,
    is_function: Vec<bool>,
}

impl MatrixBuilder {
    fn new(version: u8) -> Self {
        let size = (version as usize) * 4 + 17;
        Self {
            size,
            modules: vec![false; size * size],
            is_function: vec![false; size * size],
        }
    }

    fn set_fn(&mut self, row: usize, col: usize, dark: bool) {
        let idx = row * self.size + col;
        self.modules[idx] = dark;
        self.is_function[idx] = true;
    }

    fn draw_function_patterns(&mut self, version: u8) {
        for i in 0..self.size {
            self.set_fn(i, 6, i % 2 == 0);
            self.set_fn(6, i, i % 2 == 0);
        }

        let corners = [
            (3isize, 3isize),
            (3isize, (self.size as isize) - 4),
            ((self.size as isize) - 4, 3isize),
        ];
        for (cr, cc) in corners {
            for dr in -4..=4 {
                for dc in -4..=4 {
                    let rr = cr + dr;
                    let cc2 = cc + dc;
                    if rr >= 0
                        && cc2 >= 0
                        && (rr as usize) < self.size
                        && (cc2 as usize) < self.size
                    {
                        let dist = dr.abs().max(dc.abs());
                        self.set_fn(rr as usize, cc2 as usize, dist != 2 && dist != 4);
                    }
                }
            }
        }

        let coords = ALIGNMENT_COORDS[(version as usize) - 1];
        for (i, &ar) in coords.iter().enumerate() {
            for (j, &ac) in coords.iter().enumerate() {
                let on_finder = (i == 0 && j == 0)
                    || (i == 0 && j == coords.len() - 1)
                    || (i == coords.len() - 1 && j == 0);
                if on_finder {
                    continue;
                }
                for dr in -2isize..=2 {
                    for dc in -2isize..=2 {
                        let dist = dr.abs().max(dc.abs());
                        self.set_fn(
                            (ar as isize + dr) as usize,
                            (ac as isize + dc) as usize,
                            dist != 1,
                        );
                    }
                }
            }
        }

        self.set_fn(self.size - 8, 8, true);
        self.draw_format_bits(QrEccLevel::M, 0);

        if version >= 7 {
            let mut rem = version as u32;
            for _ in 0..12 {
                rem = (rem << 1) ^ ((rem >> 11) * 0x1F25);
            }
            let ver_bits = ((version as u32) << 12) | rem;
            for i in 0..18 {
                let dark = ((ver_bits >> i) & 1) != 0;
                let a = self.size - 11 + (i % 3);
                let b = i / 3;
                self.set_fn(a, b, dark);
                self.set_fn(b, a, dark);
            }
        }
    }

    fn draw_format_bits(&mut self, ecc: QrEccLevel, mask: u8) {
        let data = (ecc.format_bits() << 3) | (mask as u16);
        let mut rem = data;
        for _ in 0..10 {
            rem = (rem << 1) ^ ((rem >> 9) * 0x537);
        }
        let fmt = ((data << 10) | rem) ^ 0x5412;
        let bit = |k: usize| ((fmt >> k) & 1) != 0;

        for i in 0..=5 {
            self.set_fn(i, 8, bit(i));
        }
        self.set_fn(7, 8, bit(6));
        self.set_fn(8, 8, bit(7));
        self.set_fn(8, 7, bit(8));
        for i in 9..15 {
            self.set_fn(8, 14 - i, bit(i));
        }
        for i in 0..8 {
            self.set_fn(8, self.size - 1 - i, bit(i));
        }
        for i in 8..15 {
            self.set_fn(self.size - 15 + i, 8, bit(i));
        }
    }

    fn place_codewords(&mut self, codewords: &[u8]) {
        let mut bit_idx = 0usize;
        let total_bits = codewords.len() * 8;
        let mut right = (self.size as isize) - 1;
        while right >= 1 {
            if right == 6 {
                right = 5;
            }
            for step in 0..self.size {
                let upward = ((right + 1) & 2) == 0;
                let row = if upward { self.size - 1 - step } else { step };
                for j in 0..2 {
                    let col = (right - j) as usize;
                    let idx = row * self.size + col;
                    if !self.is_function[idx] {
                        let dark = if bit_idx < total_bits {
                            let cw = codewords[bit_idx >> 3];
                            ((cw >> (7 - (bit_idx & 7))) & 1) != 0
                        } else {
                            false
                        };
                        self.modules[idx] = dark;
                        bit_idx += 1;
                    }
                }
            }
            right -= 2;
        }
    }

    fn toggle_mask(&mut self, mask: u8) {
        for r in 0..self.size {
            for c in 0..self.size {
                let idx = r * self.size + c;
                if self.is_function[idx] {
                    continue;
                }
                let invert = match mask {
                    0 => (r + c) % 2 == 0,
                    1 => r % 2 == 0,
                    2 => c % 3 == 0,
                    3 => (r + c) % 3 == 0,
                    4 => (r / 2 + c / 3) % 2 == 0,
                    5 => (r * c) % 2 + (r * c) % 3 == 0,
                    6 => ((r * c) % 2 + (r * c) % 3) % 2 == 0,
                    _ => ((r + c) % 2 + (r * c) % 3) % 2 == 0,
                };
                if invert {
                    self.modules[idx] = !self.modules[idx];
                }
            }
        }
    }

    fn penalty_score(&self) -> usize {
        let n = self.size;
        let mut score = 0usize;

        for vertical in [false, true] {
            for a in 0..n {
                let mut run_len = 1usize;
                for b in 1..n {
                    let cur = if vertical {
                        self.modules[b * n + a]
                    } else {
                        self.modules[a * n + b]
                    };
                    let prv = if vertical {
                        self.modules[(b - 1) * n + a]
                    } else {
                        self.modules[a * n + (b - 1)]
                    };
                    if cur == prv {
                        run_len += 1;
                        if run_len == 5 {
                            score += 3;
                        } else if run_len > 5 {
                            score += 1;
                        }
                    } else {
                        run_len = 1;
                    }
                }
            }
        }

        for r in 0..n - 1 {
            for c in 0..n - 1 {
                let v = self.modules[r * n + c];
                if v == self.modules[r * n + c + 1]
                    && v == self.modules[(r + 1) * n + c]
                    && v == self.modules[(r + 1) * n + c + 1]
                {
                    score += 3;
                }
            }
        }

        let pat_a = [true, false, true, true, true, false, true, false, false, false, false];
        let pat_b = [false, false, false, false, true, false, true, true, true, false, true];
        for r in 0..n {
            for c in 0..=n.saturating_sub(11) {
                let mut m_h_a = true;
                let mut m_h_b = true;
                let mut m_v_a = true;
                let mut m_v_b = true;
                for k in 0..11 {
                    let h = self.modules[r * n + (c + k)];
                    let v = self.modules[(c + k) * n + r];
                    if h != pat_a[k] {
                        m_h_a = false;
                    }
                    if h != pat_b[k] {
                        m_h_b = false;
                    }
                    if v != pat_a[k] {
                        m_v_a = false;
                    }
                    if v != pat_b[k] {
                        m_v_b = false;
                    }
                }
                if m_h_a || m_h_b {
                    score += 40;
                }
                if m_v_a || m_v_b {
                    score += 40;
                }
            }
        }

        let dark_count = self.modules.iter().filter(|&&m| m).count();
        let total = n * n;
        let k = ((dark_count * 100).abs_diff(total * 50)) / (5 * total);
        score + k * 10
    }
}

pub fn encode_qr_matrix(payload: &[u8], ecc: QrEccLevel) -> Result<QrMatrix, QrEncodeError> {
    let version = pick_version(payload.len(), ecc)?;
    let codewords = encode_data_and_ecc(payload, version, ecc);

    let mut builder = MatrixBuilder::new(version);
    builder.draw_function_patterns(version);
    builder.place_codewords(&codewords);

    let mut best_mask = 0u8;
    let mut best_score = usize::MAX;
    for mask in 0u8..8 {
        builder.toggle_mask(mask);
        builder.draw_format_bits(ecc, mask);
        let penalty = builder.penalty_score();
        builder.toggle_mask(mask);
        if penalty < best_score {
            best_score = penalty;
            best_mask = mask;
        }
    }

    builder.toggle_mask(best_mask);
    builder.draw_format_bits(ecc, best_mask);

    Ok(QrMatrix {
        version,
        size: builder.size,
        ecc,
        mask: best_mask,
        modules: builder.modules,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_finder_at(m: &QrMatrix, r0: usize, c0: usize) {
        for r in 0..7 {
            for c in 0..7 {
                let expected = r == 0
                    || r == 6
                    || c == 0
                    || c == 6
                    || ((2..=4).contains(&r) && (2..=4).contains(&c));
                assert_eq!(
                    m.get(r0 + r, c0 + c),
                    expected,
                    "finder mismatch at ({},{})",
                    r0 + r,
                    c0 + c
                );
            }
        }
    }

    fn verify_bch_15_5(word: u16) -> bool {
        let unmasked = word ^ 0x5412;
        let mut rem = unmasked;
        for shift in (10..15).rev() {
            if ((rem >> shift) & 1) != 0 {
                rem ^= 0x537 << (shift - 10);
            }
        }
        (rem & 0x3FF) == 0
    }

    #[test]
    fn test_6_3_iso_qr_encoder_generates_valid_matrix_finders_timing_and_rs_ecc() {
        let short_mat = encode_qr_matrix(b"miku-link://ok", QrEccLevel::M).expect("V1 QR");
        assert_eq!(short_mat.version, 1);
        assert_eq!(short_mat.size, 21);

        let long_uri = "miku-link://pair?v=1&host=miku-pc.tailnet-demo.ts.net&port=47400&nonce=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef&pin=sha256:fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210&exp=1794563152230";
        let m = encode_qr_matrix(long_uri.as_bytes(), QrEccLevel::M).expect("encode pairing URI");
        assert!(m.version >= 7, "long pairing URI should require >= V7 (got V{})", m.version);
        let n = m.size;
        assert_eq!(n, (m.version as usize) * 4 + 17);

        assert_finder_at(&m, 0, 0);
        assert_finder_at(&m, 0, n - 7);
        assert_finder_at(&m, n - 7, 0);

        for i in 0..8 {
            assert!(!m.get(7, i));
            assert!(!m.get(i, 7));
        }

        for i in 8..(n - 8) {
            assert_eq!(m.get(6, i), i % 2 == 0);
            assert_eq!(m.get(i, 6), i % 2 == 0);
        }
        assert!(m.get(n - 8, 8));

        let mut fmt_tl = 0u16;
        for i in 0..=5 {
            if m.get(i, 8) {
                fmt_tl |= 1 << i;
            }
        }
        if m.get(7, 8) { fmt_tl |= 1 << 6; }
        if m.get(8, 8) { fmt_tl |= 1 << 7; }
        if m.get(8, 7) { fmt_tl |= 1 << 8; }
        for i in 9..15 {
            if m.get(8, 14 - i) {
                fmt_tl |= 1 << i;
            }
        }
        assert!(verify_bch_15_5(fmt_tl), "format BCH syndrome non-zero");

        let sample = b"MIKU-RS-CHECK";
        let gen = rs_generator_poly(10);
        let rem = rs_remainder(sample, &gen);
        let mut codeword = sample.to_vec();
        codeword.extend_from_slice(&rem);
        assert!(rs_remainder(&codeword, &gen).iter().all(|&x| x == 0));

        let svg = m.to_svg(6, 4);
        assert!(svg.starts_with("<svg "));
        assert!(svg.contains("Miku-Link Pairing QR"));
        assert!(svg.contains("<path d=\"M"));
    }
}
