//! Sideband stream encoders — the compression side of `entropy.rs`.
//!
//! A sideband stream is one format byte followed by a payload holding
//! `rows * line_length` quantized coefficients in row-major order. The
//! decoder supports six encodings; [`encode_sideband`] produces every one
//! this module can emit and keeps the smallest, so adding a format can only
//! ever shrink output.
//!
//! `ENCODE_RUN_ZERO` and `ENCODE_HUFFMAN` share an alphabet, so both are
//! built from the same [`tokenize`] pass:
//!
//! ```text
//!   bit 15 set  -> a run of (value & 0x7fff) zeros, 1..=32767
//!   bit 14 set  -> negative, magnitude (value & 0x3fff)
//!   otherwise   -> a positive value, 0..=16383
//! ```
//!
//! Coefficients outside +/-16383 cannot be expressed in that alphabet; when
//! one appears, only the RAW encoding is offered.

use std::collections::BinaryHeap;

use crate::entropy::{
    ENCODE_HUFFMAN, ENCODE_RAW, ENCODE_RUN_ZERO, ENCODE_ZEROS, MAX_RUN_LENGTH, RUN_MASK, SIGN_MASK,
    VALUE_MASK,
};

/// Largest zero run a single symbol can express.
const MAX_RUN: usize = MAX_RUN_LENGTH as usize; // 32767
/// Largest coefficient magnitude the run/value alphabet can express.
const MAX_VALUE: i32 = VALUE_MASK as i32; // 16383

/// One symbol of the shared RUN_ZERO / HUFFMAN alphabet.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Sym {
    /// A run of N zeros, 1..=32767.
    Run(u16),
    Val(i16),
}

impl Sym {
    /// The 16-bit on-disk form the decoder parses.
    fn word(self) -> u16 {
        match self {
            Sym::Run(n) => RUN_MASK | n,
            Sym::Val(v) if v < 0 => SIGN_MASK | (-(v as i32)) as u16,
            Sym::Val(v) => v as u16,
        }
    }
}

/// Collapse runs of zeros into `Sym::Run`. Returns `None` if any coefficient
/// is outside the alphabet's range, in which case only RAW can encode it.
fn tokenize(values: &[i16]) -> Option<Vec<Sym>> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < values.len() {
        if values[i] == 0 {
            let start = i;
            while i < values.len() && values[i] == 0 {
                i += 1;
            }
            let mut run = i - start;
            while run > 0 {
                let take = run.min(MAX_RUN);
                out.push(Sym::Run(take as u16));
                run -= take;
            }
        } else {
            let v = values[i];
            if (v as i32).abs() > MAX_VALUE {
                return None;
            }
            out.push(Sym::Val(v));
            i += 1;
        }
    }
    Some(out)
}

/// `ENCODE_ZEROS`: the whole stream is zeros, so the format byte is all of it.
fn encode_zeros(values: &[i16]) -> Option<Vec<u8>> {
    values.iter().all(|&v| v == 0).then(|| vec![ENCODE_ZEROS])
}

/// `ENCODE_RAW`: little-endian int16 per coefficient. Always available.
fn encode_raw(values: &[i16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + values.len() * 2);
    out.push(ENCODE_RAW);
    for &v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// `ENCODE_RUN_ZERO`: one 16-bit word per symbol.
fn encode_run_zero(syms: &[Sym]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + syms.len() * 2);
    out.push(ENCODE_RUN_ZERO);
    for s in syms {
        out.extend_from_slice(&s.word().to_le_bytes());
    }
    out
}

// ---------------------------------------------------------------------------
// Huffman
// ---------------------------------------------------------------------------

enum Node {
    Leaf(Sym),
    Internal(Box<Node>, Box<Node>),
}

/// Heap entry ordered by weight (min-heap via `Reverse`-style inversion), with
/// a tiebreaker so tree shape is deterministic across runs.
struct HeapItem {
    weight: u64,
    seq: usize,
    node: Node,
}

impl PartialEq for HeapItem {
    fn eq(&self, other: &Self) -> bool {
        self.weight == other.weight && self.seq == other.seq
    }
}
impl Eq for HeapItem {}
impl Ord for HeapItem {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Reversed: BinaryHeap is a max-heap and we want the smallest weight.
        other
            .weight
            .cmp(&self.weight)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}
impl PartialOrd for HeapItem {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Walk the tree, recording each leaf's root-to-leaf bit path and counting
/// nodes. Iterative: a degenerate tree must not overflow the stack.
fn collect_codes(root: &Node) -> (Vec<(Sym, Vec<bool>)>, usize) {
    let mut codes = Vec::new();
    let mut nodes = 0usize;
    let mut stack = vec![(root, Vec::new())];
    while let Some((n, path)) = stack.pop() {
        nodes += 1;
        match n {
            Node::Leaf(s) => codes.push((*s, path)),
            Node::Internal(a, b) => {
                let mut p1 = path.clone();
                p1.push(true);
                stack.push((b, p1));
                let mut p0 = path;
                p0.push(false);
                stack.push((a, p0));
            }
        }
    }
    (codes, nodes)
}

/// Serialize the tree in the decoder's preorder form: `0` for an internal
/// node followed by its two subtrees, otherwise a leaf. Leaves whose value
/// has bits 13..4 clear pack into a single byte (`0x40 | value bits`);
/// the rest are a marker byte plus a little-endian u16.
fn serialize_tree(root: &Node, out: &mut Vec<u8>) {
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        match n {
            Node::Internal(a, b) => {
                out.push(0);
                stack.push(b);
                stack.push(a);
            }
            Node::Leaf(s) => {
                let value = s.word();
                if value & 0x3ff0 == 0 {
                    out.push(0x40 | (((value >> 14) & 3) << 4) as u8 | (value & 0x0f) as u8);
                } else {
                    out.push(0x01); // any non-zero byte without 0x40 means "u16 follows"
                    out.extend_from_slice(&value.to_le_bytes());
                }
            }
        }
    }
}

/// `ENCODE_HUFFMAN`: serialized tree then an LSB-first bitstream.
///
/// Returns `None` when the stream has fewer than two distinct symbols: a
/// single-leaf tree would encode zero bits per symbol, and the other formats
/// already handle that case better.
fn encode_huffman(syms: &[Sym]) -> Option<Vec<u8>> {
    if syms.is_empty() {
        return None;
    }
    // Histogram. The alphabet is sparse in practice, so a sorted Vec beats a
    // 64K-entry table.
    let mut counts: Vec<(Sym, u64)> = Vec::new();
    {
        let mut idx: std::collections::HashMap<u16, usize> = std::collections::HashMap::new();
        for &s in syms {
            match idx.get(&s.word()) {
                Some(&i) => counts[i].1 += 1,
                None => {
                    idx.insert(s.word(), counts.len());
                    counts.push((s, 1));
                }
            }
        }
    }
    if counts.len() < 2 {
        return None;
    }
    counts.sort_by_key(|&(s, _)| s.word()); // deterministic tree

    let mut heap: BinaryHeap<HeapItem> = counts
        .iter()
        .enumerate()
        .map(|(seq, &(s, w))| HeapItem {
            weight: w,
            seq,
            node: Node::Leaf(s),
        })
        .collect();
    let mut seq = counts.len();
    while heap.len() > 1 {
        let a = heap.pop().unwrap();
        let b = heap.pop().unwrap();
        heap.push(HeapItem {
            weight: a.weight + b.weight,
            seq,
            node: Node::Internal(Box::new(a.node), Box::new(b.node)),
        });
        seq += 1;
    }
    let root = heap.pop().unwrap().node;

    let (codes, n_nodes) = collect_codes(&root);
    if n_nodes > u16::MAX as usize {
        return None; // tree too large to describe in the header's u16
    }
    let mut table: std::collections::HashMap<u16, Vec<bool>> = std::collections::HashMap::new();
    for (s, path) in codes {
        table.insert(s.word(), path);
    }

    let mut out = Vec::with_capacity(1 + 2 + n_nodes + syms.len());
    out.push(ENCODE_HUFFMAN);
    out.extend_from_slice(&(n_nodes as u16).to_le_bytes());
    serialize_tree(&root, &mut out);

    // Bitstream, LSB-first within each byte.
    let mut acc = 0u8;
    let mut nbits = 0u32;
    for s in syms {
        for &bit in &table[&s.word()] {
            if bit {
                acc |= 1 << nbits;
            }
            nbits += 1;
            if nbits == 8 {
                out.push(acc);
                acc = 0;
                nbits = 0;
            }
        }
    }
    if nbits > 0 {
        out.push(acc);
    }
    Some(out)
}

// ---------------------------------------------------------------------------

/// Encode one sideband stream, choosing the smallest available encoding.
///
/// `values` holds `rows * line_length` coefficients in row-major order. The
/// result begins with the format byte, ready to append to a block.
pub fn encode_sideband(values: &[i16]) -> Vec<u8> {
    let mut best = encode_raw(values);
    let mut consider = |cand: Option<Vec<u8>>| {
        if let Some(c) = cand {
            if c.len() < best.len() {
                best = c;
            }
        }
    };
    consider(encode_zeros(values));
    if let Some(syms) = tokenize(values) {
        consider(Some(encode_run_zero(&syms)));
        consider(encode_huffman(&syms));
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entropy::SbDecoder;

    /// Decode an encoded sideband with the real decoder and compare.
    fn assert_round_trips(values: &[i16], line_length: usize, rows: usize, expect_fmt: Option<u8>) {
        let data = encode_sideband(values);
        if let Some(f) = expect_fmt {
            assert_eq!(data[0], f, "unexpected format choice");
        }
        let mut dec = SbDecoder::start(&data, 0, line_length, 0).expect("decoder start");
        let mut got = Vec::with_capacity(values.len());
        let mut line = vec![0f32; line_length];
        for _ in 0..rows {
            dec.decode_line(&data, 0, 0, line_length, &mut line, 1.0)
                .expect("decode line");
            got.extend(line.iter().map(|&v| v as i16));
        }
        assert_eq!(got, values, "format {} round-trip mismatch", data[0]);
    }

    fn pseudo(n: usize, salt: u64, sparsity: u64) -> Vec<i16> {
        (0..n)
            .map(|i| {
                let x = (i as u64 + 1).wrapping_mul(2654435761).wrapping_add(salt);
                if !x.is_multiple_of(sparsity) {
                    0
                } else {
                    ((x >> 8) % 4001) as i16 - 2000
                }
            })
            .collect()
    }

    #[test]
    fn all_zero_uses_zeros_format() {
        assert_round_trips(&[0i16; 64], 8, 8, Some(ENCODE_ZEROS));
        assert_eq!(encode_sideband(&vec![0i16; 4096]).len(), 1);
    }

    #[test]
    fn sparse_data_round_trips() {
        for sparsity in [3u64, 7, 20, 100] {
            let v = pseudo(256, sparsity, sparsity);
            assert_round_trips(&v, 16, 16, None);
        }
    }

    #[test]
    fn dense_data_round_trips() {
        let v: Vec<i16> = (0..256).map(|i| (i as i16 * 37) % 9001 - 4500).collect();
        assert_round_trips(&v, 16, 16, None);
    }

    #[test]
    fn out_of_range_values_fall_back_to_raw() {
        // Beyond the run/value alphabet (+/-16383): RAW is the only option.
        let mut v = vec![0i16; 64];
        v[5] = 20000;
        v[9] = -30000;
        assert_round_trips(&v, 8, 8, Some(ENCODE_RAW));
    }

    #[test]
    fn long_runs_split_across_symbols() {
        // Longer than one Run symbol can hold (32767).
        let mut v = vec![0i16; 40000];
        v[0] = 5;
        v[39999] = -5;
        assert_round_trips(&v, 200, 200, None);
    }

    #[test]
    fn single_distinct_symbol_is_not_huffman() {
        // One symbol would give a zero-bit code; other formats must win.
        let v = vec![7i16; 64];
        let data = encode_sideband(&v);
        assert_ne!(data[0], ENCODE_HUFFMAN);
        assert_round_trips(&v, 8, 8, None);
    }

    #[test]
    fn huffman_wins_on_skewed_data() {
        // Heavily skewed value distribution: Huffman should beat RLE and RAW.
        let v: Vec<i16> = (0..4096)
            .map(|i| match i % 16 {
                0..=11 => 1,
                12..=13 => -1,
                14 => 2,
                _ => 300,
            })
            .collect();
        let data = encode_sideband(&v);
        assert_eq!(data[0], ENCODE_HUFFMAN, "expected Huffman to win");
        assert!(data.len() < 4096, "should beat one byte per sample");
        assert_round_trips(&v, 64, 64, Some(ENCODE_HUFFMAN));
    }

    #[test]
    fn choice_is_never_worse_than_raw() {
        for sparsity in [1u64, 2, 5, 50] {
            let v = pseudo(1024, 11, sparsity);
            assert!(encode_sideband(&v).len() <= encode_raw(&v).len());
        }
    }
}
