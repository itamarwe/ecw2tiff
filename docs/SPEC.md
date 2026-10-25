# ECW raster format: decoding specification

This document describes the on-disk ECW raster format (versions 1, 2 and 3)
and the exact arithmetic needed to decode it bit-for-bit. It describes
*behaviour* only: byte layouts, value semantics, and mathematical
definitions of the output. It deliberately prescribes no program structure,
data structures or naming; implement it however you see fit.

Throughout:

* `u8/u16/u32/u64` are unsigned integers, `i8/i16` signed two's-complement.
* **BE** = big-endian, **LE** = little-endian. The format mixes both; each
  field below says which.
* `f32`/`f64` are IEEE-754 binary32/binary64. All decoding arithmetic that
  produces pixel values is done in `f32`, with round-to-nearest-even, each
  operation rounded separately (no fused multiply-add), evaluated exactly in
  the order and parenthesisation written here. Expressions written
  `a + b + c + d` mean `((a + b) + c) + d`.
* `floor`, `trunc` and integer division `/` on non-negative integers all
  round toward zero.

---------------------------------------------------------------------------

## 1. Concepts

An ECW image of `W × H` pixels and `B` bands is stored as a wavelet pyramid
with `L ≥ 1` **stages**, numbered `0 … L-1` from the coarsest to the finest.

* Stage `s` has a grid size `(w_s, h_s)`. The grid sizes grow toward the
  finest stage, and each stage's grid is (in valid files) the ceiling-half of
  the next finer grid. The "next finer grid" of stage `L-1` is the image
  itself (`W × H`).
* Every stage stores, per band, three **detail subbands** (numbered 1, 2, 3),
  each a `w_s × h_s` plane of quantized 16-bit integer coefficients. Stage 0
  additionally stores subband 0, the **base** (low-pass) plane, also
  `w_0 × h_0`.
* Reconstructing stage `s` combines a `w_s × h_s` base plane with that
  stage's three detail planes and yields a plane of the next finer grid's size.
  The base plane for stage 0 is its stored subband 0; the base plane for stage
  `s > 0` is the reconstruction output of stage `s-1`. Reconstructing every
  stage in order yields the full-resolution image.
* Each band is reconstructed independently. Bands are then converted to
  output pixels by a colour rule chosen by the file's **colour model**.

Subband planes are cut into **tiles** of `TW × TH` coefficients (the same
tile size for all stages). Tile `(tx, ty)` of stage `s` covers columns
`tx·TW … min(w_s, (tx+1)·TW) - 1` and rows `ty·TH … min(h_s, (ty+1)·TH) - 1`.
So stage `s` has `ceil(w_s/TW) × ceil(h_s/TH)` tiles; the last tile column and
row may be narrower or shorter. A tile holds all the subbands of all bands for
its rectangle.

Tiles are numbered globally: all tiles of stage 0, then all tiles of stage 1,
and so on. Within a stage, numbering is row-major (`ty · tiles_across + tx`).

## 2. Colour models

A one-byte code in the header:

| code | model         | output channels | conversion                                  |
|------|---------------|-----------------|---------------------------------------------|
| 0    | unspecified   | min(B, 3)       | bands 0.. copied, no transform              |
| 1    | greyscale     | 1               | band 0                                      |
| 2    | YCbCr         | 3               | bands 0,1,2 = Y, Cb, Cr → RGB (section 8)   |
| 3    | multiband     | min(B, 3)       | bands 0.. copied, no transform              |
| 4    | RGB           | min(B, 3)       | bands 0.. copied, no transform              |

Any other code is an error, as is the YCbCr model with fewer than 3 bands.
Bands beyond those used for output (for example
the fourth, opacity band of an ECW v3 YCbCr+alpha file) are still present in
the tile data and must be parsed past, but are not output.

## 3. ECW version 1 / 2 file layout

A v1/v2 file begins with the byte `0x65` (`'e'`). Offsets are from the start
of the file.

| offset | size | field                                                           |
|--------|------|-----------------------------------------------------------------|
| 0      | 1    | `0x65`                                                          |
| 1      | 1    | version: 1 or 2 (anything else: unsupported)                    |
| 2      | 1    | unused                                                          |
| 3      | 1    | colour model code (section 2)                                   |
| 4      | 1    | stage count `L` (must be ≥ 1)                                   |
| 5      | 1    | subbands per stage; must be 4                                   |
| 6      | 4    | `W`, u32 BE                                                     |
| 10     | 4    | `H`, u32 BE                                                     |
| 14     | 2    | `B`, u16 BE (must be ≥ 1)                                       |
| 16     | 2    | quantization divisor `Q`, u16 BE (must be ≠ 0)                  |
| 18     | 2    | `TW`, u16 BE (must be ≠ 0)                                      |
| 20     | 2    | `TH`, u16 BE (must be ≠ 0)                                      |

Version 2 continues with georeferencing (absent in version 1):

| offset | size | field                                                           |
|--------|------|-----------------------------------------------------------------|
| 22     | 2    | target compression ratio, u16 BE (informational)                |
| 24     | 1    | cell unit code, u8 (informational; 1 = metres, 2 = degrees, 3 = feet) |
| 25     | 8    | cell size x, f64 **LE**                                         |
| 33     | 8    | cell size y, f64 LE (normally negative)                         |
| 41     | 8    | origin x (top-left corner), f64 LE                              |
| 49     | 8    | origin y, f64 LE                                                |
| 57     | 16   | datum name, ASCII, NUL-padded (take bytes up to the first NUL)  |
| 73     | 16   | projection name, ASCII, NUL-padded                              |

For version 1 files report: ratio 1, unit code 0, cell size (1, 1), origin
(0, 0), datum and projection `"RAW"`. Version 2 files store `Q = 1` in
practice; version 1 files may use other values.

### 3.1 Stage records

Immediately after the header (offset 22 for v1, 89 for v2) come `L` stage
records, for stage `s = 0 … L-1` in order:

| size  | field                                                         |
|-------|---------------------------------------------------------------|
| 1     | stage index; must equal `s`                                   |
| 4     | `w_s`, u32 BE; must be `< W`                                  |
| 4     | `h_s`, u32 BE; must be `< H`                                  |
| 4·B   | per band `b`, the quantization step `q_{s,b}`, u32 BE         |

### 3.2 Tile directory

Next:

| size      | field                                                          |
|-----------|----------------------------------------------------------------|
| 4         | `P`, u32 BE: number of bytes that follow in this directory (≥ 1) |
| 1         | encoding id (section 5), applied to the directory bytes        |
| `P - 1`   | encoded directory payload                                     |

Let `N` be the total tile count over all stages. Decoding the payload with the
given encoding (section 5.7) must produce exactly `8·(N+1)` bytes, read as
`N+1` u64 **LE** values `o_0 … o_N`. Let `D` be the file offset of the first
byte after the directory (the start of tile data). Tile `i` occupies file
bytes `[D + o_i, D + o_{i+1})`. The sequence must be non-decreasing and
`D + o_N` must not exceed the file length. A tile of length 0 is an
**empty tile**: all its coefficients are zero.

## 4. ECW version 3 file layout

A v3 file starts with the bytes `00 00 00 10` and is a sequence of boxes in
the ISO base-media style:

* A box starts with a u32 BE length (covering the whole box including this
  header) and a 4-byte ASCII type. If the length is 1, a u64 BE length
  follows and the payload starts 16 bytes into the box; otherwise the
  payload starts 8 bytes in. A length of 0 means the box extends to the end of
  the enclosing range (end of file at top level).
* Boxes follow each other back to back. A box that does not end strictly after
  where it starts is an error (it would loop forever). A box that would not
  fit 8 header bytes ends the scan.

Top-level boxes of interest (ignore all others):

### 4.1 `ecwh` (header container)

Its payload is itself a box sequence. The sub-box `ihdr` holds (offsets
within the `ihdr` payload; the payload must be at least 22 bytes):

| offset | size | field                                       |
|--------|------|---------------------------------------------|
| 0      | 2    | unknown                                     |
| 2      | 1    | colour model code                           |
| 3      | 4    | `H`, u32 BE  (note: height before width)    |
| 7      | 4    | `W`, u32 BE                                 |
| 11     | 2    | `B`, u16 BE                                 |
| 13     | 1    | `L`                                         |
| 14     | 1    | subbands per stage; must be 4               |
| 15     | 2    | `TW`, u16 BE                                |
| 17     | 2    | `TH`, u16 BE                                |
| 19     | 1    | unknown                                     |
| 20     | 2    | target compression ratio, u16 BE            |

`L`, `B`, `TW`, `TH` must be non-zero. `Q = 1` for v3.

### 4.2 `gtif` (embedded GeoTIFF tags, optional)

Payload: skip the first 5 bytes, then a sequence of records, each
`u16 BE tag, u32 BE byte length, payload`. Stop at the first record that does
not fit. Recognised tags:

* **33550** (pixel scale), payload ≥ 16 bytes: cell size x = f64 LE at +0,
  cell size y = f64 LE at +8.
* **33922** (tie point), payload ≥ 48 bytes: origin x = f64 LE at +24,
  origin y = f64 LE at +32.
* **34735** (geo-key directory): payload is u16 LE values grouped in fours;
  skip the first group (directory header). For each later group
  `(key, location, count, value)`:
  * key 1024: the image is geographic iff `value == 2`;
  * key 2048 or 3072, with `location == 0` and `value ∉ {0, 32767}`: EPSG
    code = `value` (a later matching group overrides an earlier one).

Defaults when absent: origin (0, 0), cell size (1, -1), not geographic, no
EPSG code.

Report for v3: version 3, cell unit code 2 if geographic else 1, cell size y
forced negative (negate it if positive), datum `""`, projection
`"EPSG:<code>"` if a code was found else `"RAW"`, plus the EPSG code itself.

### 4.3 `ecwc` (codestream)

Its payload starts with the same `L` stage records as section 3.1. Then:

| size     | field                                                           |
|----------|-----------------------------------------------------------------|
| 4        | u32 BE length (not needed)                                      |
| 1        | encoding id; only 1 (raw) is supported, anything else is unsupported |
| 12·N     | per tile: u64 LE **absolute** file offset, u32 LE byte length   |

The entries must fit inside the `ecwc` payload. A non-zero-length tile must
lie inside the file. Length 0 means an empty tile. Tile contents are
identical in structure to v1/v2 tiles.

`ihdr` and `ecwc` are mandatory.

## 5. Tile contents and entropy encodings

### 5.1 Tile layout

Let `k` be the number of subbands per band in the tile's stage (4 for stage
0, i.e. subbands 0,1,2,3; 3 otherwise, i.e. subbands 1,2,3) and
`n = B·k` the number of **streams** in the tile. Streams are ordered
band-major: all subbands of band 0 in ascending subband number, then band 1,
and so on.

A non-empty tile begins with `n - 1` u32 BE values `a_1 … a_{n-1}`. Let
`T = 4·(n-1)` (the size of that list). Stream 0 starts at tile offset `T`;
stream `j ≥ 1` starts at tile offset `T + a_j`. A tile shorter than `T`
bytes is an error.

Each stream begins with one **encoding id** byte, followed by its payload.
A stream encodes the subband's `tw × th` coefficients for the tile's
rectangle (`tw`, `th` = that tile's actual width and height), in row-major
order. For decoding purposes a stream's bytes extend to the end of the tile
(decoders that read ahead see the following streams' bytes; that is
expected); reads beyond the end of the tile behave as described per encoding.

### 5.2 Coefficient words

Several encodings use a 16-bit **coefficient word** `v`:

* if bit 15 (`0x8000`) is set: a **zero run** of length `v & 0x7FFF`
  (that many consecutive zero coefficients). Length 0 is an error.
* else if bit 14 (`0x4000`) is set: the coefficient `-(v & 0x3FFF)`.
* else: the coefficient `v` (0 … 0x3FFF).

A zero run may extend across row boundaries of the tile; it simply covers
the next coefficients in row-major order.

### 5.3 Encoding 1: raw

The payload is the coefficients as i16 LE, `tw·th` of them. Reading past the
end of the tile is an error.

### 5.4 Encoding 5: empty

No payload; all coefficients are zero.

### 5.5 Encoding 6: zero-run words

The payload is a sequence of coefficient words, each u16 LE (section 5.2),
covering the `tw·th` coefficients. Reading past the end of the tile is an
error.

### 5.6 Encoding 2: prefix code

The payload is a code tree followed by a bit stream.

**Tree.** A u16 LE value `M`, the maximum number of tree nodes, then the
nodes in pre-order. Each node starts with one byte `t`:

* `t == 0`: an inner node. Its first child subtree follows, then its second
  child subtree.
* `t != 0`: a leaf carrying a coefficient word `v`:
  * if `t & 0x40`: `v = ((t & 0x30) << 10) | (t & 0x0F)`, i.e. bits 5 and 4
    of `t` become bits 15 and 14 of `v`, bits 3–0 stay in place, and nothing
    more is read;
  * otherwise the next two bytes are `v` as u16 LE.

The tree is complete when the root's subtree is complete (a tree may be a
single leaf). Parsing more than `M` nodes, or running off the end of the
tile, is an error. Implementations must not use unbounded recursion for
this (a hostile file can make the tree arbitrarily deep).

**Bits.** The bit stream starts at the first byte after the tree. Bits are
consumed least-significant-bit first within each byte, bytes in order. Bits
beyond the end of the tile read as 0.

**Symbols.** To decode one coefficient word, start at the root; while at an
inner node read one bit and go to the first child on 0, the second child on 1;
the leaf reached gives `v` (a single-leaf tree consumes no bits). Interpret
`v` per section 5.2. Decode words until the `tw·th` coefficients are
covered.

### 5.7 Encodings 3 and 4: adaptive arithmetic code

Both use the same arithmetic decoder (5.7.1) driven by the same adaptive
probability model (5.7.2), which deliver a sequence of **symbols** in
`0 … 256`. Symbols 0–255 are data bytes; symbol 256 marks the end of data
(an encoder may append it). Bytes past the end of the tile read as `0x00`.

#### 5.7.1 Arithmetic decoder

State: two u32 values `low` and `width`, a byte `cur`, and a read position.
Arithmetic on `low` and `width` is modulo 2³².

*Start* (at the first payload byte `p`): ignore byte `p`; `cur` = byte
`p+1`; `low = cur >> 1`; `width = 128`; the next byte to read is `p+2`.

*Refill* (performed at the start of every symbol decode): while
`width ≤ 2²³`:
  `low = (low << 8) | ((cur << 7) & 0xFF)`; `cur` = next byte;
  `low = low | (cur >> 1)`; `width = width << 8`.

*Decode one symbol*, with the model's cumulative table `C` (5.7.2), whose
total `C[257]` is always 4096:

1. Refill.
2. `step = width >> 12`; `t = low / step` (integer division); if
   `t ≥ 4096` then `t = 4095`.
3. The symbol is the unique `x` with `C[x] ≤ t < C[x+1]`.
4. `low = low - step·C[x]`; then if `C[x+1] < 4096`,
   `width = step·(C[x+1] - C[x])`, otherwise `width = width - step·C[x]`.
5. Update the model with `x` (5.7.2).

#### 5.7.2 Adaptive model

Each stream (and each tile directory) starts a fresh model. The model has:
the cumulative table `C[0 … 257]` (`C[0] = 0`, `C[257] = 4096`), an array
`G[0 … 256]` of u16 *growing counts*, and integer counters: an **interval**
`R`, an **increment** `δ`, a **countdown** `κ`, and a **carry** `ρ`.

*Initialise:* `R = 18`, `δ = 0`, `κ = 0`, `ρ = 0`. Set `G[x] = 16` for
`x < 241` and `G[x] = 15` for `241 ≤ x ≤ 256` (4096 split as evenly as
possible over 257 symbols, the remainder going to the lowest symbols). Then
perform one *adjustment*.

*Adjustment:*

* If `ρ ≠ 0`: `δ = δ + 1`, `κ = ρ`, `ρ = 0`, and stop (the table is not
  rebuilt).
* Otherwise:
  1. If `R < 2000`, `R = min(2·R, 2000)`.
  2. Rebuild the table from the current counts: for `x = 256` down to `1`,
     `C[x] = 4096 - (G[x] + G[x+1] + … + G[256])` (`C[0]` stays 0, `C[257]`
     stays 4096).
  3. Halve every count, keeping it odd and positive:
     `G[x] = (G[x] >> 1) | 1` for all `x` in `0 … 256`.
  4. Let `m = 4096 - (sum of the new G)`. Set `δ = m / R`,
     `ρ = m mod R`, `κ = R - ρ`.

*Update with symbol `x`:* if `κ ≤ 0`, perform an adjustment. Then
`κ = κ - 1` and `G[x] = G[x] + δ` (u16, wrapping).

(Note the table `C` used for decoding changes only at rebuilds; the counts
`G` accumulate in between.)

#### 5.7.3 Encoding 3: arithmetic, 16-bit

The decoded data bytes are the coefficients as i16 **LE**, i.e. two symbols
per coefficient, low byte first. Decode `2·tw·th` symbols. Symbol 256 where a
data byte is required is an error.

#### 5.7.4 Encoding 4: arithmetic, byte deltas

The first two symbols are the high byte then the low byte of a 16-bit
starting value `z` (as i16). Then each coefficient is one symbol `d`,
interpreted as an i8 (two's complement of the byte); `z = z + d`
(16-bit wrapping) and the coefficient is the new `z`. The running value
continues across rows. Symbol 256 where a delta is required is an error.

#### 5.7.5 Encodings applied to the tile directory (v1/v2)

The directory payload is decoded into the required number of bytes:

* 1 (raw): the bytes themselves (error if too few).
* 5 (empty): all zero bytes.
* 6, 2: coefficient words / prefix code as in 5.5 / 5.6, producing i16
  values written as LE byte pairs, until the required length is covered.
* 3: one data byte per symbol, in order.
* 4: starting value and deltas as in 5.7.4, each value written as an i16 LE
  byte pair.

For the directory, "end of tile" in the rules above means the end of the
`P - 1` payload bytes.

Any other encoding id (in a stream or the directory) is an error.

### 5.8 Dequantization

Coefficient `c` (an i16) of a band-`b` stream in stage `s` becomes the `f32`

    value = f32(c) × (f32(q_{s,b}) / f32(Q))

(first the step division in f32, then the product in f32).

## 6. Reconstruction of one stage

Let the stage's input planes be, for one band, `S0` (base) and `S1`, `S2`,
`S3` (details), all `w × h`, and let the output size be `w' × h'` (the next
finer grid). Define the clamped read

    S_k⟨r, c⟩ = S_k[ clamp(r, 0, h-1) ][ clamp(c, 0, w-1) ]

(row `r`, column `c`; i.e. edge values are replicated one sample beyond each
border).

For every output position (row `Y`, column `X`), `0 ≤ Y < h'`, `0 ≤ X < w'`:

* if `Y` is even: `r₀ = Y/2 - 1`, `r₁ = Y/2`; if `Y` is odd:
  `r₀ = (Y-1)/2`, `r₁ = (Y-1)/2 + 1`.
* if `X` is even: `c_a = X/2 - 1`, `c_b = X/2`; if `X` is odd:
  `c_a = (X-1)/2`, `c_b = (X-1)/2 + 1`.

Write `kRC` for `S_k⟨r_R, c_C⟩`, e.g. `2 1a` = `S2⟨r₁, c_a⟩`,
`0 0b` = `S0⟨r₀, c_b⟩`. Then, in f32, with `½ = 0.5` and `¼ = 0.25`:

**Y even, X even**

    out = 0 1b  -  (1 0b + 1 1b + 2 1a + 2 1b) × ½  +  (3 0a + 3 0b + 3 1a + 3 1b) × ¼

**Y even, X odd**

    out = ((0 1a + 0 1b) - (3 0a + 3 1a)) × ½  -  (1 0a + 1 0b + 1 1a + 1 1b) × ¼  +  2 1a

**Y odd, X even**

    out = ((0 0b + 0 1b) - (3 0a + 3 0b)) × ½  +  1 0b  -  (2 0a + 2 0b + 2 1a + 2 1b) × ¼

**Y odd, X odd**

    out = (0 0a + 0 0b + 0 1a + 0 1b) × ¼  +  (1 0a + 1 0b + 2 0a + 2 1a) × ½  +  3 0a

The top-level operators are applied left to right; for example the first
formula is `(A - B×½) + C×¼` with `A`, `B`, `C` the grouped terms, and each
four-term group is summed left to right as written.

The full-resolution band is obtained by reconstructing stage 0 (base = its
stored subband 0), feeding its output as the base of stage 1, and so on up to
stage `L-1`, whose output is `W × H`. Write `G_j` for the plane produced
when stopping `j` stages short of the top: `G_0` is the full-resolution plane
(output of stage `L-1`), `G_1` is the output of stage `L-2` (size
`w_{L-1} × h_{L-1}`), …, `G_{L-1}` is the output of stage 0 (size
`w_1 × h_1`, or `W × H` if `L = 1`).

The values are defined for the whole plane. A decoder that only needs part
of a plane may compute only the necessary samples, but the values must be
identical to those of the whole-plane definition (the clamping applies only
at the true plane borders).

## 7. Windowed and reduced-resolution decoding

A decode request names an inclusive pixel window `[x0, x1] × [y0, y1]` of
the full-resolution image (`x0 ≤ x1 < W`, `y0 ≤ y1 < H`) and an output size
`nx × ny` with `1 ≤ nx ≤ x1-x0+1` and `1 ≤ ny ≤ y1-y0+1`. Anything else is
an invalid request.

1. **Choose the plane.** Start with `j = 0` and the window `(x0, x1, y0, y1)`.
   While `(y1 - y0 + 1) > 2·ny` **and** `(x1 - x0 + 1) > 2·nx` **and**
   `j < L - 1`: replace each of `x0, x1, y0, y1` by itself integer-divided
   by 2, and increment `j`. The output samples are taken from plane `G_j`
   using the resulting window.
2. **Degenerate axes.** If `nx = 1`: `x0 = x0 + (x1 - x0)/2`, `x1 = x0`.
   If `ny = 1`: likewise for `y0, y1`.
3. Let `ow = x1 - x0 + 1`, `oh = y1 - y0 + 1`.
4. **Columns.** `F = trunc_to_u64( (f64(ow) / f64(nx)) × 65536.0 ) × 65536`
   (an integer). Output column `i` (`0 ≤ i < nx`) reads plane column
   `x0 + ((i × F) >> 32)`, computed exactly in integers (≥ 64 bits).
5. **Rows.** Output row `jr` (`0 ≤ jr < ny`) reads plane row
   `trunc( f64(y0) + (f64(oh) / f64(ny)) × f64(jr) )`, computed in f64 as
   written (the product first, then the sum; compute each row's value afresh
   from `jr`, do not accumulate).

## 8. Output pixels

For each output position the per-band f32 values are converted to 8-bit
samples with `u8(v) = clamp( round_half_to_even(v), 0, 255 )`.

* greyscale: one channel, `u8(band0)`.
* YCbCr, with `Yv = band0`, `Cb = band1`, `Cr = band2`, in f32:

      R = Yv + 1.402 × Cr
      G = (Yv + (-0.34414) × Cb) + (-0.71414) × Cr
      B = Yv + 1.772 × Cb

  (constants rounded to f32), output `u8(R), u8(G), u8(B)`.
* other models: `min(B, 3)` channels, `u8(band_i)`.

Output rows are interleaved samples: for each pixel, its channels in order.

## 9. Behaviour on bad input

Files may be truncated, corrupt or hostile. A decoder must never panic,
loop forever, overflow the stack, or allocate memory far beyond what the file
size justifies (for example, check that header-derived counts are plausible
for the file length before allocating). Malformed input must produce an
error. Exact error messages are free-form.
