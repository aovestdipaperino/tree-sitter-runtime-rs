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
        .define(
            "ts_external_scanner_state_init",
            "c_ref_external_scanner_state_init",
        )
        .define(
            "ts_external_scanner_state_copy",
            "c_ref_external_scanner_state_copy",
        )
        .define(
            "ts_external_scanner_state_delete",
            "c_ref_external_scanner_state_delete",
        )
        .define(
            "ts_external_scanner_state_data",
            "c_ref_external_scanner_state_data",
        )
        .define(
            "ts_external_scanner_state_eq",
            "c_ref_external_scanner_state_eq",
        )
        .define("ts_subtree_array_copy", "c_ref_subtree_array_copy")
        .define("ts_subtree_array_clear", "c_ref_subtree_array_clear")
        .define("ts_subtree_array_delete", "c_ref_subtree_array_delete")
        .define(
            "ts_subtree_array_remove_trailing_extras",
            "c_ref_subtree_array_remove_trailing_extras",
        )
        .define("ts_subtree_array_reverse", "c_ref_subtree_array_reverse")
        .define("ts_subtree_pool_new", "c_ref_subtree_pool_new")
        .define("ts_subtree_pool_delete", "c_ref_subtree_pool_delete")
        .define("ts_subtree_new_leaf", "c_ref_subtree_new_leaf")
        .define("ts_subtree_set_symbol", "c_ref_subtree_set_symbol")
        .define("ts_subtree_new_error", "c_ref_subtree_new_error")
        .define("ts_subtree_clone", "c_ref_subtree_clone")
        .define("ts_subtree_make_mut", "c_ref_subtree_make_mut")
        .define("ts_subtree_compress", "c_ref_subtree_compress")
        .define(
            "ts_subtree_summarize_children",
            "c_ref_subtree_summarize_children",
        )
        .define("ts_subtree_new_node", "c_ref_subtree_new_node")
        .define("ts_subtree_new_error_node", "c_ref_subtree_new_error_node")
        .define(
            "ts_subtree_new_missing_leaf",
            "c_ref_subtree_new_missing_leaf",
        )
        .define("ts_subtree_retain", "c_ref_subtree_retain")
        .define("ts_subtree_release", "c_ref_subtree_release")
        .define("ts_subtree_compare", "c_ref_subtree_compare")
        .define("ts_subtree_edit", "c_ref_subtree_edit")
        .define(
            "ts_subtree_last_external_token",
            "c_ref_subtree_last_external_token",
        )
        .define("ts_subtree_string", "c_ref_subtree_string")
        .define(
            "ts_subtree_print_dot_graph",
            "c_ref_subtree_print_dot_graph",
        )
        .define(
            "ts_subtree_external_scanner_state",
            "c_ref_subtree_external_scanner_state",
        )
        .define(
            "ts_subtree_external_scanner_state_eq",
            "c_ref_subtree_external_scanner_state_eq",
        )
        .file(src.join("subtree.c"))
        .file(shim.join("subtree_shim.c"))
        .define("ts_stack_new", "c_ref_stack_new")
        .define("ts_stack_delete", "c_ref_stack_delete")
        .define("ts_stack_version_count", "c_ref_stack_version_count")
        .define(
            "ts_stack_halted_version_count",
            "c_ref_stack_halted_version_count",
        )
        .define("ts_stack_state", "c_ref_stack_state")
        .define(
            "ts_stack_last_external_token",
            "c_ref_stack_last_external_token",
        )
        .define(
            "ts_stack_set_last_external_token",
            "c_ref_stack_set_last_external_token",
        )
        .define("ts_stack_position", "c_ref_stack_position")
        .define("ts_stack_push", "c_ref_stack_push")
        .define("ts_stack_pop_count", "c_ref_stack_pop_count")
        .define("ts_stack_pop_error", "c_ref_stack_pop_error")
        .define("ts_stack_pop_pending", "c_ref_stack_pop_pending")
        .define("ts_stack_pop_all", "c_ref_stack_pop_all")
        .define(
            "ts_stack_node_count_since_error",
            "c_ref_stack_node_count_since_error",
        )
        .define(
            "ts_stack_dynamic_precedence",
            "c_ref_stack_dynamic_precedence",
        )
        .define(
            "ts_stack_has_advanced_since_error",
            "c_ref_stack_has_advanced_since_error",
        )
        .define("ts_stack_record_summary", "c_ref_stack_record_summary")
        .define("ts_stack_get_summary", "c_ref_stack_get_summary")
        .define("ts_stack_error_cost", "c_ref_stack_error_cost")
        .define("ts_stack_merge", "c_ref_stack_merge")
        .define("ts_stack_can_merge", "c_ref_stack_can_merge")
        .define("ts_stack_resume", "c_ref_stack_resume")
        .define("ts_stack_pause", "c_ref_stack_pause")
        .define("ts_stack_halt", "c_ref_stack_halt")
        .define("ts_stack_is_active", "c_ref_stack_is_active")
        .define("ts_stack_is_paused", "c_ref_stack_is_paused")
        .define("ts_stack_is_halted", "c_ref_stack_is_halted")
        .define("ts_stack_renumber_version", "c_ref_stack_renumber_version")
        .define("ts_stack_swap_versions", "c_ref_stack_swap_versions")
        .define("ts_stack_copy_version", "c_ref_stack_copy_version")
        .define("ts_stack_remove_version", "c_ref_stack_remove_version")
        .define("ts_stack_clear", "c_ref_stack_clear")
        .define("ts_stack_print_dot_graph", "c_ref_stack_print_dot_graph")
        .file(src.join("stack.c"))
        .define("ts_tree_new", "c_ref_tree_new")
        .define("ts_tree_copy", "c_ref_tree_copy")
        .define("ts_tree_delete", "c_ref_tree_delete")
        .define("ts_tree_root_node", "c_ref_tree_root_node")
        .define(
            "ts_tree_root_node_with_offset",
            "c_ref_tree_root_node_with_offset",
        )
        .define("ts_tree_language", "c_ref_tree_language")
        .define("ts_tree_edit", "c_ref_tree_edit")
        .define("ts_tree_included_ranges", "c_ref_tree_included_ranges")
        .define(
            "ts_tree_get_changed_ranges",
            "c_ref_tree_get_changed_ranges",
        )
        .define("ts_tree_print_dot_graph", "c_ref_tree_print_dot_graph")
        .define("_ts_dup", "c_ref__ts_dup")
        .file(src.join("tree.c"))
        .define("ts_tree_cursor_new", "c_ref_tree_cursor_new")
        .define("ts_tree_cursor_reset", "c_ref_tree_cursor_reset")
        .define("ts_tree_cursor_init", "c_ref_tree_cursor_init")
        .define("ts_tree_cursor_delete", "c_ref_tree_cursor_delete")
        .define(
            "ts_tree_cursor_goto_first_child_internal",
            "c_ref_tree_cursor_goto_first_child_internal",
        )
        .define(
            "ts_tree_cursor_goto_first_child",
            "c_ref_tree_cursor_goto_first_child",
        )
        .define(
            "ts_tree_cursor_goto_last_child_internal",
            "c_ref_tree_cursor_goto_last_child_internal",
        )
        .define(
            "ts_tree_cursor_goto_last_child",
            "c_ref_tree_cursor_goto_last_child",
        )
        .define(
            "ts_tree_cursor_goto_first_child_for_byte",
            "c_ref_tree_cursor_goto_first_child_for_byte",
        )
        .define(
            "ts_tree_cursor_goto_first_child_for_point",
            "c_ref_tree_cursor_goto_first_child_for_point",
        )
        .define(
            "ts_tree_cursor_goto_sibling_internal",
            "c_ref_tree_cursor_goto_sibling_internal",
        )
        .define(
            "ts_tree_cursor_goto_next_sibling_internal",
            "c_ref_tree_cursor_goto_next_sibling_internal",
        )
        .define(
            "ts_tree_cursor_goto_next_sibling",
            "c_ref_tree_cursor_goto_next_sibling",
        )
        .define(
            "ts_tree_cursor_goto_previous_sibling_internal",
            "c_ref_tree_cursor_goto_previous_sibling_internal",
        )
        .define(
            "ts_tree_cursor_goto_previous_sibling",
            "c_ref_tree_cursor_goto_previous_sibling",
        )
        .define(
            "ts_tree_cursor_goto_parent",
            "c_ref_tree_cursor_goto_parent",
        )
        .define(
            "ts_tree_cursor_goto_descendant",
            "c_ref_tree_cursor_goto_descendant",
        )
        .define(
            "ts_tree_cursor_current_descendant_index",
            "c_ref_tree_cursor_current_descendant_index",
        )
        .define(
            "ts_tree_cursor_current_node",
            "c_ref_tree_cursor_current_node",
        )
        .define(
            "ts_tree_cursor_current_status",
            "c_ref_tree_cursor_current_status",
        )
        .define(
            "ts_tree_cursor_current_depth",
            "c_ref_tree_cursor_current_depth",
        )
        .define(
            "ts_tree_cursor_parent_node",
            "c_ref_tree_cursor_parent_node",
        )
        .define(
            "ts_tree_cursor_current_field_id",
            "c_ref_tree_cursor_current_field_id",
        )
        .define(
            "ts_tree_cursor_current_field_name",
            "c_ref_tree_cursor_current_field_name",
        )
        .define("ts_tree_cursor_copy", "c_ref_tree_cursor_copy")
        .define("ts_tree_cursor_reset_to", "c_ref_tree_cursor_reset_to")
        .file(src.join("tree_cursor.c"))
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
