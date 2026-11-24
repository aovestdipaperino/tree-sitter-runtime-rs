//! Builds the C units that are not ported yet, each as its own object (ADR-011), with the flags
//! the upstream `tree-sitter` crate uses. `ported.txt` lists the units whose symbols now come
//! from Rust.
use std::{collections::HashSet, env, fs, path::PathBuf};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.join("../..");
    let src = root.join("src");
    let include = root.join("include");

    let ported_list = manifest.join("ported.txt");
    println!("cargo::rerun-if-changed={}", ported_list.display());
    let ported: HashSet<String> = fs::read_to_string(&ported_list)
        .unwrap()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect();

    let mut build = cc::Build::new();
    build
        .std("c11")
        .flag_if_supported("-fvisibility=hidden")
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-incompatible-pointer-types")
        .include(&src)
        .include(&include)
        .define("_POSIX_C_SOURCE", "200112L")
        .define("_DEFAULT_SOURCE", None)
        .define("_BSD_SOURCE", None)
        .define("_DARWIN_C_SOURCE", None)
        .warnings(false);

    let mut files: Vec<_> = fs::read_dir(&src)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".c") && n != "lib.c" && !ported.contains(n))
        .collect();
    files.sort();
    for f in &files {
        println!("cargo::rerun-if-changed={}", src.join(f).display());
        build.file(src.join(f));
    }
    for entry in fs::read_dir(&src).unwrap() {
        println!(
            "cargo::rerun-if-changed={}",
            entry.unwrap().path().display()
        );
    }
    build.compile("tree-sitter-c-units");

    // Test shims (layout and header-only helpers); tiny, and dead-stripped from real binaries.
    let shim = manifest.join("shim");
    for e in fs::read_dir(&shim).unwrap() {
        println!("cargo::rerun-if-changed={}", e.unwrap().path().display());
    }
    cc::Build::new()
        .std("c11")
        .include(&src)
        .include(&include)
        .warnings(false)
        .file(shim.join("helpers.c"))
        .file(shim.join("layout.c"))
        // The C original of each ported unit, symbols renamed, as a reference for tests.
        .define("ts_point_edit", "shim_c_ts_point_edit")
        .file(src.join("point.c"))
        .define("ts_set_allocator", "c_ref_ts_set_allocator")
        .define("ts_current_malloc", "c_ref_current_malloc")
        .define("ts_current_calloc", "c_ref_current_calloc")
        .define("ts_current_realloc", "c_ref_current_realloc")
        .define("ts_current_free", "c_ref_current_free")
        .file(shim.join("alloc_ref.c"))
        .file(src.join("alloc.c"))
        .compile("tree-sitter-shim");
}
