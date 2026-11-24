//! The public types of `include/tree_sitter/api.h` (ADR-002: layouts are frozen).
//!
//! Copied from the bindgen output in `binding_rust/bindings.rs` (everything before the first
//! function declaration). `tests/layout.rs` checks every struct against the C `sizeof`,
//! `_Alignof` and `offsetof` values.
#![allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    clippy::all
)]

pub const TREE_SITTER_LANGUAGE_VERSION: u32 = 15;
pub const TREE_SITTER_MIN_COMPATIBLE_LANGUAGE_VERSION: u32 = 13;
pub type TSStateId = u16;
pub type TSSymbol = u16;
pub type TSFieldId = u16;
pub use crate::parser_abi::TSLanguage;
#[repr(C)]
#[derive(Debug)]
pub struct TSParser {
    _unused: [u8; 0],
}
pub use crate::tree::TSTree;
#[repr(C)]
#[derive(Debug)]
pub struct TSQuery {
    _unused: [u8; 0],
}
#[repr(C)]
#[derive(Debug)]
pub struct TSQueryCursor {
    _unused: [u8; 0],
}
#[repr(C)]
#[derive(Debug)]
pub struct TSLookaheadIterator {
    _unused: [u8; 0],
}
pub type TSDecodeFunction = ::core::option::Option<
    unsafe extern "C" fn(string: *const u8, length: u32, code_point: *mut i32) -> u32,
>;
pub const TSInputEncodingUTF8: TSInputEncoding = 0;
pub const TSInputEncodingUTF16LE: TSInputEncoding = 1;
pub const TSInputEncodingUTF16BE: TSInputEncoding = 2;
pub const TSInputEncodingCustom: TSInputEncoding = 3;
pub type TSInputEncoding = ::core::ffi::c_uint;
pub const TSSymbolTypeRegular: TSSymbolType = 0;
pub const TSSymbolTypeAnonymous: TSSymbolType = 1;
pub const TSSymbolTypeSupertype: TSSymbolType = 2;
pub const TSSymbolTypeAuxiliary: TSSymbolType = 3;
pub type TSSymbolType = ::core::ffi::c_uint;
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct TSPoint {
    pub row: u32,
    pub column: u32,
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct TSRange {
    pub start_point: TSPoint,
    pub end_point: TSPoint,
    pub start_byte: u32,
    pub end_byte: u32,
}
#[repr(C)]
#[derive(Debug)]
pub struct TSInput {
    pub payload: *mut ::core::ffi::c_void,
    pub read: ::core::option::Option<
        unsafe extern "C" fn(
            payload: *mut ::core::ffi::c_void,
            byte_index: u32,
            position: TSPoint,
            bytes_read: *mut u32,
        ) -> *const ::core::ffi::c_char,
    >,
    pub encoding: TSInputEncoding,
    pub decode: TSDecodeFunction,
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct TSParseState {
    pub payload: *mut ::core::ffi::c_void,
    pub current_byte_offset: u32,
    pub has_error: bool,
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct TSParseOptions {
    pub payload: *mut ::core::ffi::c_void,
    pub progress_callback:
        ::core::option::Option<unsafe extern "C" fn(state: *mut TSParseState) -> bool>,
}
pub const TSLogTypeParse: TSLogType = 0;
pub const TSLogTypeLex: TSLogType = 1;
pub type TSLogType = ::core::ffi::c_uint;
#[repr(C)]
#[derive(Debug)]
pub struct TSLogger {
    pub payload: *mut ::core::ffi::c_void,
    pub log: ::core::option::Option<
        unsafe extern "C" fn(
            payload: *mut ::core::ffi::c_void,
            log_type: TSLogType,
            buffer: *const ::core::ffi::c_char,
        ),
    >,
}
#[doc = " A summary of a change to a text document.\n\n The `start_byte` and `start_point` values must be less than or equal to the\n `old_end_byte` and `old_end_point` values, respectively. Passing an edit\n that violates these invariants may produce nonsensical results."]
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct TSInputEdit {
    pub start_byte: u32,
    pub old_end_byte: u32,
    pub new_end_byte: u32,
    pub start_point: TSPoint,
    pub old_end_point: TSPoint,
    pub new_end_point: TSPoint,
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct TSNode {
    pub context: [u32; 4usize],
    pub id: *const ::core::ffi::c_void,
    pub tree: *const TSTree,
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct TSTreeCursor {
    pub tree: *const ::core::ffi::c_void,
    pub id: *const ::core::ffi::c_void,
    pub context: [u32; 3usize],
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TSQueryCapture {
    pub node: TSNode,
    pub index: u32,
}
pub const TSQuantifierZero: TSQuantifier = 0;
pub const TSQuantifierZeroOrOne: TSQuantifier = 1;
pub const TSQuantifierZeroOrMore: TSQuantifier = 2;
pub const TSQuantifierOne: TSQuantifier = 3;
pub const TSQuantifierOneOrMore: TSQuantifier = 4;
pub type TSQuantifier = ::core::ffi::c_uint;
#[repr(C)]
#[derive(Debug)]
pub struct TSQueryMatch {
    pub id: u32,
    pub pattern_index: u16,
    pub capture_count: u16,
    pub captures: *const TSQueryCapture,
}
pub const TSQueryPredicateStepTypeDone: TSQueryPredicateStepType = 0;
pub const TSQueryPredicateStepTypeCapture: TSQueryPredicateStepType = 1;
pub const TSQueryPredicateStepTypeString: TSQueryPredicateStepType = 2;
pub type TSQueryPredicateStepType = ::core::ffi::c_uint;
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TSQueryPredicateStep {
    pub type_: TSQueryPredicateStepType,
    pub value_id: u32,
}
pub const TSQueryErrorNone: TSQueryError = 0;
pub const TSQueryErrorSyntax: TSQueryError = 1;
pub const TSQueryErrorNodeType: TSQueryError = 2;
pub const TSQueryErrorField: TSQueryError = 3;
pub const TSQueryErrorCapture: TSQueryError = 4;
pub const TSQueryErrorStructure: TSQueryError = 5;
pub const TSQueryErrorLanguage: TSQueryError = 6;
pub type TSQueryError = ::core::ffi::c_uint;
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct TSQueryCursorState {
    pub payload: *mut ::core::ffi::c_void,
    pub current_byte_offset: u32,
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct TSQueryCursorOptions {
    pub payload: *mut ::core::ffi::c_void,
    pub progress_callback:
        ::core::option::Option<unsafe extern "C" fn(state: *mut TSQueryCursorState) -> bool>,
}
#[doc = " The metadata associated with a language.\n\n Currently, this metadata can be used to check the [Semantic Version](https://semver.org/)\n of the language. This version information should be used to signal if a given parser might\n be incompatible with existing queries when upgrading between major versions, or minor versions\n if it's in zerover."]
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct TSLanguageMetadata {
    pub major_version: u8,
    pub minor_version: u8,
    pub patch_version: u8,
}
