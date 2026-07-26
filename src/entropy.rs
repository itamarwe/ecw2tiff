//! Entropy decoders for ECW sideband data: RAW, ZEROS, RUN_ZERO, HUFFMAN,
//! and the Schindler range coder (RANGE / RANGE8) with its quasistatic model.
//!
//! Symbol conventions (16-bit quantized coefficients):
//!   bit 15 (RUN_MASK)  -> value is a run of zeros, length = value & 0x7fff
//!   bit 14 (SIGN_MASK) -> negative value, magnitude = value & 0x3fff

pub const ENCODE_RAW: u8 = 1;
pub const ENCODE_HUFFMAN: u8 = 2;
pub const ENCODE_RANGE: u8 = 3;
pub const ENCODE_RANGE8: u8 = 4;
pub const ENCODE_ZEROS: u8 = 5;
pub const ENCODE_RUN_ZERO: u8 = 6;

pub const RUN_MASK: u16 = 0x8000;
pub const SIGN_MASK: u16 = 0x4000;
pub const VALUE_MASK: u16 = 0x3fff;
pub const MAX_RUN_LENGTH: u16 = 0x7fff;

pub type Error = String;

// ---------------------------------------------------------------------------
// Range coder (Michael Schindler's rangecod, decode side, CODE_BITS = 32)
// ---------------------------------------------------------------------------

const EXTRA_BITS: u32 = 7; // ((32 - 2) % 8) + 1
const TOP_VALUE: u32 = 1 << 31;
const BOTTOM_VALUE: u32 = TOP_VALUE >> 8;
const SHIFT_BITS: u32 = 32 - 9;

#[derive(Clone)]
pub struct RangeDecoder {
    low: u32,
    range: u32,
    help: u32,
    buffer: u8,
    pos: usize, // index into the block payload
}

impl RangeDecoder {
    /// `data` is the full sideband payload; decoding starts at `pos`.
    pub fn start(data: &[u8], mut pos: usize) -> RangeDecoder {
        let inbyte = |p: &mut usize| -> u8 {
            let b = data.get(*p).copied().unwrap_or(0);
            *p += 1;
            b
        };
        let _c = inbyte(&mut pos); // the char passed to start_encoding
        let buffer = inbyte(&mut pos);
        RangeDecoder {
            low: (buffer >> (8 - EXTRA_BITS)) as u32,
            range: 1 << EXTRA_BITS,
            help: 0,
            buffer,
            pos,
        }
    }

    #[inline]
    fn inbyte(&mut self, data: &[u8]) -> u8 {
        let b = data.get(self.pos).copied().unwrap_or(0);
        self.pos += 1;
        b
    }

    #[inline]
    fn normalize(&mut self, data: &[u8]) {
        while self.range <= BOTTOM_VALUE {
            self.low = (self.low << 8) | (((self.buffer as u32) << EXTRA_BITS) & 0xff);
            self.buffer = self.inbyte(data);
            self.low |= (self.buffer >> (8 - EXTRA_BITS)) as u32;
            self.range <<= 8;
        }
    }

    #[inline]
    fn decode_culshift(&mut self, data: &[u8], shift: u32) -> u32 {
        self.normalize(data);
        self.help = self.range >> shift;
        let tmp = self.low / self.help;
        if (tmp >> shift) != 0 {
            (1 << shift) - 1
        } else {
            tmp
        }
    }

    #[inline]
    fn decode_update(&mut self, sy_f: u32, lt_f: u32, tot_f: u32) {
        let tmp = self.help.wrapping_mul(lt_f);
        self.low = self.low.wrapping_sub(tmp);
        if lt_f + sy_f < tot_f {
            self.range = self.help.wrapping_mul(sy_f);
        } else {
            self.range = self.range.wrapping_sub(tmp);
        }
    }

    #[allow(unused)]
    fn shift_bits() -> u32 {
        SHIFT_BITS
    }
}

// ---------------------------------------------------------------------------
// Quasistatic model (qsmodel), n = 257 symbols, total frequency 1 << 12
// ---------------------------------------------------------------------------

const QSM_N: usize = 257;
const QSM_LG_TOTF: i32 = 12;
const QSM_TARGETRESCALE: i32 = 2000;
const TBLSHIFT: i32 = 7;

#[derive(Clone)]
pub struct QsModel {
    left: i32,
    nextleft: i32,
    rescale: i32,
    incr: i32,
    searchshift: i32,
    cf: [u16; QSM_N + 1],
    newf: [u16; QSM_N + 1],
    search: [u16; (1 << TBLSHIFT) + 1],
}

impl Default for QsModel {
    fn default() -> Self {
        Self::new()
    }
}

impl QsModel {
    pub fn new() -> QsModel {
        let mut m = QsModel {
            left: 0,
            nextleft: 0,
            rescale: (QSM_N as i32 >> 4) | 2,
            incr: 0,
            searchshift: (QSM_LG_TOTF - TBLSHIFT).max(0),
            cf: [0; QSM_N + 1],
            newf: [0; QSM_N + 1],
            search: [0; (1 << TBLSHIFT) + 1],
        };
        m.cf[QSM_N] = 1 << QSM_LG_TOTF;
        m.cf[0] = 0;
        m.search[1 << TBLSHIFT] = (QSM_N - 1) as u16;
        // resetqsmodel with init == NULL
        let initval = (m.cf[QSM_N] as i32 / QSM_N as i32) as u16;
        let end = m.cf[QSM_N] as i32 % QSM_N as i32;
        for i in 0..QSM_N {
            m.newf[i] = if (i as i32) < end {
                initval + 1
            } else {
                initval
            };
        }
        m.dorescale();
        m
    }

    fn dorescale(&mut self) {
        if self.nextleft != 0 {
            self.incr += 1;
            self.left = self.nextleft;
            self.nextleft = 0;
            return;
        }
        if self.rescale < QSM_TARGETRESCALE {
            self.rescale <<= 1;
            if self.rescale > QSM_TARGETRESCALE {
                self.rescale = QSM_TARGETRESCALE;
            }
        }
        let mut cf = self.cf[QSM_N] as i32;
        let mut missing = cf;
        for i in (1..QSM_N).rev() {
            let mut tmp = self.newf[i] as i32;
            cf -= tmp;
            self.cf[i] = cf as u16;
            tmp = (tmp >> 1) | 1;
            missing -= tmp;
            self.newf[i] = tmp as u16;
        }
        self.newf[0] = (self.newf[0] >> 1) | 1;
        missing -= self.newf[0] as i32;
        self.incr = missing / self.rescale;
        self.nextleft = missing % self.rescale;
        self.left = self.rescale - self.nextleft;
        // rebuild the search table
        let ss = self.searchshift;
        let mut i = QSM_N;
        while i > 0 {
            let end = ((self.cf[i] as i32 - 1) >> ss) as usize;
            i -= 1;
            let start = (self.cf[i] as i32 >> ss) as usize;
            for s in start..=end {
                self.search[s] = i as u16;
            }
        }
    }

    #[inline]
    fn getsym(&self, lt_f: u32) -> usize {
        let idx = (lt_f >> self.searchshift) as usize;
        let mut lo = self.search[idx] as usize;
        let mut hi = self.search[idx + 1] as usize + 1;
        while lo + 1 < hi {
            let mid = (lo + hi) >> 1;
            if (lt_f as i32) < self.cf[mid] as i32 {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        lo
    }

    #[inline]
    fn getfreq(&self, sym: usize) -> (u32, u32) {
        let lt = self.cf[sym] as u32;
        let sy = self.cf[sym + 1] as u32 - lt;
        (sy, lt)
    }

    #[inline]
    fn update(&mut self, sym: usize) {
        if self.left <= 0 {
            self.dorescale();
        }
        self.left -= 1;
        self.newf[sym] = self.newf[sym].wrapping_add(self.incr as u16);
    }
}

/// Decode one byte-symbol (0..=256) with the range coder + model.
#[inline]
pub fn range_symbol(rc: &mut RangeDecoder, qsm: &mut QsModel, data: &[u8]) -> usize {
    let lt = rc.decode_culshift(data, QSM_LG_TOTF as u32);
    let ch = qsm.getsym(lt);
    let (sy, ltf) = qsm.getfreq(ch);
    rc.decode_update(sy, ltf, 1 << QSM_LG_TOTF);
    qsm.update(ch);
    ch
}

// ---------------------------------------------------------------------------
// Huffman coder (NCSHuffmanCoder)
// ---------------------------------------------------------------------------

const HUFF_SMALL_SYMBOL: u8 = 0x40;
const HUFF_SMALL_SHIFT: u32 = 10;

#[derive(Clone, Copy)]
pub enum HuffSymbol {
    Value(i16),
    /// run length minus one (as stored by the SDK decoder)
    ZeroRun(u16),
}

#[derive(Clone, Copy)]
enum HuffNode {
    /// children indices (0-branch, 1-branch)
    Internal(u32, u32),
    Leaf(HuffSymbol),
}

#[derive(Clone)]
pub struct HuffTree {
    nodes: Vec<HuffNode>,
    root: u32,
}

impl HuffTree {
    /// Parse a packed tree (preorder: 0 = internal node followed by its two
    /// subtrees, nonzero = leaf); advances `pos` past the tree bytes.
    /// Iterative with an explicit stack: a hostile deep tree must not be able
    /// to overflow the call stack.
    pub fn parse(data: &[u8], pos: &mut usize) -> Result<HuffTree, Error> {
        if data.len() < *pos + 2 {
            return Err("huffman tree truncated".into());
        }
        let n_nodes = (u16::from_le_bytes([data[*pos], data[*pos + 1]]) as u32).wrapping_add(1);
        *pos += 2;
        let mut t = HuffTree {
            nodes: Vec::with_capacity(n_nodes as usize),
            root: 0,
        };
        let mut budget = n_nodes;
        // internal nodes whose children are still being parsed: (index, next child slot)
        let mut stack: Vec<(u32, u8)> = Vec::new();
        loop {
            budget = budget.wrapping_sub(1);
            if budget == 0 {
                return Err("huffman tree node budget exhausted".into());
            }
            let byte = *data.get(*pos).ok_or("huffman tree truncated")?;
            *pos += 1;
            let idx = t.nodes.len() as u32;
            if byte == 0 {
                t.nodes.push(HuffNode::Internal(0, 0)); // children patched below
            } else {
                let value: u16 = if byte & HUFF_SMALL_SYMBOL != 0 {
                    (((byte & 0x30) as u16) << HUFF_SMALL_SHIFT) | (byte & 0x0f) as u16
                } else {
                    if data.len() < *pos + 2 {
                        return Err("huffman tree truncated".into());
                    }
                    let v = u16::from_le_bytes([data[*pos], data[*pos + 1]]);
                    *pos += 2;
                    v
                };
                let sym = if value & RUN_MASK != 0 {
                    HuffSymbol::ZeroRun((value & MAX_RUN_LENGTH).wrapping_sub(1))
                } else if value & SIGN_MASK != 0 {
                    HuffSymbol::Value(-((value & VALUE_MASK) as i16))
                } else {
                    HuffSymbol::Value(value as i16)
                };
                t.nodes.push(HuffNode::Leaf(sym));
            }
            if idx != 0 {
                let &(parent, slot) = stack.last().unwrap();
                if let HuffNode::Internal(c0, c1) = &mut t.nodes[parent as usize] {
                    if slot == 0 {
                        *c0 = idx;
                    } else {
                        *c1 = idx;
                    }
                }
                if slot == 0 {
                    stack.last_mut().unwrap().1 = 1;
                } else {
                    stack.pop();
                }
            }
            if byte == 0 {
                stack.push((idx, 0));
            }
            if stack.is_empty() {
                return Ok(t); // root subtree complete
            }
        }
    }

    /// Decode one symbol; bits are LSB-first within each byte starting at
    /// absolute bit position `bitpos` in `data`.
    #[inline]
    pub fn decode(&self, data: &[u8], bitpos: &mut usize) -> HuffSymbol {
        let mut node = self.root;
        loop {
            match self.nodes[node as usize] {
                HuffNode::Leaf(sym) => return sym,
                HuffNode::Internal(c0, c1) => {
                    let byte = data.get(*bitpos >> 3).copied().unwrap_or(0);
                    let bit = (byte >> (*bitpos & 7)) & 1;
                    *bitpos += 1;
                    node = if bit == 0 { c0 } else { c1 };
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// unpack_data: whole-buffer decode (used for the block offset table)
// ---------------------------------------------------------------------------

pub fn unpack_data(fmt: u8, packed: &[u8], raw_length: usize) -> Result<Vec<u8>, Error> {
    let mut out = vec![0u8; raw_length];
    match fmt {
        ENCODE_ZEROS => {}
        ENCODE_RAW => {
            if packed.len() < raw_length {
                return Err("raw data truncated".into());
            }
            out.copy_from_slice(&packed[..raw_length]);
        }
        ENCODE_RUN_ZERO => {
            let mut word_count = raw_length / 2;
            let mut pos = 0usize;
            let mut o = 0usize;
            while word_count > 0 {
                word_count -= 1;
                if packed.len() < pos + 2 {
                    return Err("run-zero data truncated".into());
                }
                let v = u16::from_le_bytes([packed[pos], packed[pos + 1]]);
                pos += 2;
                if v & RUN_MASK != 0 {
                    let run = (v & MAX_RUN_LENGTH) as usize;
                    if run == 0 {
                        return Err("zero-length run".into());
                    }
                    let zero_run = run - 1;
                    let nzero = if zero_run >= word_count {
                        let n = word_count + 1;
                        word_count = 0;
                        n
                    } else {
                        word_count -= zero_run;
                        zero_run + 1
                    };
                    o += nzero * 2; // already zeroed
                } else {
                    let val: i16 = if v & SIGN_MASK != 0 {
                        -((v & VALUE_MASK) as i16)
                    } else {
                        v as i16
                    };
                    out[o..o + 2].copy_from_slice(&val.to_le_bytes());
                    o += 2;
                }
            }
        }
        ENCODE_HUFFMAN => {
            let mut pos = 0usize;
            let tree = HuffTree::parse(packed, &mut pos)?;
            let mut bitpos = pos * 8;
            let mut word_count = raw_length / 2;
            let mut o = 0usize;
            while word_count > 0 {
                word_count -= 1;
                match tree.decode(packed, &mut bitpos) {
                    HuffSymbol::ZeroRun(run_m1) => {
                        let zero_run = run_m1 as usize;
                        let nzero = if zero_run >= word_count {
                            let n = word_count + 1;
                            word_count = 0;
                            n
                        } else {
                            word_count -= zero_run;
                            zero_run + 1
                        };
                        o += nzero * 2;
                    }
                    HuffSymbol::Value(v) => {
                        out[o..o + 2].copy_from_slice(&v.to_le_bytes());
                        o += 2;
                    }
                }
            }
        }
        ENCODE_RANGE => {
            let mut rc = RangeDecoder::start(packed, 0);
            let mut qsm = QsModel::new();
            for b in out.iter_mut() {
                *b = range_symbol(&mut rc, &mut qsm, packed) as u8;
            }
        }
        ENCODE_RANGE8 => {
            let mut rc = RangeDecoder::start(packed, 0);
            let mut qsm = QsModel::new();
            let hi = range_symbol(&mut rc, &mut qsm, packed);
            let lo = range_symbol(&mut rc, &mut qsm, packed);
            let mut prev = (((hi & 0xff) as u16) << 8 | (lo & 0xff) as u16) as i16;
            for chunk in out.chunks_exact_mut(2) {
                let diff = range_symbol(&mut rc, &mut qsm, packed) as u8 as i8;
                prev = prev.wrapping_add(diff as i16);
                chunk.copy_from_slice(&prev.to_le_bytes());
            }
        }
        _ => return Err(format!("invalid encode format {fmt}")),
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Per-sideband streaming line decoder (mirrors unpack_start_line_block /
// unpack_line). All offsets are into the block's packed buffer.
// ---------------------------------------------------------------------------

pub enum SbDecoder {
    Zeros,
    Raw {
        pos: usize,
    },
    RunZero {
        pos: usize,
        pending: u32, // outstanding zero words
    },
    Huffman {
        tree: HuffTree,
        bitpos: usize,
        pending: u32,
    },
    Range {
        rc: RangeDecoder,
        qsm: Box<QsModel>,
    },
    Range8 {
        rc: RangeDecoder,
        qsm: Box<QsModel>,
        prev: i16,
    },
}

impl SbDecoder {
    /// Set up decoding of one sideband stream. `off` points at the encode
    /// format byte; payload follows. Skips `lines_to_skip` lines of
    /// `line_length` symbols each.
    pub fn start(
        data: &[u8],
        off: usize,
        line_length: usize,
        lines_to_skip: usize,
    ) -> Result<SbDecoder, Error> {
        let fmt = *data.get(off).ok_or("sideband offset out of range")?;
        let payload = off + 1;
        let skip_words = line_length * lines_to_skip;
        Ok(match fmt {
            ENCODE_ZEROS => SbDecoder::Zeros,
            ENCODE_RAW => SbDecoder::Raw {
                pos: payload + skip_words * 2,
            },
            ENCODE_RUN_ZERO => {
                let mut d = SbDecoder::RunZero {
                    pos: payload,
                    pending: 0,
                };
                d.skip_words(data, skip_words)?;
                d
            }
            ENCODE_HUFFMAN => {
                let mut pos = payload;
                let tree = HuffTree::parse(data, &mut pos)?;
                let mut d = SbDecoder::Huffman {
                    tree,
                    bitpos: pos * 8,
                    pending: 0,
                };
                d.skip_words(data, skip_words)?;
                d
            }
            ENCODE_RANGE => {
                let mut rc = RangeDecoder::start(data, payload);
                let mut qsm = Box::new(QsModel::new());
                for _ in 0..skip_words * 2 {
                    range_symbol(&mut rc, &mut qsm, data);
                }
                SbDecoder::Range { rc, qsm }
            }
            ENCODE_RANGE8 => {
                let mut rc = RangeDecoder::start(data, payload);
                let mut qsm = Box::new(QsModel::new());
                let hi = range_symbol(&mut rc, &mut qsm, data);
                let lo = range_symbol(&mut rc, &mut qsm, data);
                let mut prev = (((hi & 0xff) as u16) << 8 | (lo & 0xff) as u16) as i16;
                for _ in 0..skip_words {
                    let diff = range_symbol(&mut rc, &mut qsm, data) as u8 as i8;
                    prev = prev.wrapping_add(diff as i16);
                }
                SbDecoder::Range8 { rc, qsm, prev }
            }
            _ => return Err(format!("invalid sideband encode format {fmt}")),
        })
    }

    /// Consume `n` symbols without producing output (RunZero/Huffman only;
    /// other variants handle skipping inline).
    fn skip_words(&mut self, data: &[u8], n: usize) -> Result<(), Error> {
        let mut count = n;
        match self {
            SbDecoder::RunZero { pos, pending } => {
                // consume outstanding zeros first
                if *pending as usize >= count {
                    *pending -= count as u32;
                    return Ok(());
                }
                count -= *pending as usize;
                *pending = 0;
                while count > 0 {
                    count -= 1;
                    if data.len() < *pos + 2 {
                        return Err("run-zero truncated".into());
                    }
                    let v = u16::from_le_bytes([data[*pos], data[*pos + 1]]);
                    *pos += 2;
                    if v & RUN_MASK != 0 {
                        let run = (v & MAX_RUN_LENGTH) as usize;
                        if run == 0 {
                            return Err("zero-length run".into());
                        }
                        let zero_run = run - 1;
                        if zero_run >= count {
                            *pending = (zero_run - count) as u32;
                            return Ok(());
                        }
                        count -= zero_run;
                    }
                }
                Ok(())
            }
            SbDecoder::Huffman {
                tree,
                bitpos,
                pending,
            } => {
                if *pending as usize >= count {
                    *pending -= count as u32;
                    return Ok(());
                }
                count -= *pending as usize;
                *pending = 0;
                while count > 0 {
                    count -= 1;
                    if let HuffSymbol::ZeroRun(run_m1) = tree.decode(data, bitpos) {
                        let zero_run = run_m1 as usize;
                        if zero_run >= count {
                            *pending = (zero_run - count) as u32;
                            return Ok(());
                        }
                        count -= zero_run;
                    }
                }
                Ok(())
            }
            _ => unreachable!("skip_words on non-run decoder"),
        }
    }

    /// Decode one line: skip `pre` symbols, write `out.len()` dequantized
    /// values, skip `post` symbols. `line_length` is the full block width.
    pub fn decode_line(
        &mut self,
        data: &[u8],
        pre: usize,
        post: usize,
        line_length: usize,
        out: &mut [f32],
        bin_size: f32,
    ) -> Result<(), Error> {
        let valid = out.len();
        match self {
            SbDecoder::Zeros => {
                out.fill(0.0);
            }
            SbDecoder::Raw { pos } => {
                let start = *pos + pre * 2;
                if data.len() < start + valid * 2 {
                    return Err("raw sideband truncated".into());
                }
                for (i, o) in out.iter_mut().enumerate() {
                    let v = i16::from_le_bytes([data[start + i * 2], data[start + i * 2 + 1]]);
                    *o = v as f32 * bin_size;
                }
                *pos += line_length * 2;
            }
            SbDecoder::RunZero { .. } => {
                self.skip_words(data, pre)?;
                self.run_zero_emit(data, out, bin_size)?;
                self.skip_words(data, post)?;
            }
            SbDecoder::Huffman { .. } => {
                self.skip_words(data, pre)?;
                self.huffman_emit(data, out, bin_size)?;
                self.skip_words(data, post)?;
            }
            SbDecoder::Range { rc, qsm } => {
                for _ in 0..pre * 2 {
                    range_symbol(rc, qsm, data);
                }
                for o in out.iter_mut() {
                    let lo = range_symbol(rc, qsm, data);
                    if lo == 256 {
                        return Err("unexpected end of range stream".into());
                    }
                    let hi = range_symbol(rc, qsm, data);
                    let v = ((lo & 0xff) as u16 | (((hi & 0xff) as u16) << 8)) as i16;
                    *o = v as f32 * bin_size;
                }
                for _ in 0..post * 2 {
                    range_symbol(rc, qsm, data);
                }
            }
            SbDecoder::Range8 { rc, qsm, prev } => {
                for _ in 0..pre {
                    let diff = range_symbol(rc, qsm, data) as u8 as i8;
                    *prev = prev.wrapping_add(diff as i16);
                }
                for o in out.iter_mut() {
                    let ch = range_symbol(rc, qsm, data);
                    if ch == 256 {
                        return Err("unexpected end of range8 stream".into());
                    }
                    let diff = ch as u8 as i8;
                    *prev = prev.wrapping_add(diff as i16);
                    *o = *prev as f32 * bin_size;
                }
                for _ in 0..post {
                    let diff = range_symbol(rc, qsm, data) as u8 as i8;
                    *prev = prev.wrapping_add(diff as i16);
                }
            }
        }
        Ok(())
    }

    fn run_zero_emit(&mut self, data: &[u8], out: &mut [f32], bin_size: f32) -> Result<(), Error> {
        let SbDecoder::RunZero { pos, pending } = self else {
            unreachable!()
        };
        let mut count = out.len();
        let mut o = 0usize;
        if *pending > 0 && count > 0 {
            let nzero = (*pending as usize).min(count);
            out[o..o + nzero].fill(0.0);
            o += nzero;
            *pending -= nzero as u32;
            count -= nzero;
        }
        while count > 0 {
            count -= 1;
            if data.len() < *pos + 2 {
                return Err("run-zero truncated".into());
            }
            let v = u16::from_le_bytes([data[*pos], data[*pos + 1]]);
            *pos += 2;
            if v & RUN_MASK != 0 {
                let run = (v & MAX_RUN_LENGTH) as usize;
                if run == 0 {
                    return Err("zero-length run".into());
                }
                let zero_run = run - 1;
                let nzero = if zero_run >= count {
                    *pending = (zero_run - count) as u32;
                    let n = count + 1;
                    count = 0;
                    n
                } else {
                    count -= zero_run;
                    zero_run + 1
                };
                out[o..o + nzero].fill(0.0);
                o += nzero;
            } else if v & SIGN_MASK != 0 {
                out[o] = -((v & VALUE_MASK) as f32 * bin_size);
                o += 1;
            } else {
                out[o] = v as f32 * bin_size;
                o += 1;
            }
        }
        Ok(())
    }

    fn huffman_emit(&mut self, data: &[u8], out: &mut [f32], bin_size: f32) -> Result<(), Error> {
        let SbDecoder::Huffman {
            tree,
            bitpos,
            pending,
        } = self
        else {
            unreachable!()
        };
        let mut count = out.len();
        let mut o = 0usize;
        if *pending > 0 && count > 0 {
            let nzero = (*pending as usize).min(count);
            out[o..o + nzero].fill(0.0);
            o += nzero;
            *pending -= nzero as u32;
            count -= nzero;
        }
        while count > 0 {
            count -= 1;
            match tree.decode(data, bitpos) {
                HuffSymbol::ZeroRun(run_m1) => {
                    let zero_run = run_m1 as usize;
                    let nzero = if zero_run >= count {
                        *pending = (zero_run - count) as u32;
                        let n = count + 1;
                        count = 0;
                        n
                    } else {
                        count -= zero_run;
                        zero_run + 1
                    };
                    out[o..o + nzero].fill(0.0);
                    o += nzero;
                }
                HuffSymbol::Value(v) => {
                    out[o] = v as f32 * bin_size;
                    o += 1;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn huffman_parse_and_decode() {
        // node count 5 (stored as count, budget = count + 1), then preorder:
        // internal, leaf Value(5) (small symbol), internal,
        // leaf Value(1) (small symbol), leaf ZeroRun(2) (full u16 0x8003)
        let mut data = vec![0x05, 0x00, 0x00, 0x45, 0x00, 0x41, 0x01, 0x03, 0x80];
        let mut pos = 0usize;
        let tree = HuffTree::parse(&data, &mut pos).unwrap();
        assert_eq!(pos, 9);
        // codes: 0 -> Value(5), 10 -> Value(1), 11 -> ZeroRun(2);
        // LSB-first bit sequence 0,1,0,1,1 = 0x1A
        data.push(0x1A);
        let mut bitpos = pos * 8;
        assert!(matches!(
            tree.decode(&data, &mut bitpos),
            HuffSymbol::Value(5)
        ));
        assert!(matches!(
            tree.decode(&data, &mut bitpos),
            HuffSymbol::Value(1)
        ));
        assert!(matches!(
            tree.decode(&data, &mut bitpos),
            HuffSymbol::ZeroRun(2)
        ));
    }

    #[test]
    fn huffman_deep_tree_no_stack_overflow() {
        // Left-degenerate chain of 30000 internal nodes: valid, but would
        // overflow the call stack with a recursive parser.
        const DEPTH: usize = 30000;
        let mut data = vec![0xff, 0xff];
        data.extend(std::iter::repeat_n(0u8, DEPTH));
        data.extend(std::iter::repeat_n(0x45u8, DEPTH + 1));
        let mut pos = 0usize;
        let tree = HuffTree::parse(&data, &mut pos).unwrap();
        assert_eq!(tree.nodes.len(), 2 * DEPTH + 1);
    }

    #[test]
    fn huffman_truncated_tree_errors() {
        let mut pos = 0usize;
        assert!(HuffTree::parse(&[0x05, 0x00, 0x00, 0x45], &mut pos).is_err());
    }

    #[test]
    fn run_zero_rejects_zero_length_run() {
        // one RUN_MASK word with run length 0
        let packed = 0x8000u16.to_le_bytes();
        assert!(unpack_data(ENCODE_RUN_ZERO, &packed, 4).is_err());
    }
}
