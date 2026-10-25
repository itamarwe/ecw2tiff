//! Entropy decoding of coefficient streams (docs/SPEC.md section 5).
//!
//! A [`Stream`] turns the bytes of one subband stream (or the v1/v2 tile
//! directory) into 16-bit coefficients, one row at a time.

use super::io::ByteSource;
use super::{Error, Result};

fn truncated() -> Error {
    Error::malformed("coefficient stream runs past the end of its tile")
}

fn zero_length_run() -> Error {
    Error::malformed("zero run of length 0")
}

fn end_symbol() -> Error {
    Error::malformed("end-of-data symbol inside arithmetic-coded data")
}

/// A decoder for one stream, positioned at its next coefficient.
pub(crate) enum Stream<'f> {
    /// All coefficients are zero (encoding 5, or an empty tile).
    Zero,
    /// Encoding 1: little-endian i16 values.
    Raw(ByteSource<'f>),
    /// Encoding 6: little-endian coefficient words.
    Words { src: ByteSource<'f>, pending: u32 },
    /// Encoding 2: prefix-coded coefficient words.
    Prefix(Box<PrefixStream<'f>>),
    /// Encoding 3: arithmetic-coded i16 values, low byte first.
    Arith16(Box<ArithStream<'f>>),
    /// Encoding 4: arithmetic-coded byte deltas.
    ArithDelta(Box<ArithStream<'f>>, i16),
}

impl<'f> Stream<'f> {
    /// Read the encoding id and any per-stream preamble.
    pub(crate) fn open(mut src: ByteSource<'f>) -> Result<Stream<'f>> {
        let id = src.next().ok_or_else(|| {
            if src.failed() {
                Error::new(super::ErrorKind::Io, "I/O error while reading tile")
            } else {
                Error::malformed("stream has no encoding byte")
            }
        })?;
        let stream = match id {
            1 => Stream::Raw(src),
            5 => Stream::Zero,
            6 => Stream::Words { src, pending: 0 },
            2 => Stream::Prefix(Box::new(PrefixStream::new(src)?)),
            3 => Stream::Arith16(Box::new(ArithStream::new(src))),
            4 => {
                let mut ac = Box::new(ArithStream::new(src));
                let hi = ac.symbol();
                let lo = ac.symbol();
                if hi > 255 || lo > 255 {
                    return Err(end_symbol());
                }
                let z = i16::from_be_bytes([hi as u8, lo as u8]);
                Stream::ArithDelta(ac, z)
            }
            other => {
                return Err(Error::unsupported(format!(
                    "unknown stream encoding {other}"
                )));
            }
        };
        Ok(stream)
    }

    fn failed(&self) -> bool {
        match self {
            Stream::Zero => false,
            Stream::Raw(src) | Stream::Words { src, .. } => src.failed(),
            Stream::Prefix(p) => p.src.failed(),
            Stream::Arith16(a) | Stream::ArithDelta(a, _) => a.src.failed(),
        }
    }

    /// Decode the next `out.len()` coefficients in stream order.
    pub(crate) fn decode(&mut self, out: &mut [i16]) -> Result<()> {
        let res = match self {
            Stream::Zero => {
                out.fill(0);
                Ok(())
            }
            Stream::Raw(src) => {
                for o in out.iter_mut() {
                    *o = src.next_u16le().ok_or_else(truncated)? as i16;
                }
                Ok(())
            }
            Stream::Words { src, pending } => {
                expand_words(out, pending, || src.next_u16le().ok_or_else(truncated))
            }
            Stream::Prefix(p) => {
                let PrefixStream {
                    tree,
                    bits,
                    src,
                    pending,
                } = &mut **p;
                expand_words(out, pending, || Ok(tree.decode(bits, src)))
            }
            Stream::Arith16(a) => a.decode_i16(out),
            Stream::ArithDelta(a, z) => a.decode_deltas(out, z),
        };
        if self.failed() {
            return Err(Error::new(
                super::ErrorKind::Io,
                "I/O error while reading tile",
            ));
        }
        res
    }
}

/// Expand coefficient words (spec 5.2) into `out`. `pending` carries the
/// unfinished part of a zero run between calls.
#[inline]
fn expand_words(
    out: &mut [i16],
    pending: &mut u32,
    mut next_word: impl FnMut() -> Result<u16>,
) -> Result<()> {
    let len = out.len();
    let mut i = (*pending as usize).min(len);
    out[..i].fill(0);
    *pending -= i as u32;
    while i < len {
        let v = next_word()?;
        if v & 0x8000 != 0 {
            let run = (v & 0x7FFF) as usize;
            if run == 0 {
                return Err(zero_length_run());
            }
            let k = run.min(len - i);
            out[i..i + k].fill(0);
            i += k;
            *pending = (run - k) as u32;
        } else {
            out[i] = if v & 0x4000 != 0 {
                -((v & 0x3FFF) as i16)
            } else {
                v as i16
            };
            i += 1;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Prefix code (encoding 2)

/// Marks a tree reference as a leaf; the low 16 bits are its word.
const LEAF: u32 = 1 << 31;

pub(crate) struct PrefixStream<'f> {
    tree: PrefixTree,
    bits: BitReader,
    src: ByteSource<'f>,
    pending: u32,
}

impl<'f> PrefixStream<'f> {
    fn new(mut src: ByteSource<'f>) -> Result<PrefixStream<'f>> {
        let tree = PrefixTree::parse(&mut src)?;
        Ok(PrefixStream {
            tree,
            bits: BitReader { buf: 0, count: 0 },
            src,
            pending: 0,
        })
    }
}

struct BitReader {
    buf: u32,
    count: u32,
}

impl BitReader {
    #[inline(always)]
    fn bit(&mut self, src: &mut ByteSource<'_>) -> usize {
        if self.count == 0 {
            self.buf = u32::from(src.next_or_zero());
            self.count = 8;
        }
        let b = (self.buf & 1) as usize;
        self.buf >>= 1;
        self.count -= 1;
        b
    }
}

/// A binary code tree. `root` and the entries of `inner` are references:
/// either `LEAF | word` or the index of an inner node in `inner`.
struct PrefixTree {
    root: u32,
    inner: Vec<[u32; 2]>,
}

impl PrefixTree {
    /// Parse a pre-order serialised tree without recursion.
    fn parse(src: &mut ByteSource<'_>) -> Result<PrefixTree> {
        let max_nodes = src.next_u16le().ok_or_else(truncated)? as usize;
        let mut inner: Vec<[u32; 2]> = Vec::new();
        // Inner nodes whose children are still being parsed, with the
        // number of children attached so far.
        let mut open: Vec<(usize, usize)> = Vec::new();
        let mut parsed = 0usize;
        let mut root = None;
        loop {
            if parsed >= max_nodes {
                return Err(Error::malformed("prefix code tree has too many nodes"));
            }
            parsed += 1;
            let t = src.next().ok_or_else(truncated)?;
            let node = if t == 0 {
                inner.push([LEAF, LEAF]);
                (inner.len() - 1) as u32
            } else if t & 0x40 != 0 {
                let t = u16::from(t);
                LEAF | u32::from(((t & 0x30) << 10) | (t & 0x0F))
            } else {
                LEAF | u32::from(src.next_u16le().ok_or_else(truncated)?)
            };
            match open.last_mut() {
                None => root = Some(node),
                Some((parent, filled)) => {
                    if let Some(slot) = inner.get_mut(*parent).and_then(|n| n.get_mut(*filled)) {
                        *slot = node;
                    }
                    *filled += 1;
                }
            }
            if node & LEAF == 0 {
                open.push((node as usize, 0));
            } else {
                while open.last().is_some_and(|&(_, filled)| filled == 2) {
                    open.pop();
                }
                if open.is_empty() {
                    break;
                }
            }
        }
        Ok(PrefixTree {
            root: root.unwrap_or(LEAF),
            inner,
        })
    }

    #[inline]
    fn decode(&self, bits: &mut BitReader, src: &mut ByteSource<'_>) -> u16 {
        let mut r = self.root;
        while r & LEAF == 0 {
            let b = bits.bit(src);
            r = match self.inner.get(r as usize) {
                Some(n) => n[b],
                None => LEAF,
            };
        }
        r as u16
    }
}

// ---------------------------------------------------------------------------
// Adaptive arithmetic code (encodings 3 and 4)

const TOTAL: u32 = 4096;
const SYMBOLS: usize = 257;
const END_SYMBOL: u32 = 256;

/// The adaptive frequency model of spec 5.7.2.
struct Model {
    /// Cumulative table `C[0..=257]`.
    cum: [u32; SYMBOLS + 1],
    /// Growing counts `G[0..=256]`.
    counts: [u16; SYMBOLS],
    /// `lookup[t >> 4]` is the symbol whose interval contains `t & !15`.
    lookup: [u16; 256],
    interval: u32,
    increment: u16,
    countdown: i32,
    carry: u32,
}

impl Model {
    fn new() -> Model {
        let mut counts = [16u16; SYMBOLS];
        for c in counts.iter_mut().skip(241) {
            *c = 15;
        }
        let mut m = Model {
            cum: [0; SYMBOLS + 1],
            counts,
            lookup: [0; 256],
            interval: 18,
            increment: 0,
            countdown: 0,
            carry: 0,
        };
        m.adjust();
        m
    }

    #[cold]
    fn adjust(&mut self) {
        if self.carry != 0 {
            self.increment = self.increment.wrapping_add(1);
            self.countdown = self.carry as i32;
            self.carry = 0;
            return;
        }
        if self.interval < 2000 {
            self.interval = (self.interval * 2).min(2000);
        }
        // Rebuild the cumulative table from the counts.
        let mut suffix = 0u32;
        for x in (1..SYMBOLS).rev() {
            suffix = suffix.wrapping_add(u32::from(self.counts[x]));
            self.cum[x] = TOTAL.wrapping_sub(suffix);
        }
        self.cum[0] = 0;
        self.cum[SYMBOLS] = TOTAL;
        // Search accelerator.
        let mut x = 0usize;
        for (q, slot) in self.lookup.iter_mut().enumerate() {
            let t = (q as u32) << 4;
            while x < SYMBOLS - 1 && self.cum[x + 1] <= t {
                x += 1;
            }
            *slot = x as u16;
        }
        // Halve the counts, keeping them odd.
        let mut sum = 0u32;
        for c in self.counts.iter_mut() {
            *c = (*c >> 1) | 1;
            sum += u32::from(*c);
        }
        let m = TOTAL.saturating_sub(sum);
        self.increment = (m / self.interval) as u16;
        self.carry = m % self.interval;
        self.countdown = (self.interval - self.carry) as i32;
    }

    #[inline(always)]
    fn update(&mut self, x: usize) {
        if self.countdown <= 0 {
            self.adjust();
        }
        self.countdown -= 1;
        if let Some(c) = self.counts.get_mut(x) {
            *c = c.wrapping_add(self.increment);
        }
    }
}

/// Arithmetic decoder (spec 5.7.1) together with its model.
pub(crate) struct ArithStream<'f> {
    src: ByteSource<'f>,
    coder: Coder,
    model: Model,
}

/// Arithmetic decoder registers; copied into locals by the row loops.
#[derive(Clone, Copy)]
struct Coder {
    low: u32,
    width: u32,
    cur: u32,
}

impl Coder {
    /// Decode one symbol in `0..=256`.
    #[inline(always)]
    fn symbol(&mut self, src: &mut ByteSource<'_>, m: &mut Model) -> u32 {
        while self.width <= 1 << 23 {
            self.low = (self.low << 8) | ((self.cur << 7) & 0xFF);
            self.cur = u32::from(src.next_or_zero());
            self.low |= self.cur >> 1;
            self.width <<= 8;
        }
        let step = self.width >> 12;
        let t = (self.low / step).min(TOTAL - 1);
        let mut x = usize::from(m.lookup[(t >> 4) as usize & 255]);
        while x < SYMBOLS - 1 && m.cum[x + 1] <= t {
            x += 1;
        }
        let lo = m.cum[x];
        let hi = m.cum[x + 1];
        self.low = self.low.wrapping_sub(step.wrapping_mul(lo));
        self.width = if hi < TOTAL {
            step.wrapping_mul(hi.wrapping_sub(lo))
        } else {
            self.width.wrapping_sub(step.wrapping_mul(lo))
        };
        m.update(x);
        x as u32
    }
}

impl<'f> ArithStream<'f> {
    fn new(mut src: ByteSource<'f>) -> ArithStream<'f> {
        let _ = src.next_or_zero();
        let cur = u32::from(src.next_or_zero());
        ArithStream {
            src,
            coder: Coder {
                low: cur >> 1,
                width: 128,
                cur,
            },
            model: Model::new(),
        }
    }

    fn symbol(&mut self) -> u32 {
        self.coder.symbol(&mut self.src, &mut self.model)
    }

    fn decode_i16(&mut self, out: &mut [i16]) -> Result<()> {
        let mut c = self.coder;
        let (src, m) = (&mut self.src, &mut self.model);
        let mut res = Ok(());
        for o in out.iter_mut() {
            let lo = c.symbol(src, m);
            let hi = c.symbol(src, m);
            if (lo | hi) > 255 {
                res = Err(end_symbol());
                break;
            }
            *o = i16::from_le_bytes([lo as u8, hi as u8]);
        }
        self.coder = c;
        res
    }

    fn decode_deltas(&mut self, out: &mut [i16], z: &mut i16) -> Result<()> {
        let mut c = self.coder;
        let (src, m) = (&mut self.src, &mut self.model);
        let mut v = *z;
        let mut res = Ok(());
        for o in out.iter_mut() {
            let d = c.symbol(src, m);
            if d == END_SYMBOL {
                res = Err(end_symbol());
                break;
            }
            v = v.wrapping_add(i16::from(d as u8 as i8));
            *o = v;
        }
        *z = v;
        self.coder = c;
        res
    }
}
