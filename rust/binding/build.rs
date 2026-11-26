//! Only copies the wasm stdlib symbol list that `wasm_stdlib_symbols()` embeds. The C library is
//! not built here: its symbols come from `tree-sitter-runtime-rs`.
use std::{env, fs, path::PathBuf};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let symbols = manifest.join("data/stdlib-symbols.txt");
    println!("cargo::rerun-if-changed={}", symbols.display());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::copy(&symbols, out.join("stdlib-symbols.txt")).unwrap();
}
