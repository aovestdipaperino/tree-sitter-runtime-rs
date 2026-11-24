//! The tree-sitter runtime, ported from C to Rust one unit at a time (see `rust/MAP.md`).
//!
//! Ported units export the same `extern "C"` symbols as the C they replace. Units not ported yet
//! are compiled from `src/` by `build.rs`.
pub mod alloc;
pub mod api;
pub mod array;
pub mod consts;
pub mod language;
pub mod length;
pub mod lexer;
pub mod parser_abi;
pub mod point;
pub mod reduce_action;
pub mod subtree;
pub mod unicode;
