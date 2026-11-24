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
    // C that has no Rust form yet (variadic functions): see c/lexer_log.c.
    let extra = manifest.join("c");
    for e in fs::read_dir(&extra).unwrap() {
        let p = e.unwrap().path();
        println!("cargo::rerun-if-changed={}", p.display());
        build.file(p);
    }
    build.compile("tree-sitter-c-units");

    // Test shims (layout and header-only helpers); tiny, and dead-stripped from real binaries.
    let shim = manifest.join("shim");
    for e in fs::read_dir(&shim).unwrap() {
        println!("cargo::rerun-if-changed={}", e.unwrap().path().display());
    }
    cc::Build::new()
        .cargo_metadata(false)
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
        .define("ts_lexer_init", "c_ref_lexer_init")
        .define("ts_lexer_delete", "c_ref_lexer_delete")
        .define("ts_lexer_set_input", "c_ref_lexer_set_input")
        .define("ts_lexer_reset", "c_ref_lexer_reset")
        .define("ts_lexer_start", "c_ref_lexer_start")
        .define("ts_lexer_finish", "c_ref_lexer_finish")
        .define("ts_lexer_mark_end", "c_ref_lexer_mark_end")
        .define(
            "ts_lexer_set_included_ranges",
            "c_ref_lexer_set_included_ranges",
        )
        .define("ts_lexer_included_ranges", "c_ref_lexer_included_ranges")
        .file(src.join("lexer.c"))
        .define("ts_language_copy", "c_ref_language_copy")
        .define("ts_language_delete", "c_ref_language_delete")
        .define("ts_language_symbol_count", "c_ref_language_symbol_count")
        .define("ts_language_state_count", "c_ref_language_state_count")
        .define("ts_language_supertypes", "c_ref_language_supertypes")
        .define("ts_language_subtypes", "c_ref_language_subtypes")
        .define("ts_language_abi_version", "c_ref_language_abi_version")
        .define("ts_language_metadata", "c_ref_language_metadata")
        .define("ts_language_name", "c_ref_language_name")
        .define("ts_language_field_count", "c_ref_language_field_count")
        .define("ts_language_table_entry", "c_ref_language_table_entry")
        .define(
            "ts_language_lex_mode_for_state",
            "c_ref_language_lex_mode_for_state",
        )
        .define(
            "ts_language_is_reserved_word",
            "c_ref_language_is_reserved_word",
        )
        .define(
            "ts_language_symbol_metadata",
            "c_ref_language_symbol_metadata",
        )
        .define("ts_language_public_symbol", "c_ref_language_public_symbol")
        .define("ts_language_next_state", "c_ref_language_next_state")
        .define("ts_language_symbol_name", "c_ref_language_symbol_name")
        .define(
            "ts_language_symbol_for_name",
            "c_ref_language_symbol_for_name",
        )
        .define("ts_language_symbol_type", "c_ref_language_symbol_type")
        .define(
            "ts_language_field_name_for_id",
            "c_ref_language_field_name_for_id",
        )
        .define(
            "ts_language_field_id_for_name",
            "c_ref_language_field_id_for_name",
        )
        .define("ts_lookahead_iterator_new", "c_ref_lookahead_iterator_new")
        .define(
            "ts_lookahead_iterator_delete",
            "c_ref_lookahead_iterator_delete",
        )
        .define(
            "ts_lookahead_iterator_reset_state",
            "c_ref_lookahead_iterator_reset_state",
        )
        .define(
            "ts_lookahead_iterator_language",
            "c_ref_lookahead_iterator_language",
        )
        .define(
            "ts_lookahead_iterator_reset",
            "c_ref_lookahead_iterator_reset",
        )
        .define(
            "ts_lookahead_iterator_next",
            "c_ref_lookahead_iterator_next",
        )
        .define(
            "ts_lookahead_iterator_current_symbol",
            "c_ref_lookahead_iterator_current_symbol",
        )
        .define(
            "ts_lookahead_iterator_current_symbol_name",
            "c_ref_lookahead_iterator_current_symbol_name",
        )
        .file(src.join("language.c"))
        .file(shim.join("lang_ref.c"))
        .file(shim.join("alloc_ref.c"))
        .file(src.join("alloc.c"))
        .compile("tree-sitter-shim");
    // Linked straight into each final binary rather than bundled into the rlib, so that shim
    // objects never end up in the shipped static library.
    println!(
        "cargo::rustc-link-search=native={}",
        env::var("OUT_DIR").unwrap()
    );
    println!("cargo::rustc-link-lib=static:-bundle=tree-sitter-shim");
}
