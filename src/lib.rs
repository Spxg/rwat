//! Parse annotated wat into wasm object files with `linking` and `reloc.CODE`
//! custom sections.
//!
//! The main entry point is [`parse_rwat`], which accepts wat source using the
//! `(@rwat)`, `(@sym)`, and `(@reloc)` annotations and returns encoded wasm
//! bytes suitable for linking with tools such as `wasm-ld`.

mod code;
mod emit;
mod link;
mod parse;
mod reloc;
mod scan;
mod types;

mod annotation {
    wast::annotation!(rwat);
}

pub use parse::parse_rwat;
