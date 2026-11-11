//! Bink video playback: the `BIK` container and the Bink 1 video codec (revisions `f` to `k`; HP3's files are `i`).
//!
//! Written from the public descriptions of the format (the multimedia wiki, and FFmpeg's LGPL Bink decoder as the permitted
//! reference for the codec's algorithm and constants). A frame is three planes (luma, then the two half-size chroma planes) of 8x8
//! blocks; every block is one of ten kinds (skip, scaled 16x16, motion copy from the previous frame, runs of colours, residue,
//! intra DCT, fill, inter DCT, two-colour pattern, raw). The values the blocks need come from nine "bundles" that are filled a block
//! row at a time from the bit stream. Bits are read least significant first.
//!
//! Audio tracks are skipped here (HP3's skin videos have none).

mod tables;

use tables::{BINK_INTER_QUANT, BINK_INTRA_QUANT, BINK_PATTERNS, BINK_SCAN, BINK_TREE_BITS, BINK_TREE_LENS};

const FLAG_ALPHA: u32 = 0x0010_0000;
/// A black-and-white video: it has no chroma planes (the picture's colour is neutral, whatever the chroma buffers hold).
const FLAG_GRAY: u32 = 0x0002_0000;

/// A decoding failure: the frame is left as far as it got.
#[derive(Debug, Clone)]
pub struct Error(pub String);

type Result<T> = std::result::Result<T, Error>;

fn err<T>(what: &str) -> Result<T> {
    Err(Error(what.to_string()))
}

/// Bits are read least significant first.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    fn left(&self) -> isize {
        self.data.len() as isize * 8 - self.pos as isize
    }

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

    fn skip(&mut self, n: usize) {
        self.pos += n;
    }

    /// The next plane's data starts on a 32-bit boundary.
    fn align32(&mut self) {
        if self.pos & 31 != 0 {
            self.pos += 32 - (self.pos & 31);
        }
    }
}

/// Number of bits of `x` (`floor(log2(x)) + 1`).
fn bit_len(x: usize) -> u32 {
    usize::BITS - x.leading_zeros()
}

#[derive(Clone, Copy, Default)]
struct Tree {
    vlc: usize,
    syms: [u8; 16],
}

/// One symbol of a 4-bit Huffman tree: codes are read a bit at a time, first bit least significant.
fn read_huff(bits: &mut Bits<'_>, tree: &Tree) -> u8 {
    let (lens, codes) = (&BINK_TREE_LENS[tree.vlc], &BINK_TREE_BITS[tree.vlc]);
    let mut value = 0u32;
    for n in 1..=16u32 {
        value |= bits.get(1) << (n - 1);
        for s in 0..16 {
            if lens[s] as u32 == n && codes[s] as u32 == value {
                return tree.syms[s];
            }
        }
    }
    tree.syms[0]
}

fn merge(bits: &mut Bits<'_>, dst: &mut [u8], src: &[u8], size: usize) {
    let (mut a, mut b) = (0usize, size);
    let (mut left_a, mut left_b) = (size, size);
    let mut out = 0;
    loop {
        if !bits.flag() {
            dst[out] = src[a];
            a += 1;
            left_a -= 1;
        } else {
            dst[out] = src[b];
            b += 1;
            left_b -= 1;
        }
        out += 1;
        if left_a == 0 || left_b == 0 {
            break;
        }
    }
    while left_a > 0 {
        dst[out] = src[a];
        out += 1;
        a += 1;
        left_a -= 1;
    }
    while left_b > 0 {
        dst[out] = src[b];
        out += 1;
        b += 1;
        left_b -= 1;
    }
}

fn read_tree(bits: &mut Bits<'_>) -> Result<Tree> {
    if bits.left() < 4 {
        return err("tree out of data");
    }
    let mut tree = Tree { vlc: bits.get(4) as usize, syms: [0; 16] };
    if tree.vlc == 0 {
        for i in 0..16 {
            tree.syms[i] = i as u8;
        }
        return Ok(tree);
    }
    if bits.flag() {
        let mut used = [false; 16];
        let mut len = bits.get(3) as usize;
        for i in 0..=len {
            tree.syms[i] = bits.get(4) as u8;
            used[tree.syms[i] as usize] = true;
        }
        let mut i = 0;
        while i < 16 && len < 15 {
            if !used[i] {
                len += 1;
                tree.syms[len] = i as u8;
            }
            i += 1;
        }
    } else {
        let len = bits.get(2) as usize;
        let mut a = [0u8; 16];
        let mut b = [0u8; 16];
        for i in 0..16 {
            a[i] = i as u8;
        }
        let (mut input, mut output) = (&mut a, &mut b);
        for i in 0..=len {
            let size = 1usize << i;
            let mut t = 0;
            while t < 16 {
                let (src, dst) = (*input, &mut output[t..]);
                merge(bits, dst, &src[t..], size);
                t += size << 1;
            }
            std::mem::swap(&mut input, &mut output);
        }
        tree.syms = *input;
    }
    Ok(tree)
}

const SRC_BLOCK_TYPES: usize = 0;
const SRC_SUB_BLOCK_TYPES: usize = 1;
const SRC_COLORS: usize = 2;
const SRC_PATTERN: usize = 3;
const SRC_X_OFF: usize = 4;
const SRC_Y_OFF: usize = 5;
const SRC_INTRA_DC: usize = 6;
const SRC_INTER_DC: usize = 7;
const SRC_RUN: usize = 8;
const NB_SRC: usize = 9;

/// The values of one kind, decoded a block row ahead of their use.
#[derive(Default)]
struct Bundle {
    len: u32,
    tree: Tree,
    bytes: Vec<u8>,
    words: Vec<i16>,
    /// Where decoding goes on (None: nothing more this plane).
    cur_dec: Option<usize>,
    /// The next value to hand out.
    cur_ptr: usize,
}

// Block types.
const SKIP_BLOCK: u8 = 0;
const SCALED_BLOCK: u8 = 1;
const MOTION_BLOCK: u8 = 2;
const RUN_BLOCK: u8 = 3;
const RESIDUE_BLOCK: u8 = 4;
const INTRA_BLOCK: u8 = 5;
const FILL_BLOCK: u8 = 6;
const INTER_BLOCK: u8 = 7;
const PATTERN_BLOCK: u8 = 8;
const RAW_BLOCK: u8 = 9;

const RLE_LENS: [usize; 4] = [4, 8, 12, 32];

struct Plane {
    stride: usize,
    rows: usize,
    pixels: Vec<u8>,
}

impl Plane {
    fn new(blocks_w: usize, blocks_h: usize) -> Plane {
        Plane { stride: blocks_w * 8, rows: blocks_h * 8, pixels: vec![0; blocks_w * 8 * blocks_h * 8] }
    }
}

struct Decoder {
    version: u8,
    has_alpha: bool,
    swap_planes: bool,
    width: usize,
    height: usize,
    bundles: Vec<Bundle>,
    col_high: [Tree; 16],
    col_last: usize,
    cur: Vec<Plane>,
    last: Vec<Plane>,
}

impl Decoder {
    fn new(version: u8, flags: u32, width: usize, height: usize) -> Decoder {
        let (bw, bh) = ((width + 7) >> 3, (height + 7) >> 3);
        let (cw, ch) = ((width + 15) >> 4, (height + 15) >> 4);
        let mk = || vec![Plane::new(bw, bh), Plane::new(cw, ch), Plane::new(cw, ch), Plane::new(bw, bh)];
        let blocks = bw * bh;
        Decoder {
            version,
            has_alpha: flags & FLAG_ALPHA != 0,
            swap_planes: version >= b'h',
            width,
            height,
            bundles: (0..NB_SRC).map(|_| Bundle { bytes: vec![0; blocks * 64], words: vec![0; blocks * 64], ..Default::default() }).collect(),
            col_high: [Tree::default(); 16],
            col_last: 0,
            cur: mk(),
            last: mk(),
        }
    }

    fn init_lengths(&mut self, width: usize, bw: usize) {
        let width = (width + 7) & !7;
        let b = &mut self.bundles;
        b[SRC_BLOCK_TYPES].len = bit_len((width >> 3) + 511);
        b[SRC_SUB_BLOCK_TYPES].len = bit_len((width >> 4) + 511);
        b[SRC_COLORS].len = bit_len(bw * 64 + 511);
        let l = bit_len((width >> 3) + 511);
        b[SRC_INTRA_DC].len = l;
        b[SRC_INTER_DC].len = l;
        b[SRC_X_OFF].len = l;
        b[SRC_Y_OFF].len = l;
        b[SRC_PATTERN].len = bit_len((bw << 3) + 511);
        b[SRC_RUN].len = bit_len(bw * 48 + 511);
    }

    fn read_bundle(&mut self, bits: &mut Bits<'_>, n: usize) -> Result<()> {
        if n == SRC_COLORS {
            for i in 0..16 {
                self.col_high[i] = read_tree(bits)?;
            }
            self.col_last = 0;
        }
        if n != SRC_INTRA_DC && n != SRC_INTER_DC {
            self.bundles[n].tree = read_tree(bits)?;
        }
        self.bundles[n].cur_dec = Some(0);
        self.bundles[n].cur_ptr = 0;
        Ok(())
    }

    /// The count of values a bundle is asked for now (None: nothing to decode).
    fn check_read(&mut self, bits: &mut Bits<'_>, n: usize) -> Option<usize> {
        let b = &mut self.bundles[n];
        match b.cur_dec {
            Some(d) if d <= b.cur_ptr => {}
            _ => return None,
        }
        let t = bits.get(b.len) as usize;
        if t == 0 {
            b.cur_dec = None;
            return None;
        }
        Some(t)
    }

    fn read_runs(&mut self, bits: &mut Bits<'_>) -> Result<()> {
        let Some(t) = self.check_read(bits, SRC_RUN) else { return Ok(()) };
        let b = &mut self.bundles[SRC_RUN];
        let start = b.cur_dec.unwrap();
        if start + t > b.bytes.len() {
            return err("run value out of bounds");
        }
        if bits.left() < 1 {
            return err("out of data");
        }
        if bits.flag() {
            let v = bits.get(4) as u8;
            b.bytes[start..start + t].fill(v);
        } else {
            for i in 0..t {
                b.bytes[start + i] = read_huff(bits, &b.tree);
            }
        }
        b.cur_dec = Some(start + t);
        Ok(())
    }

    fn read_motion_values(&mut self, bits: &mut Bits<'_>, n: usize) -> Result<()> {
        let Some(t) = self.check_read(bits, n) else { return Ok(()) };
        let b = &mut self.bundles[n];
        let start = b.cur_dec.unwrap();
        if start + t > b.bytes.len() {
            return err("too many motion values");
        }
        if bits.left() < 1 {
            return err("out of data");
        }
        if bits.flag() {
            let mut v = bits.get(4) as i32;
            if v != 0 {
                let sign = -(bits.get(1) as i32);
                v = (v ^ sign) - sign;
            }
            b.bytes[start..start + t].fill(v as i8 as u8);
        } else {
            for i in 0..t {
                let mut v = read_huff(bits, &b.tree) as i32;
                if v != 0 {
                    let sign = -(bits.get(1) as i32);
                    v = (v ^ sign) - sign;
                }
                b.bytes[start + i] = v as i8 as u8;
            }
        }
        b.cur_dec = Some(start + t);
        Ok(())
    }

    fn read_block_types(&mut self, bits: &mut Bits<'_>, n: usize) -> Result<()> {
        let Some(t) = self.check_read(bits, n) else { return Ok(()) };
        let version = self.version;
        let b = &mut self.bundles[n];
        let mut t = t;
        if version == b'k' {
            t ^= 0xBB;
            if t == 0 {
                b.cur_dec = None;
                return Ok(());
            }
        }
        let start = b.cur_dec.unwrap();
        let end = start + t;
        if end > b.bytes.len() {
            return err("too many block type values");
        }
        if bits.left() < 1 {
            return err("out of data");
        }
        let mut at = start;
        if bits.flag() {
            let v = bits.get(4) as u8;
            b.bytes[at..end].fill(v);
            at = end;
        } else {
            let mut last = 0u8;
            while at < end {
                let v = read_huff(bits, &b.tree);
                if v < 12 {
                    last = v;
                    b.bytes[at] = v;
                    at += 1;
                } else {
                    let run = RLE_LENS[(v - 12) as usize];
                    if end - at < run {
                        return err("block type run out of bounds");
                    }
                    b.bytes[at..at + run].fill(last);
                    at += run;
                }
            }
        }
        b.cur_dec = Some(at);
        Ok(())
    }

    fn read_patterns(&mut self, bits: &mut Bits<'_>) -> Result<()> {
        let Some(t) = self.check_read(bits, SRC_PATTERN) else { return Ok(()) };
        let b = &mut self.bundles[SRC_PATTERN];
        let start = b.cur_dec.unwrap();
        if start + t > b.bytes.len() {
            return err("too many pattern values");
        }
        for i in 0..t {
            if bits.left() < 2 {
                return err("out of data");
            }
            let mut v = read_huff(bits, &b.tree);
            v |= read_huff(bits, &b.tree) << 4;
            b.bytes[start + i] = v;
        }
        b.cur_dec = Some(start + t);
        Ok(())
    }

    fn read_colors(&mut self, bits: &mut Bits<'_>) -> Result<()> {
        let Some(t) = self.check_read(bits, SRC_COLORS) else { return Ok(()) };
        let version = self.version;
        let start = self.bundles[SRC_COLORS].cur_dec.unwrap();
        if start + t > self.bundles[SRC_COLORS].bytes.len() {
            return err("too many color values");
        }
        if bits.left() < 1 {
            return err("out of data");
        }
        let mut one = |this: &mut Decoder, bits: &mut Bits<'_>| -> u8 {
            this.col_last = read_huff(bits, &this.col_high[this.col_last]) as usize;
            let v = read_huff(bits, &this.bundles[SRC_COLORS].tree) as i32;
            let mut v = ((this.col_last as i32) << 4) | v;
            if version < b'i' {
                let sign = ((v as i8) >> 7) as i32;
                v = ((v & 0x7F) ^ sign) - sign;
                v += 0x80;
            }
            v as u8
        };
        if bits.flag() {
            let v = one(self, bits);
            self.bundles[SRC_COLORS].bytes[start..start + t].fill(v);
        } else {
            for i in 0..t {
                if bits.left() < 2 {
                    return err("out of data");
                }
                let v = one(self, bits);
                self.bundles[SRC_COLORS].bytes[start + i] = v;
            }
        }
        self.bundles[SRC_COLORS].cur_dec = Some(start + t);
        Ok(())
    }

    fn read_dcs(&mut self, bits: &mut Bits<'_>, n: usize, start_bits: u32, has_sign: bool) -> Result<()> {
        let Some(len) = self.check_read(bits, n) else { return Ok(()) };
        let b = &mut self.bundles[n];
        let mut at = b.cur_dec.unwrap();
        let sb = start_bits - has_sign as u32;
        if bits.left() < sb as isize {
            return err("out of data");
        }
        let mut v = bits.get(sb) as i32;
        if v != 0 && has_sign {
            let sign = -(bits.get(1) as i32);
            v = (v ^ sign) - sign;
        }
        if at + len > b.words.len() {
            return err("too many dc values");
        }
        b.words[at] = v as i16;
        at += 1;
        let len = len - 1;
        let mut i = 0;
        while i < len {
            let len2 = (len - i).min(8);
            let bsize = bits.get(4);
            if bsize != 0 {
                for _ in 0..len2 {
                    let mut v2 = bits.get(bsize) as i32;
                    if v2 != 0 {
                        let sign = -(bits.get(1) as i32);
                        v2 = (v2 ^ sign) - sign;
                    }
                    v += v2;
                    b.words[at] = v as i16;
                    at += 1;
                    if !(-32768..=32767).contains(&v) {
                        return err("dc value out of bounds");
                    }
                }
            } else {
                for _ in 0..len2 {
                    b.words[at] = v as i16;
                    at += 1;
                }
            }
            i += 8;
        }
        b.cur_dec = Some(at);
        Ok(())
    }

    fn value(&mut self, n: usize) -> i32 {
        let b = &mut self.bundles[n];
        let p = b.cur_ptr;
        b.cur_ptr += 1;
        match n {
            SRC_X_OFF | SRC_Y_OFF => b.bytes.get(p).copied().unwrap_or(0) as i8 as i32,
            SRC_INTRA_DC | SRC_INTER_DC => b.words.get(p).copied().unwrap_or(0) as i32,
            _ => b.bytes.get(p).copied().unwrap_or(0) as i32,
        }
    }

    /// Reads the coefficients of one DCT block into `block` (in scan order positions); returns the quantiser index and the count.
    fn read_dct_coeffs(&mut self, bits: &mut Bits<'_>, block: &mut [i32; 64], coef_idx: &mut [usize; 64]) -> Result<(usize, usize)> {
        let mut coef_list = [0i32; 128];
        let mut mode_list = [0i32; 128];
        let (mut list_start, mut list_end) = (64usize, 64usize);
        let mut coef_count = 0usize;
        if bits.left() < 4 {
            return err("out of data");
        }
        for (c, m) in [(4, 0), (24, 0), (44, 0), (1, 3), (2, 3), (3, 3)] {
            coef_list[list_end] = c;
            mode_list[list_end] = m;
            list_end += 1;
        }
        let mut bit = bits.get(4) as i32 - 1;
        while bit >= 0 {
            let mut list_pos = list_start;
            while list_pos < list_end {
                if (mode_list[list_pos] | coef_list[list_pos]) == 0 || !bits.flag() {
                    list_pos += 1;
                    continue;
                }
                let mut ccoef = coef_list[list_pos];
                let mode = mode_list[list_pos];
                match mode {
                    0 | 2 => {
                        if mode == 0 {
                            coef_list[list_pos] = ccoef + 4;
                            mode_list[list_pos] = 1;
                        } else {
                            coef_list[list_pos] = 0;
                            mode_list[list_pos] = 0;
                            list_pos += 1;
                        }
                        for _ in 0..4 {
                            if bits.flag() {
                                list_start -= 1;
                                coef_list[list_start] = ccoef;
                                mode_list[list_start] = 3;
                            } else {
                                let t = if bit == 0 {
                                    1 - ((bits.get(1) as i32) << 1)
                                } else {
                                    let t = (bits.get(bit as u32) | (1u32 << bit)) as i32;
                                    let sign = -(bits.get(1) as i32);
                                    (t ^ sign) - sign
                                };
                                block[BINK_SCAN[ccoef as usize] as usize] = t;
                                coef_idx[coef_count] = ccoef as usize;
                                coef_count += 1;
                            }
                            ccoef += 1;
                        }
                    }
                    1 => {
                        mode_list[list_pos] = 2;
                        for _ in 0..3 {
                            ccoef += 4;
                            coef_list[list_end] = ccoef;
                            mode_list[list_end] = 2;
                            list_end += 1;
                        }
                    }
                    _ => {
                        let t = if bit == 0 {
                            1 - ((bits.get(1) as i32) << 1)
                        } else {
                            let t = (bits.get(bit as u32) | (1u32 << bit)) as i32;
                            let sign = -(bits.get(1) as i32);
                            (t ^ sign) - sign
                        };
                        block[BINK_SCAN[ccoef as usize] as usize] = t;
                        coef_idx[coef_count] = ccoef as usize;
                        coef_count += 1;
                        coef_list[list_pos] = 0;
                        mode_list[list_pos] = 0;
                        list_pos += 1;
                    }
                }
            }
            bit -= 1;
        }
        let quant_idx = bits.get(4) as usize;
        Ok((quant_idx, coef_count))
    }

    /// Reads the residue of an 8x8 block after motion compensation.
    fn read_residue(bits: &mut Bits<'_>, block: &mut [i16; 64], mut masks_count: i32) {
        let mut coef_list = [0i32; 128];
        let mut mode_list = [0i32; 128];
        let (mut list_start, mut list_end) = (64usize, 64usize);
        let mut nz = [0usize; 64];
        let mut nz_count = 0usize;
        for (c, m) in [(4, 0), (24, 0), (44, 0), (0, 2)] {
            coef_list[list_end] = c;
            mode_list[list_end] = m;
            list_end += 1;
        }
        let mut mask = 1i32 << bits.get(3);
        while mask != 0 {
            for i in 0..nz_count {
                if !bits.flag() {
                    continue;
                }
                let p = nz[i];
                if block[p] < 0 {
                    block[p] -= mask as i16;
                } else {
                    block[p] += mask as i16;
                }
                masks_count -= 1;
                if masks_count < 0 {
                    return;
                }
            }
            let mut list_pos = list_start;
            while list_pos < list_end {
                if (coef_list[list_pos] | mode_list[list_pos]) == 0 || !bits.flag() {
                    list_pos += 1;
                    continue;
                }
                let mut ccoef = coef_list[list_pos];
                let mode = mode_list[list_pos];
                match mode {
                    0 | 2 => {
                        if mode == 0 {
                            coef_list[list_pos] = ccoef + 4;
                            mode_list[list_pos] = 1;
                        } else {
                            coef_list[list_pos] = 0;
                            mode_list[list_pos] = 0;
                            list_pos += 1;
                        }
                        for _ in 0..4 {
                            if bits.flag() {
                                list_start -= 1;
                                coef_list[list_start] = ccoef;
                                mode_list[list_start] = 3;
                            } else {
                                let p = BINK_SCAN[ccoef as usize] as usize;
                                nz[nz_count] = p;
                                nz_count += 1;
                                let sign = -(bits.get(1) as i32);
                                block[p] = ((mask ^ sign) - sign) as i16;
                                masks_count -= 1;
                                if masks_count < 0 {
                                    return;
                                }
                            }
                            ccoef += 1;
                        }
                    }
                    1 => {
                        mode_list[list_pos] = 2;
                        for _ in 0..3 {
                            ccoef += 4;
                            coef_list[list_end] = ccoef;
                            mode_list[list_end] = 2;
                            list_end += 1;
                        }
                    }
                    _ => {
                        let p = BINK_SCAN[ccoef as usize] as usize;
                        nz[nz_count] = p;
                        nz_count += 1;
                        let sign = -(bits.get(1) as i32);
                        block[p] = ((mask ^ sign) - sign) as i16;
                        coef_list[list_pos] = 0;
                        mode_list[list_pos] = 0;
                        list_pos += 1;
                        masks_count -= 1;
                        if masks_count < 0 {
                            return;
                        }
                    }
                }
            }
            mask >>= 1;
        }
    }

    /// Reads a run-coded block (a pattern scan with runs of one colour or single colours) into `put`.
    fn read_run_block(&mut self, bits: &mut Bits<'_>, put: &mut dyn FnMut(usize, u8)) -> Result<()> {
        let scan = &BINK_PATTERNS[bits.get(4) as usize];
        let mut i = 0usize;
        let mut s = 0usize;
        loop {
            let run = self.value(SRC_RUN) as usize + 1;
            i += run;
            if i > 64 {
                return err("run out of bounds");
            }
            if bits.flag() {
                let v = self.value(SRC_COLORS) as u8;
                for _ in 0..run {
                    put(scan[s] as usize, v);
                    s += 1;
                }
            } else {
                for _ in 0..run {
                    let v = self.value(SRC_COLORS) as u8;
                    put(scan[s] as usize, v);
                    s += 1;
                }
            }
            if i >= 63 {
                break;
            }
        }
        if i == 63 {
            let v = self.value(SRC_COLORS) as u8;
            put(scan[s] as usize, v);
        }
        Ok(())
    }

    fn decode_plane(&mut self, bits: &mut Bits<'_>, plane_idx: usize, is_chroma: bool) -> Result<()> {
        let (bw, bh) = if is_chroma { ((self.width + 15) >> 4, (self.height + 15) >> 4) } else { ((self.width + 7) >> 3, (self.height + 7) >> 3) };
        let width = self.width >> is_chroma as usize;
        let stride = self.cur[plane_idx].stride;
        self.init_lengths(width.max(8), bw);
        for i in 0..NB_SRC {
            self.read_bundle(bits, i)?;
        }
        let plane_len = self.cur[plane_idx].pixels.len();
        let rows = self.cur[plane_idx].rows;
        let ref_end = (bw - 1 + stride * (bh - 1)) * 8;
        let _ = rows;
        let mut block = [0i16; 64];
        let mut ublock = [0u8; 64];
        let mut dct = [0i32; 64];
        let mut coef_idx = [0usize; 64];
        for by in 0..bh {
            self.read_block_types(bits, SRC_BLOCK_TYPES)?;
            self.read_block_types(bits, SRC_SUB_BLOCK_TYPES)?;
            self.read_colors(bits)?;
            self.read_patterns(bits)?;
            self.read_motion_values(bits, SRC_X_OFF)?;
            self.read_motion_values(bits, SRC_Y_OFF)?;
            self.read_dcs(bits, SRC_INTRA_DC, 11, false)?;
            self.read_dcs(bits, SRC_INTER_DC, 11, true)?;
            self.read_runs(bits)?;
            let row0 = 8 * by * stride;
            let mut bx = 0usize;
            while bx < bw {
                let at = row0 + bx * 8;
                let blk = self.value(SRC_BLOCK_TYPES) as u8;
                if ((by & 1) != 0 || (bx & 1) != 0) && blk == SCALED_BLOCK {
                    bx += 2;
                    continue;
                }
                match blk {
                    SKIP_BLOCK => copy8(&mut self.cur[plane_idx].pixels, &self.last[plane_idx].pixels, at, at, stride),
                    SCALED_BLOCK => {
                        let sub = self.value(SRC_SUB_BLOCK_TYPES) as u8;
                        match sub {
                            RUN_BLOCK => {
                                if bits.left() < 4 {
                                    return err("out of data");
                                }
                                let mut tmp = [0u8; 64];
                                self.read_run_block(bits, &mut |p, v| tmp[p] = v)?;
                                ublock = tmp;
                            }
                            INTRA_BLOCK => {
                                dct = [0; 64];
                                dct[0] = self.value(SRC_INTRA_DC);
                                let (q, count) = self.read_dct_coeffs(bits, &mut dct, &mut coef_idx)?;
                                unquantize(&mut dct, &BINK_INTRA_QUANT[q], count, &coef_idx);
                                idct_put(&mut ublock, 0, 8, &mut dct);
                            }
                            FILL_BLOCK => {
                                let v = self.value(SRC_COLORS) as u8;
                                for j in 0..16 {
                                    let o = at + j * stride;
                                    if let Some(row) = self.cur[plane_idx].pixels.get_mut(o..o + 16) {
                                        row.fill(v);
                                    }
                                }
                            }
                            PATTERN_BLOCK => {
                                let col = [self.value(SRC_COLORS) as u8, self.value(SRC_COLORS) as u8];
                                for j in 0..8 {
                                    let mut v = self.value(SRC_PATTERN);
                                    for i in 0..8 {
                                        ublock[i + j * 8] = col[(v & 1) as usize];
                                        v >>= 1;
                                    }
                                }
                            }
                            RAW_BLOCK => {
                                for j in 0..8 {
                                    for i in 0..8 {
                                        ublock[i + j * 8] = self.value(SRC_COLORS) as u8;
                                    }
                                }
                            }
                            _ => return err("incorrect 16x16 block type"),
                        }
                        if sub != FILL_BLOCK {
                            scale_block(&ublock, &mut self.cur[plane_idx].pixels, at, stride);
                        }
                        bx += 2;
                        continue;
                    }
                    MOTION_BLOCK => self.motion(plane_idx, at, stride, ref_end, plane_len)?,
                    RUN_BLOCK => {
                        let mut tmp = [0u8; 64];
                        self.read_run_block(bits, &mut |p, v| tmp[p] = v)?;
                        for j in 0..8 {
                            let o = at + j * stride;
                            self.cur[plane_idx].pixels[o..o + 8].copy_from_slice(&tmp[j * 8..j * 8 + 8]);
                        }
                    }
                    RESIDUE_BLOCK => {
                        self.motion(plane_idx, at, stride, ref_end, plane_len)?;
                        block = [0; 64];
                        let v = bits.get(7) as i32;
                        Decoder::read_residue(bits, &mut block, v);
                        add_pixels8(&mut self.cur[plane_idx].pixels, at, stride, &block);
                    }
                    INTRA_BLOCK => {
                        dct = [0; 64];
                        dct[0] = self.value(SRC_INTRA_DC);
                        let (q, count) = self.read_dct_coeffs(bits, &mut dct, &mut coef_idx)?;
                        unquantize(&mut dct, &BINK_INTRA_QUANT[q], count, &coef_idx);
                        idct_put(&mut self.cur[plane_idx].pixels, at, stride, &mut dct);
                    }
                    FILL_BLOCK => {
                        let v = self.value(SRC_COLORS) as u8;
                        for j in 0..8 {
                            let o = at + j * stride;
                            self.cur[plane_idx].pixels[o..o + 8].fill(v);
                        }
                    }
                    INTER_BLOCK => {
                        self.motion(plane_idx, at, stride, ref_end, plane_len)?;
                        dct = [0; 64];
                        dct[0] = self.value(SRC_INTER_DC);
                        let (q, count) = self.read_dct_coeffs(bits, &mut dct, &mut coef_idx)?;
                        unquantize(&mut dct, &BINK_INTER_QUANT[q], count, &coef_idx);
                        idct_add(&mut self.cur[plane_idx].pixels, at, stride, &mut dct);
                    }
                    PATTERN_BLOCK => {
                        let col = [self.value(SRC_COLORS) as u8, self.value(SRC_COLORS) as u8];
                        for i in 0..8 {
                            let mut v = self.value(SRC_PATTERN);
                            for j in 0..8 {
                                self.cur[plane_idx].pixels[at + i * stride + j] = col[(v & 1) as usize];
                                v >>= 1;
                            }
                        }
                    }
                    RAW_BLOCK => {
                        for i in 0..8 {
                            for j in 0..8 {
                                self.cur[plane_idx].pixels[at + i * stride + j] = self.value(SRC_COLORS) as u8;
                            }
                        }
                        // The raw block's 64 values were just handed out one by one; the original steps over them in one go.
                    }
                    _ => return err("unknown block type"),
                }
                bx += 1;
            }
        }
        bits.align32();
        Ok(())
    }

    /// Copies the 8x8 block the motion vector points at in the previous frame.
    fn motion(&mut self, plane: usize, at: usize, stride: usize, ref_end: usize, plane_len: usize) -> Result<()> {
        let xoff = self.value(SRC_X_OFF);
        let yoff = self.value(SRC_Y_OFF);
        let r = at as isize + xoff as isize + yoff as isize * stride as isize;
        if r < 0 || r as usize > ref_end || r as usize + 7 * stride + 8 > plane_len {
            return err("copy out of bounds");
        }
        copy8(&mut self.cur[plane].pixels, &self.last[plane].pixels, at, r as usize, stride);
        Ok(())
    }

    fn decode_frame(&mut self, data: &[u8]) -> Result<()> {
        let mut bits = Bits { data, pos: 0 };
        if self.has_alpha {
            if self.version >= b'i' {
                bits.skip(32);
            }
            self.decode_plane(&mut bits, 3, false)?;
        }
        if self.version >= b'i' {
            bits.skip(32);
        }
        for plane in 0..3 {
            let idx = if plane == 0 || !self.swap_planes { plane } else { plane ^ 3 };
            self.decode_plane(&mut bits, idx, plane != 0)?;
            if bits.left() <= 0 {
                break;
            }
        }
        for (cur, last) in self.cur.iter().zip(self.last.iter_mut()) {
            last.pixels.copy_from_slice(&cur.pixels);
        }
        Ok(())
    }
}

fn copy8(dst: &mut [u8], src: &[u8], at: usize, from: usize, stride: usize) {
    for j in 0..8 {
        let (d, s) = (at + j * stride, from + j * stride);
        if let (Some(row), true) = (src.get(s..s + 8), d + 8 <= dst.len()) {
            dst[d..d + 8].copy_from_slice(row);
        }
    }
}

fn scale_block(src: &[u8; 64], dst: &mut [u8], at: usize, stride: usize) {
    for j in 0..8 {
        for i in 0..8 {
            let v = src[j * 8 + i];
            for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                if let Some(p) = dst.get_mut(at + (2 * j + dy) * stride + 2 * i + dx) {
                    *p = v;
                }
            }
        }
    }
}

fn add_pixels8(dst: &mut [u8], at: usize, stride: usize, block: &[i16; 64]) {
    for j in 0..8 {
        for i in 0..8 {
            let p = &mut dst[at + j * stride + i];
            *p = (*p as i32 + block[j * 8 + i] as i32) as u8;
        }
    }
}

fn unquantize(block: &mut [i32; 64], quant: &[i32; 64], count: usize, coef_idx: &[usize; 64]) {
    block[0] = (block[0].wrapping_mul(quant[0])) >> 11;
    for &idx in coef_idx.iter().take(count) {
        let p = BINK_SCAN[idx] as usize;
        block[p] = (block[p].wrapping_mul(quant[idx])) >> 11;
    }
}

const A1: i32 = 2896;
const A2: i32 = 2217;
const A3: i32 = 3784;
const A4: i32 = -5352;

fn mul(x: i32, y: i32) -> i32 {
    ((x as u32).wrapping_mul(y as u32) as i32) >> 11
}

/// One 8-point pass of the inverse transform: `s` are the eight inputs, the result the eight outputs (before any rounding).
fn idct8(s: [i32; 8]) -> [i32; 8] {
    let a0 = s[0].wrapping_add(s[4]);
    let a1 = s[0].wrapping_sub(s[4]);
    let a2 = s[2].wrapping_add(s[6]);
    let a3 = mul(A1, s[2].wrapping_sub(s[6]));
    let a4 = s[5].wrapping_add(s[3]);
    let a5 = s[5].wrapping_sub(s[3]);
    let a6 = s[1].wrapping_add(s[7]);
    let a7 = s[1].wrapping_sub(s[7]);
    let b0 = a4.wrapping_add(a6);
    let b1 = mul(A3, a5.wrapping_add(a7));
    let b2 = mul(A4, a5).wrapping_sub(b0).wrapping_add(b1);
    let b3 = mul(A1, a6.wrapping_sub(a4)).wrapping_sub(b2);
    let b4 = mul(A2, a7).wrapping_add(b3).wrapping_sub(b1);
    [
        a0.wrapping_add(a2).wrapping_add(b0),
        a1.wrapping_add(a3).wrapping_sub(a2).wrapping_add(b2),
        a1.wrapping_sub(a3).wrapping_add(a2).wrapping_add(b3),
        a0.wrapping_sub(a2).wrapping_sub(b4),
        a0.wrapping_sub(a2).wrapping_add(b4),
        a1.wrapping_sub(a3).wrapping_add(a2).wrapping_sub(b3),
        a1.wrapping_add(a3).wrapping_sub(a2).wrapping_sub(b2),
        a0.wrapping_add(a2).wrapping_sub(b0),
    ]
}

/// The inverse transform of a block: columns, then rows (each row output rounded by `(x + 0x7F) >> 8`).
fn idct(block: &[i32; 64]) -> [i32; 64] {
    let mut temp = [0i32; 64];
    for i in 0..8 {
        let col = [block[i], block[8 + i], block[16 + i], block[24 + i], block[32 + i], block[40 + i], block[48 + i], block[56 + i]];
        let out = if col[1..].iter().all(|&v| v == 0) { [col[0]; 8] } else { idct8(col) };
        for k in 0..8 {
            temp[k * 8 + i] = out[k];
        }
    }
    let mut out = [0i32; 64];
    for r in 0..8 {
        let row: [i32; 8] = temp[r * 8..r * 8 + 8].try_into().unwrap();
        let o = idct8(row);
        for k in 0..8 {
            out[r * 8 + k] = o[k].wrapping_add(0x7F) >> 8;
        }
    }
    out
}

fn idct_put(dst: &mut [u8], at: usize, stride: usize, block: &mut [i32; 64]) {
    let out = idct(block);
    for j in 0..8 {
        for i in 0..8 {
            if let Some(p) = dst.get_mut(at + j * stride + i) {
                *p = out[j * 8 + i] as u8;
            }
        }
    }
}

fn idct_add(dst: &mut [u8], at: usize, stride: usize, block: &mut [i32; 64]) {
    let out = idct(block);
    for j in 0..8 {
        for i in 0..8 {
            if let Some(p) = dst.get_mut(at + j * stride + i) {
                *p = (*p as i32 + out[j * 8 + i]) as u8;
            }
        }
    }
}

/// A Bink video file, decoded a frame at a time to RGBA.
pub struct BinkVideo {
    data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    /// Frames per second.
    pub fps: f32,
    flags: u32,
    audio_tracks: usize,
    offsets: Vec<usize>,
    decoder: Decoder,
    /// The frame the planes now hold the decoding of (None: none yet).
    decoded: Option<u32>,
    rgba: Vec<u8>,
    rgba_of: Option<u32>,
}

fn le32(d: &[u8], at: usize) -> Option<u32> {
    d.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

impl BinkVideo {
    /// Reads the header and the frame table of a `BIK` file.
    pub fn new(data: Vec<u8>) -> Result<BinkVideo> {
        if data.len() < 44 || &data[0..3] != b"BIK" {
            return err("not a Bink file");
        }
        let version = data[3];
        if !(b'f'..=b'k').contains(&version) {
            return err("unsupported Bink revision");
        }
        let frames = le32(&data, 8).ok_or_else(|| Error("short header".into()))?;
        let width = le32(&data, 20).ok_or_else(|| Error("short header".into()))?;
        let height = le32(&data, 24).ok_or_else(|| Error("short header".into()))?;
        let (num, den) = (le32(&data, 28).unwrap_or(30), le32(&data, 32).unwrap_or(1).max(1));
        let flags = le32(&data, 36).unwrap_or(0);
        let audio_tracks = le32(&data, 40).unwrap_or(0) as usize;
        if frames == 0 || frames > 1_000_000 || width == 0 || height == 0 || width > 8192 || height > 8192 {
            return err("implausible Bink header");
        }
        let table = 44 + 12 * audio_tracks;
        let mut offsets = Vec::with_capacity(frames as usize + 1);
        for i in 0..=frames as usize {
            let v = le32(&data, table + 4 * i).ok_or_else(|| Error("short frame table".into()))?;
            offsets.push((v & !1) as usize);
        }
        let decoder = Decoder::new(version, flags, width as usize, height as usize);
        Ok(BinkVideo {
            data,
            width,
            height,
            frames,
            fps: num as f32 / den as f32,
            flags,
            audio_tracks,
            offsets,
            decoder,
            decoded: None,
            rgba: vec![0; width as usize * height as usize * 4],
            rgba_of: None,
        })
    }

    /// The video part of a frame's data (after its audio packets).
    fn video_of(&self, frame: u32) -> Result<&[u8]> {
        let (start, end) = (self.offsets[frame as usize], self.offsets[frame as usize + 1]);
        let packet = self.data.get(start..end.min(self.data.len())).ok_or_else(|| Error("frame outside the file".into()))?;
        let mut at = 0usize;
        for _ in 0..self.audio_tracks {
            let size = le32(packet, at).ok_or_else(|| Error("short audio packet".into()))? as usize;
            at += 4 + size;
        }
        packet.get(at..).ok_or_else(|| Error("audio packets overrun the frame".into()))
    }

    /// Whether the picture has an alpha plane.
    pub fn has_alpha(&self) -> bool {
        self.flags & FLAG_ALPHA != 0
    }

    /// The picture of `frame` as RGBA (`width * height * 4` bytes). Frames are decoded in order, so going back starts again from the
    /// first. `None` when the frame is not there or does not decode.
    pub fn frame_rgba(&mut self, frame: u32) -> Option<&[u8]> {
        if frame >= self.frames {
            return None;
        }
        if self.rgba_of == Some(frame) {
            return Some(&self.rgba);
        }
        let from = match self.decoded {
            Some(d) if d <= frame => d + 1,
            _ => 0,
        };
        for f in from..=frame {
            let data = self.video_of(f).ok()?.to_vec();
            if self.decoder.decode_frame(&data).is_err() {
                self.decoded = None;
                return None;
            }
            self.decoded = Some(f);
        }
        self.to_rgba();
        self.rgba_of = Some(frame);
        Some(&self.rgba)
    }

    fn to_rgba(&mut self) {
        let (w, h) = (self.width as usize, self.height as usize);
        let d = &self.decoder;
        let (y, u, v) = (&d.cur[0], &d.cur[1], &d.cur[2]);
        let alpha = self.flags & FLAG_ALPHA != 0;
        let gray = self.flags & FLAG_GRAY != 0;
        for row in 0..h {
            for col in 0..w {
                let yy = y.pixels[row * y.stride + col] as f32 - 16.0;
                let (cu, cv) = if gray {
                    (0.0, 0.0)
                } else {
                    (u.pixels[(row >> 1) * u.stride + (col >> 1)] as f32 - 128.0, v.pixels[(row >> 1) * v.stride + (col >> 1)] as f32 - 128.0)
                };
                let r = 1.164 * yy + 1.596 * cv;
                let g = 1.164 * yy - 0.813 * cv - 0.391 * cu;
                let b = 1.164 * yy + 2.018 * cu;
                let o = (row * w + col) * 4;
                self.rgba[o] = r.clamp(0.0, 255.0) as u8;
                self.rgba[o + 1] = g.clamp(0.0, 255.0) as u8;
                self.rgba[o + 2] = b.clamp(0.0, 255.0) as u8;
                self.rgba[o + 3] = if alpha { d.cur[3].pixels[row * d.cur[3].stride + col] } else { 255 };
            }
        }
    }
}
