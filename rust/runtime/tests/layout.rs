//! The Rust mirrors of the C structs must have the C layout (ADR-011).
use core::mem::{align_of, offset_of, size_of};
use std::collections::BTreeMap;
use std::ffi::{CStr, c_char};
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
        TableEntry { actions, action_count, is_reusable },
        LookaheadIterator { language, data, group_end, state, table_value, section_index, group_count,
                is_small_state, actions, symbol, next_state, action_count },
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
    // The C field is named `type`, a Rust keyword.
    let mut c = c_table();
    if let Some(v) = c.remove("TSQueryPredicateStep.type") {
        c.insert("TSQueryPredicateStep.type_".into(), v);
    }
    assert_eq!(c.len(), r.len(), "tables differ in size");
    for (k, v) in &c {
        assert_eq!(r.get(k), Some(v), "layout mismatch for {k}");
    }
}
