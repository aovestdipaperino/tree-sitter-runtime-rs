//! Port of the no-wasm half of `src/wasm_store.c` (ADR-003): without the `wasm` feature every
//! wasm entry point is a dummy. `ts_language_is_wasm` is always false, so the parser never reaches
//! the others; they exist because the parser and language units link against them.
use crate::api::{TSLanguage, TSStateId};
use core::ffi::{c_char, c_void};

#[unsafe(no_mangle)]
pub extern "C" fn ts_wasm_store_delete(_this: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn ts_wasm_store_start(
    _this: *mut c_void,
    _lexer: *mut c_void,
    _language: *const TSLanguage,
) -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn ts_wasm_store_reset(_this: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn ts_wasm_store_call_lex_main(_this: *mut c_void, _state: TSStateId) -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn ts_wasm_store_call_lex_keyword(_this: *mut c_void, _state: TSStateId) -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn ts_wasm_store_call_scanner_create(_this: *mut c_void) -> u32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn ts_wasm_store_call_scanner_destroy(_this: *mut c_void, _scanner_address: u32) {}

#[unsafe(no_mangle)]
pub extern "C" fn ts_wasm_store_call_scanner_scan(
    _this: *mut c_void,
    _scanner_address: u32,
    _valid_tokens_ix: u32,
) -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn ts_wasm_store_call_scanner_serialize(
    _this: *mut c_void,
    _scanner_address: u32,
    _buffer: *mut c_char,
) -> u32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn ts_wasm_store_call_scanner_deserialize(
    _this: *mut c_void,
    _scanner_address: u32,
    _buffer: *const c_char,
    _length: u32,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn ts_wasm_store_has_error(_this: *const c_void) -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn ts_language_is_wasm(_this: *const TSLanguage) -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn ts_wasm_language_retain(_this: *const TSLanguage) {}

#[unsafe(no_mangle)]
pub extern "C" fn ts_wasm_language_release(_this: *const TSLanguage) {}
