//! The Rust mirrors of the C structs must have the C layout (ADR-011).
#![cfg(feature = "reference")]
use core::mem::{align_of, offset_of, size_of};
use std::collections::BTreeMap;
use std::ffi::{CStr, c_char};
use tree_sitter_runtime_rs::subtree::*;
use tree_sitter_runtime_rs::{
    api::*, language::*, length::Length, lexer::*, parser_abi::*, reduce_action::*,
};

unsafe extern "C" {
    fn shim_layout_len() -> usize;
    fn shim_layout_key(i: usize) -> *const c_char;
    fn shim_layout_value(i: usize) -> usize;
}

fn c_table() -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    unsafe {
        for i in 0..shim_layout_len() {
            let k = CStr::from_ptr(shim_layout_key(i))
                .to_str()
                .unwrap()
                .to_string();
            assert!(m.insert(k, shim_layout_value(i)).is_none());
        }
    }
    m
}

macro_rules! rust_table {
    ($m:ident; $( $t:ident { $($f:ident),* } ),* $(,)?) => {$(
        $m.insert(format!("{}.sizeof", stringify!($t)), size_of::<$t>());
        $m.insert(format!("{}.alignof", stringify!($t)), align_of::<$t>());
        $( $m.insert(format!("{}.{}", stringify!($t), stringify!($f)), offset_of!($t, $f)); )*
    )*};
}

#[test]
fn rust_layout_equals_c_layout() {
    let mut r: BTreeMap<String, usize> = BTreeMap::new();
    rust_table!(r;
        TSPoint { row, column },
        TSRange { start_point, end_point, start_byte, end_byte },
        TSInput { payload, read, encoding, decode },
        TSParseState { payload, current_byte_offset, has_error },
        TSParseOptions { payload, progress_callback },
        TSLogger { payload, log },
        TSInputEdit { start_byte, old_end_byte, new_end_byte, start_point, old_end_point, new_end_point },
        TSNode { context, id, tree },
        TSTreeCursor { tree, id, context },
        TSQueryCapture { node, index },
        TSQueryMatch { id, pattern_index, capture_count, captures },
        TSQueryPredicateStep { type_, value_id },
        TSQueryCursorState { payload, current_byte_offset },
        TSQueryCursorOptions { payload, progress_callback },
        Length { bytes, extent },
        ExternalScannerState { length },
        SubtreeInlineData {}, Subtree {}, MutableSubtree {},
        SubtreePool { free_trees, tree_stack },
        SubtreeHeapData { ref_count, padding, size, lookahead_bytes, error_cost, child_count, symbol, parse_state },
        TableEntry { actions, action_count, is_reusable },
        LookaheadIterator { language, data, group_end, table_value, group_count,
                is_small_state, phase, actions, symbol, next_state, action_count },
        TSLexer { lookahead, result_symbol, advance, mark_end, get_column, is_at_included_range_start, eof, log },
        Lexer { data, current_position, token_start_position, token_end_position, included_ranges, chunk, input, logger,
                included_range_count, current_included_range_index, chunk_start, chunk_size, lookahead_size, did_get_column,
                column_data, debug_buffer },
        ColumnData {}, TSFieldMapEntry {}, TSMapSlice {}, TSSymbolMetadata {}, TSLexerMode {}, TSCharacterRange {},
        TSParseAction {}, TSParseActionEntry {},
        TSLanguage { abi_version, symbol_count, alias_count, token_count, external_token_count, state_count,
                large_state_count, production_id_count, field_count, max_alias_sequence_length, parse_table,
                small_parse_table, small_parse_table_map, parse_actions, symbol_names, field_names, field_map_slices,
                field_map_entries, symbol_metadata, public_symbol_map, alias_map, alias_sequences, lex_modes, lex_fn,
                keyword_lex_fn, keyword_capture_token, external_scanner, primary_state_ids, name, reserved_words,
                max_reserved_word_set_size, supertype_count, supertype_symbols, supertype_map_slices,
                supertype_map_entries, metadata },
        ReduceAction { count, symbol, dynamic_precedence, production_id },
        ReduceActionSet { contents, size, capacity },
        TSLanguageMetadata { major_version, minor_version, patch_version },
    );
    // Fields inside the heap data's anonymous unions and the external state's data union.
    let u = offset_of!(SubtreeHeapData, u);
    r.insert(
        "ExternalScannerState.long_data".into(),
        offset_of!(ExternalScannerState, data),
    );
    r.insert(
        "ExternalScannerState.short_data".into(),
        offset_of!(ExternalScannerState, data),
    );
    r.insert("SubtreeHeapData.external_scanner_state".into(), u);
    r.insert("SubtreeHeapData.lookahead_char".into(), u);
    r.insert(
        "SubtreeHeapData.visible_child_count".into(),
        u + offset_of!(NodeData, visible_child_count),
    );
    r.insert(
        "SubtreeHeapData.named_child_count".into(),
        u + offset_of!(NodeData, named_child_count),
    );
    r.insert(
        "SubtreeHeapData.visible_descendant_count".into(),
        u + offset_of!(NodeData, visible_descendant_count),
    );
    r.insert(
        "SubtreeHeapData.dynamic_precedence".into(),
        u + offset_of!(NodeData, dynamic_precedence),
    );
    r.insert(
        "SubtreeHeapData.repeat_depth".into(),
        u + offset_of!(NodeData, repeat_depth),
    );
    r.insert(
        "SubtreeHeapData.production_id".into(),
        u + offset_of!(NodeData, production_id),
    );
    r.insert(
        "SubtreeHeapData.first_leaf".into(),
        u + offset_of!(NodeData, first_leaf),
    );
    // The C inline data has alignment 2 (its widest member is a uint16_t) but the port keeps it
    // as one u64 (alignment 8). It only ever occurs inside the 8-aligned `Subtree` unions, whose
    // layout is compared above, so only its size is checked.
    r.remove("SubtreeInlineData.alignof");
    let mut c = c_table();
    c.remove("SubtreeInlineData.alignof");
    // The C field is named `type`, a Rust keyword.
    if let Some(v) = c.remove("TSQueryPredicateStep.type") {
        c.insert("TSQueryPredicateStep.type_".into(), v);
    }
    assert_eq!(c.len(), r.len(), "tables differ in size");
    for (k, v) in &c {
        assert_eq!(r.get(k), Some(v), "layout mismatch for {k}");
    }
}
