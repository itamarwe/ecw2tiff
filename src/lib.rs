//! From-scratch ECW (v2/v3) decoder and BigTIFF converter.
//!
//! The crate is primarily the `ecw2tiff` command-line tool (see `main.rs`),
//! but the decode and encode building blocks are exposed here so they can be
//! reused in-process — in particular by the optional Python binding
//! (`--features python`), which decodes map tiles without spawning the CLI.

pub mod ecw3;
pub mod encode;
pub mod entropy;
pub mod fdwt;
pub mod header;
pub mod idwt;
pub mod tiff;

#[cfg(feature = "python")]
mod python;
