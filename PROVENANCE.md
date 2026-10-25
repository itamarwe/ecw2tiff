# Decoder provenance

The ECW decoder in `src/codec/` (ecw2tiff 0.2 and later) is a clean-room
implementation. It was written from a behavioural format specification,
without access to the source code of the ECW SDK or of any other ECW
implementation. It replaces the decoder shipped in ecw2tiff 0.1.x.

It is verified to decode bit-identically to the previous release across the
project's test corpus.

The ECW patents (US 6,201,897 and US 6,442,298) have expired.
