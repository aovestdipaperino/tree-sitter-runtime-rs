//! Types of `src/parser.h`: what generated parsers and the runtime share (ABI 15).
//! Layout mirrors the C header and is checked by `tests/layout.rs`.
#![allow(non_camel_case_types, non_upper_case_globals)]
use crate::api::{TSFieldId, TSLanguageMetadata, TSStateId, TSSymbol};
use core::ffi::{c_char, c_void};

pub const ts_builtin_sym_end: TSSymbol = 0;
pub const ts_builtin_sym_error: TSSymbol = TSSymbol::MAX;
pub const ts_builtin_sym_error_repeat: TSSymbol = ts_builtin_sym_error - 1;
pub const TREE_SITTER_SERIALIZATION_BUFFER_SIZE: usize = 1024;

#[repr(C)]
pub struct TSLexer {
    pub lookahead: i32,
    pub result_symbol: TSSymbol,
    pub advance: unsafe extern "C" fn(*mut TSLexer, bool),
    pub mark_end: unsafe extern "C" fn(*mut TSLexer),
    pub get_column: unsafe extern "C" fn(*mut TSLexer) -> u32,
    pub is_at_included_range_start: unsafe extern "C" fn(*const TSLexer) -> bool,
    pub eof: unsafe extern "C" fn(*const TSLexer) -> bool,
    pub log: Option<unsafe extern "C" fn(*const TSLexer, *const c_char, ...)>,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TSFieldMapEntry {
    pub field_id: TSFieldId,
    pub child_index: u8,
    pub inherited: bool,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TSMapSlice {
    pub index: u16,
    pub length: u16,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TSSymbolMetadata {
    pub visible: bool,
    pub named: bool,
    pub supertype: bool,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TSLexerMode {
    pub lex_state: u16,
    pub external_lex_state: u16,
    pub reserved_word_set_id: u16,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TSCharacterRange {
    pub start: i32,
    pub end: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ParseShift {
    pub kind: u8,
    pub state: TSStateId,
    pub extra: bool,
    pub repetition: bool,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ParseReduce {
    pub kind: u8,
    pub child_count: u8,
    pub symbol: TSSymbol,
    pub dynamic_precedence: i16,
    pub production_id: u16,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub union TSParseAction {
    pub shift: ParseShift,
    pub reduce: ParseReduce,
    pub kind: u8,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EntryHeader {
    pub count: u8,
    pub reusable: bool,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub union TSParseActionEntry {
    pub action: TSParseAction,
    pub entry: EntryHeader,
}

#[repr(C)]
pub struct TSExternalScanner {
    pub states: *const bool,
    pub symbol_map: *const TSSymbol,
    pub create: Option<unsafe extern "C" fn() -> *mut c_void>,
    pub destroy: Option<unsafe extern "C" fn(*mut c_void)>,
    pub scan: Option<unsafe extern "C" fn(*mut c_void, *mut TSLexer, *const bool) -> bool>,
    pub serialize: Option<unsafe extern "C" fn(*mut c_void, *mut c_char) -> u32>,
    pub deserialize: Option<unsafe extern "C" fn(*mut c_void, *const c_char, u32)>,
}

#[repr(C)]
pub struct TSLanguage {
    pub abi_version: u32,
    pub symbol_count: u32,
    pub alias_count: u32,
    pub token_count: u32,
    pub external_token_count: u32,
    pub state_count: u32,
    pub large_state_count: u32,
    pub production_id_count: u32,
    pub field_count: u32,
    pub max_alias_sequence_length: u16,
    pub parse_table: *const u16,
    pub small_parse_table: *const u16,
    pub small_parse_table_map: *const u32,
    pub parse_actions: *const TSParseActionEntry,
    pub symbol_names: *const *const c_char,
    pub field_names: *const *const c_char,
    pub field_map_slices: *const TSMapSlice,
    pub field_map_entries: *const TSFieldMapEntry,
    pub symbol_metadata: *const TSSymbolMetadata,
    pub public_symbol_map: *const TSSymbol,
    pub alias_map: *const u16,
    pub alias_sequences: *const TSSymbol,
    pub lex_modes: *const TSLexerMode,
    pub lex_fn: Option<unsafe extern "C" fn(*mut TSLexer, TSStateId) -> bool>,
    pub keyword_lex_fn: Option<unsafe extern "C" fn(*mut TSLexer, TSStateId) -> bool>,
    pub keyword_capture_token: TSSymbol,
    pub external_scanner: TSExternalScanner,
    pub primary_state_ids: *const TSStateId,
    pub name: *const c_char,
    pub reserved_words: *const TSSymbol,
    pub max_reserved_word_set_size: u16,
    pub supertype_count: u32,
    pub supertype_symbols: *const TSSymbol,
    pub supertype_map_slices: *const TSMapSlice,
    pub supertype_map_entries: *const TSSymbol,
    pub metadata: TSLanguageMetadata,
}
