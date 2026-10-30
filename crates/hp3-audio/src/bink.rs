//! Bink audio: the `BIK` container (audio track 0 only) and its transform-coded audio.
//!
//! Written from the public descriptions of the format (the multimedia wiki and FFmpeg's documentation), not
//! from any engine source. An audio packet holds blocks, each a frame of spectrum coefficients (run-length
//! coded, one quantizer per band) turned into samples by an inverse DCT or an inverse real DFT; blocks are
//! cross-faded over their overlap. Bits are read least significant first.

use std::f64::consts::PI;

/// Decoded audio: interleaved 16-bit samples.
#[derive(Clone, Debug)]
pub struct Pcm {
    pub sample_rate: u32,
    pub channels: usize,
    pub samples: Vec<i16>,
}

impl Pcm {
    pub fn seconds(&self) -> f32 {
        self.samples.len() as f32 / (self.sample_rate as f32 * self.channels as f32)
    }
}

/// What the decoder saw, for checking decodes against each other.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub blocks: usize,
    /// Mean relative difference between a block's overlap and the previous block's tail there (0 = they agree).
    pub overlap_mismatch: f32,
    pub dct: bool,
    pub version_b: bool,
    /// Energy of the first difference over the energy (about 2 for noise, far less for real audio).
    pub roughness: f32,
}

const AUD_STEREO: u16 = 0x2000;
const AUD_USE_DCT: u16 = 0x1000;

/// Upper edges (Hz) of the critical bands that group coefficients under one quantizer.
const CRITICAL_FREQS: [u32; 25] = [
    100, 200, 300, 400, 510, 630, 770, 920, 1080, 1270, 1480, 1720, 2000, 2320, 2700, 3150, 3700, 4400, 5300, 6400, 7700, 9500,
    12000, 15500, 24500,
];

/// Run lengths (in groups of 8 coefficients) a run header can announce.
const RLE_LENGTHS: [usize; 16] = [2, 3, 4, 5, 6, 8, 9, 10, 11, 12, 13, 14, 15, 16, 32, 64];

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Bits<'a> {
        Bits { data, pos: 0 }
    }

    fn left(&self) -> isize {
        self.data.len() as isize * 8 - self.pos as isize
    }

    /// Up to 32 bits, the first bit read being the least significant.
    fn get(&mut self, n: u32) -> u32 {
        let mut v = 0u64;
        for i in 0..n as usize {
            let p = self.pos + i;
            let bit = self.data.get(p >> 3).map_or(0, |b| (b >> (p & 7)) & 1);
            v |= (bit as u64) << i;
        }
        self.pos += n as usize;
        v as u32
    }

    fn flag(&mut self) -> bool {
        self.get(1) != 0
    }

    fn align32(&mut self) {
        self.pos = (self.pos + 31) & !31;
    }

    /// A float in Bink's 5-bit exponent, 23-bit mantissa, sign format.
    fn float(&mut self) -> f32 {
        let power = self.get(5) as i32;
        let mantissa = self.get(23) as f32;
        let f = mantissa * 2f32.powi(power - 23);
        if self.flag() { -f } else { f }
    }
}

#[derive(Clone, Copy, Default)]
struct Complex {
    re: f64,
    im: f64,
}

/// In-place radix-2 FFT; `inverse` uses the positive exponent. Neither direction is normalized.
fn fft(data: &mut [Complex], inverse: bool) {
    let n = data.len();
    debug_assert!(n.is_power_of_two());
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            data.swap(i, j);
        }
    }
    let sign = if inverse { 1.0 } else { -1.0 };
    let mut len = 2;
    while len <= n {
        let angle = sign * 2.0 * PI / len as f64;
        let (wr, wi) = (angle.cos(), angle.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0, 0.0);
            for k in 0..len / 2 {
                let a = data[start + k];
                let b = data[start + k + len / 2];
                let t = Complex { re: b.re * cr - b.im * ci, im: b.re * ci + b.im * cr };
                data[start + k] = Complex { re: a.re + t.re, im: a.im + t.im };
                data[start + k + len / 2] = Complex { re: a.re - t.re, im: a.im - t.im };
                let next = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = next;
            }
        }
        len <<= 1;
    }
}

/// DCT-III: `y[n] = x[0] + sum_{k>=1} x[k] cos(pi k (2n + 1) / 2N)`.
fn dct3(x: &[f32]) -> Vec<f32> {
    let n = x.len();
    let mut z = vec![Complex::default(); 2 * n];
    for (k, &v) in x.iter().enumerate() {
        let a = PI * k as f64 / (2 * n) as f64;
        z[k] = Complex { re: v as f64 * a.cos(), im: v as f64 * a.sin() };
    }
    fft(&mut z, true);
    z[..n].iter().map(|c| c.re as f32).collect()
}

/// DCT-IV, `y[n] = sum_k x[k] cos(pi (2n + 1)(2k + 1) / 4N)`: only for comparing against `dct3` while calibrating.
fn dct4(x: &[f32]) -> Vec<f32> {
    let n = x.len();
    (0..n)
        .map(|i| {
            x.iter()
                .enumerate()
                .map(|(k, &v)| v as f64 * (PI * (2 * i + 1) as f64 * (2 * k + 1) as f64 / (4 * n) as f64).cos())
                .sum::<f64>() as f32
        })
        .collect()
}

/// Inverse real DFT of a packed spectrum (`[DC, Nyquist, re1, im1, re2, im2, ...]`), halved.
fn irdft(x: &[f32], im_sign: f64) -> Vec<f32> {
    let n = x.len();
    let mut z = vec![Complex::default(); n];
    z[0] = Complex { re: x[0] as f64, im: 0.0 };
    z[n / 2] = Complex { re: x[1] as f64, im: 0.0 };
    for k in 1..n / 2 {
        let (re, im) = (x[2 * k] as f64, im_sign * x[2 * k + 1] as f64);
        z[k] = Complex { re, im };
        z[n - k] = Complex { re, im: -im };
    }
    fft(&mut z, true);
    z.iter().map(|c| (c.re * 0.5) as f32).collect()
}

struct Decoder {
    channels: usize,
    frame_len: usize,
    overlap_len: usize,
    bands: Vec<usize>,
    quant_table: [f32; 96],
    root: f32,
    dct: bool,
    version_b: bool,
    im_sign: f64,
    previous: Vec<Vec<f32>>,
    first: bool,
    stats: Stats,
    mismatch_sum: f64,
    mismatch_n: usize,
}

impl Decoder {
    fn new(sample_rate: u32, file_channels: usize, dct: bool, version_b: bool) -> Decoder {
        let mut frame_len_bits = if sample_rate < 22050 {
            9
        } else if sample_rate < 44100 {
            10
        } else {
            11
        };
        let mut rate = sample_rate;
        let channels = if dct {
            file_channels
        } else {
            // The inverse DFT codec codes the interleaved channels as one longer signal.
            rate *= file_channels as u32;
            frame_len_bits += file_channels.ilog2();
            1
        };
        let frame_len = 1usize << frame_len_bits;
        let overlap_len = frame_len / 16;
        let rate_half = rate.div_ceil(2);
        let root = if dct {
            frame_len as f32 / ((frame_len as f32).sqrt() * 32768.0)
        } else {
            2.0 / ((frame_len as f32).sqrt() * 32768.0)
        };
        let mut quant_table = [0.0; 96];
        for (i, q) in quant_table.iter_mut().enumerate() {
            *q = (i as f32 * 0.152_891_65).exp() * root;
        }
        let mut num_bands = 1;
        while num_bands < 25 && rate_half > CRITICAL_FREQS[num_bands - 1] {
            num_bands += 1;
        }
        let mut bands = vec![0usize; num_bands + 1];
        bands[0] = 2;
        for i in 1..num_bands {
            bands[i] = (CRITICAL_FREQS[i - 1] as usize * frame_len / rate_half as usize) & !1;
        }
        bands[num_bands] = frame_len;
        Decoder {
            channels,
            frame_len,
            overlap_len,
            bands,
            quant_table,
            root,
            dct,
            version_b,
            // The sign of the spectrum's imaginary parts (a convention of the format).
            im_sign: std::env::var("HP3_BINK_IM_SIGN").ok().and_then(|v| v.parse().ok()).unwrap_or(-1.0),
            previous: vec![vec![0.0; overlap_len]; channels],
            first: true,
            stats: Stats { dct, version_b, ..Default::default() },
            mismatch_sum: 0.0,
            mismatch_n: 0,
        }
    }

    /// Decodes one block; returns its samples per channel (`frame_len - overlap_len` each).
    fn block(&mut self, br: &mut Bits<'_>) -> Option<Vec<Vec<f32>>> {
        if self.dct {
            br.get(2);
        }
        let n = self.frame_len;
        let mut out = Vec::with_capacity(self.channels);
        for _ in 0..self.channels {
            let mut coeffs = vec![0f32; n];
            let need = if self.version_b { 64 } else { 58 };
            if br.left() < need {
                return None;
            }
            if self.version_b {
                coeffs[0] = f32::from_bits(br.get(32)) * self.root;
                coeffs[1] = f32::from_bits(br.get(32)) * self.root;
            } else {
                coeffs[0] = br.float() * self.root;
                coeffs[1] = br.float() * self.root;
            }
            let num_bands = self.bands.len() - 1;
            if br.left() < num_bands as isize * 8 {
                return None;
            }
            let quant: Vec<f32> = (0..num_bands).map(|_| self.quant_table[br.get(8).min(95) as usize]).collect();
            let (mut q, mut k) = (0f32, 0usize);
            let mut i = 2;
            while i < n {
                let mut j = if br.flag() { i + RLE_LENGTHS[br.get(4) as usize] * 8 } else { i + 8 };
                j = j.min(n);
                let width = br.get(4);
                if width == 0 {
                    // coeffs are already zero.
                    i = j;
                    while k < num_bands && self.bands[k] < i {
                        q = quant[k];
                        k += 1;
                    }
                } else {
                    while i < j {
                        if k < num_bands && self.bands[k] == i {
                            q = quant[k];
                            k += 1;
                        }
                        let coeff = br.get(width);
                        if coeff != 0 {
                            let v = q * coeff as f32;
                            coeffs[i] = if br.flag() { -v } else { v };
                        }
                        i += 1;
                    }
                }
                if br.left() < 0 {
                    return None;
                }
            }
            let samples = if self.dct {
                coeffs[0] *= std::env::var("HP3_BINK_DC").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(2.0);
                if std::env::var_os("HP3_BINK_DCT4").is_some() { dct4(&coeffs) } else { dct3(&coeffs) }
            } else {
                irdft(&coeffs, self.im_sign)
            };
            out.push(samples);
        }
        // Cross-fade the overlap with the previous block's tail.
        let count = (self.overlap_len * self.channels) as f32;
        for ch in 0..self.channels {
            if !self.first {
                let (mut diff, mut size) = (0f64, 0f64);
                for i in 0..self.overlap_len {
                    let j = (ch + i * self.channels) as f32;
                    let (p, c) = (self.previous[ch][i], out[ch][i]);
                    diff += (p - c).abs() as f64;
                    size += (p.abs() + c.abs()) as f64;
                    out[ch][i] = (p * (count - j) + c * j) / count;
                }
                if size > 1e-9 {
                    self.mismatch_sum += diff / size;
                    self.mismatch_n += 1;
                }
            }
            self.previous[ch].copy_from_slice(&out[ch][n - self.overlap_len..]);
            out[ch].truncate(n - self.overlap_len);
        }
        self.first = false;
        self.stats.blocks += 1;
        Some(out)
    }
}

fn u16_at(d: &[u8], at: usize) -> Option<u16> {
    d.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    d.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Decodes the first audio track of a Bink file.
pub fn decode(file: &[u8]) -> Result<Pcm, String> {
    decode_with_stats(file).map(|(p, _)| p)
}

pub fn decode_with_stats(file: &[u8]) -> Result<(Pcm, Stats), String> {
    if file.len() < 44 || &file[..3] != b"BIK" {
        return Err("not a Bink file".into());
    }
    let revision = file[3];
    let frames = u32_at(file, 8).ok_or("short header")? as usize;
    let tracks = u32_at(file, 40).ok_or("short header")? as usize;
    if tracks == 0 {
        return Err("no audio track".into());
    }
    let mut p = 44 + 4 * tracks;
    let rate = u16_at(file, p).ok_or("short header")? as u32;
    let flags = u16_at(file, p + 2).ok_or("short header")?;
    p += 4 * tracks; // rate and flags of each track
    p += 4 * tracks; // track ids
    let offsets: Vec<usize> = (0..=frames).map(|i| u32_at(file, p + 4 * i).map(|v| (v & !1) as usize)).collect::<Option<_>>().ok_or("short frame table")?;
    let channels = if flags & AUD_STEREO != 0 { 2 } else { 1 };
    let mut dec = Decoder::new(rate, channels, flags & AUD_USE_DCT != 0, revision == b'b');
    let mut pcm: Vec<i16> = Vec::new();
    // Debug aid for calibrating the output level.
    let gain: f32 = std::env::var("HP3_BINK_GAIN").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0 / dec.frame_len as f32);
    for f in 0..frames {
        let (start, end) = (offsets[f], offsets[f + 1].min(file.len()));
        let size = u32_at(file, start).ok_or("short frame")? as usize;
        if size < 4 || start + 4 + size > end.max(start + 4 + size).min(file.len()) {
            continue;
        }
        let data = &file[start + 8..start + 4 + size];
        let mut br = Bits::new(data);
        while br.left() >= 32 {
            let Some(block) = dec.block(&mut br) else { break };
            br.align32();
            let len = block[0].len();
            for i in 0..len {
                for ch in block.iter().take(dec.channels) {
                    pcm.push((ch[i] * gain * 32768.0).round().clamp(-32768.0, 32767.0) as i16);
                }
            }
        }
    }
    let mut stats = dec.stats.clone();
    let (mut e, mut de) = (0f64, 0f64);
    for w in pcm.windows(2) {
        e += (w[0] as f64).powi(2);
        de += ((w[1] - w[0]) as f64).powi(2);
    }
    stats.roughness = if e > 0.0 { (de / e) as f32 } else { 0.0 };
    if dec.mismatch_n > 0 {
        stats.overlap_mismatch = (dec.mismatch_sum / dec.mismatch_n as f64) as f32;
    }
    // The inverse DFT codec's single signal already holds the interleaved channels.
    Ok((Pcm { sample_rate: rate, channels, samples: pcm }, stats))
}

/// Length in seconds of a Bink sound, from its headers alone (no decoding).
pub fn duration(file: &[u8]) -> Option<f32> {
    if file.len() < 44 || &file[..3] != b"BIK" {
        return None;
    }
    let frames = u32_at(file, 8)? as usize;
    let tracks = u32_at(file, 40)? as usize;
    if tracks == 0 {
        return None;
    }
    let mut p = 44 + 4 * tracks;
    let rate = u16_at(file, p)? as f32;
    let flags = u16_at(file, p + 2)?;
    p += 8 * tracks;
    let channels = if flags & AUD_STEREO != 0 { 2.0 } else { 1.0 };
    let mut bytes = 0u64;
    for f in 0..frames {
        let start = (u32_at(file, p + 4 * f)? & !1) as usize;
        if u32_at(file, start)? >= 4 {
            bytes += u32_at(file, start + 4)? as u64;
        }
    }
    Some(bytes as f32 / (rate * channels * 2.0))
}
