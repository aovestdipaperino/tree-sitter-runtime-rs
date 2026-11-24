//! Port of `src/language.c` and the inline helpers of `src/language.h`.
//!
//! Everything here reads the tables of a `TSLanguage` through raw pointers exactly as the C does;
//! the tables come from generated parsers (or the wasm stubs), so nothing is bounds checked.
#![allow(clippy::missing_safety_doc)]
use crate::alloc::{ts_free, ts_malloc};
use crate::api::*;
use crate::parser_abi::*;
use crate::ts_assert;
use core::ffi::{c_char, c_int, c_void};
use core::ptr;

pub const LANGUAGE_VERSION_WITH_RESERVED_WORDS: u32 = 15;
pub const LANGUAGE_VERSION_WITH_PRIMARY_STATES: u32 = 14;

pub const TS_PARSE_ACTION_TYPE_SHIFT: u8 = 0;
pub const TS_PARSE_ACTION_TYPE_REDUCE: u8 = 1;
pub const TS_PARSE_ACTION_TYPE_ACCEPT: u8 = 2;
pub const TS_PARSE_ACTION_TYPE_RECOVER: u8 = 3;

unsafe extern "C" {
    fn strncmp(a: *const c_char, b: *const c_char, n: usize) -> c_int;
    // Defined by `wasm_store.c` (no-wasm stubs, or the real thing with the `wasm` feature).
    fn ts_language_is_wasm(this: *const TSLanguage) -> bool;
    fn ts_wasm_language_retain(this: *const TSLanguage);
    fn ts_wasm_language_release(this: *const TSLanguage);
}

#[repr(C)]
pub struct TableEntry {
    pub actions: *const TSParseAction,
    pub action_count: u32,
    pub is_reusable: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct LookaheadIterator {
    pub language: *const TSLanguage,
    pub data: *const u16,
    pub group_end: *const u16,
    pub state: TSStateId,
    pub table_value: u16,
    pub section_index: u16,
    pub group_count: u16,
    pub is_small_state: bool,
    pub actions: *const TSParseAction,
    pub symbol: TSSymbol,
    pub next_state: TSStateId,
    pub action_count: u16,
}

// ---- Inline helpers of language.h ----

/// `ts_language_lookup`: the table value for a state and symbol.
pub unsafe fn ts_language_lookup(
    this: *const TSLanguage,
    state: TSStateId,
    symbol: TSSymbol,
) -> u16 {
    unsafe {
        let l = &*this;
        if u32::from(state) >= l.large_state_count {
            let index = *l
                .small_parse_table_map
                .add((u32::from(state) - l.large_state_count) as usize);
            let mut data = l.small_parse_table.add(index as usize);
            let group_count = *data;
            data = data.add(1);
            for _ in 0..group_count {
                let section_value = *data;
                data = data.add(1);
                let symbol_count = *data;
                data = data.add(1);
                for _ in 0..symbol_count {
                    let s = *data;
                    data = data.add(1);
                    if s == symbol {
                        return section_value;
                    }
                }
            }
            0
        } else {
            *l.parse_table.add(
                u32::from(state)
                    .wrapping_mul(l.symbol_count)
                    .wrapping_add(u32::from(symbol)) as usize,
            )
        }
    }
}

pub unsafe fn ts_language_has_actions(
    this: *const TSLanguage,
    state: TSStateId,
    symbol: TSSymbol,
) -> bool {
    unsafe { ts_language_lookup(this, state, symbol) != 0 }
}

pub unsafe fn ts_language_actions(
    this: *const TSLanguage,
    state: TSStateId,
    symbol: TSSymbol,
    count: *mut u32,
) -> *const TSParseAction {
    unsafe {
        let mut entry = core::mem::MaybeUninit::<TableEntry>::uninit();
        ts_language_table_entry(this, state, symbol, entry.as_mut_ptr());
        let entry = entry.assume_init();
        *count = entry.action_count;
        entry.actions
    }
}

pub unsafe fn ts_language_has_reduce_action(
    this: *const TSLanguage,
    state: TSStateId,
    symbol: TSSymbol,
) -> bool {
    unsafe {
        let mut entry = core::mem::MaybeUninit::<TableEntry>::uninit();
        ts_language_table_entry(this, state, symbol, entry.as_mut_ptr());
        let entry = entry.assume_init();
        entry.action_count > 0 && (*entry.actions).kind == TS_PARSE_ACTION_TYPE_REDUCE
    }
}

/// `ts_language_lookaheads`.
pub unsafe fn ts_language_lookaheads(
    this: *const TSLanguage,
    state: TSStateId,
) -> LookaheadIterator {
    unsafe {
        let l = &*this;
        let is_small_state = u32::from(state) >= l.large_state_count;
        let data;
        let mut group_end = ptr::null();
        let mut group_count = 0u16;
        if is_small_state {
            let index = *l
                .small_parse_table_map
                .add((u32::from(state) - l.large_state_count) as usize);
            data = l.small_parse_table.add(index as usize);
            group_end = data.add(1);
            group_count = *data;
        } else {
            data = l
                .parse_table
                .wrapping_add(u32::from(state).wrapping_mul(l.symbol_count) as usize)
                .wrapping_sub(1);
        }
        LookaheadIterator {
            language: this,
            data,
            group_end,
            state: 0,
            table_value: 0,
            section_index: 0,
            group_count,
            is_small_state,
            actions: ptr::null(),
            symbol: u16::MAX,
            next_state: 0,
            action_count: 0,
        }
    }
}

/// `ts_lookahead_iterator__next`.
pub unsafe fn ts_lookahead_iterator_next_inner(it: *mut LookaheadIterator) -> bool {
    unsafe {
        let s = &mut *it;
        if s.is_small_state {
            s.data = s.data.add(1);
            if s.data == s.group_end {
                if s.group_count == 0 {
                    return false;
                }
                s.group_count -= 1;
                s.table_value = *s.data;
                s.data = s.data.add(1);
                let symbol_count = *s.data;
                s.data = s.data.add(1);
                s.group_end = s.data.add(usize::from(symbol_count));
                s.symbol = *s.data;
            } else {
                s.symbol = *s.data;
                return true;
            }
        } else {
            loop {
                s.data = s.data.add(1);
                s.symbol = s.symbol.wrapping_add(1);
                if u32::from(s.symbol) >= (*s.language).symbol_count {
                    return false;
                }
                s.table_value = *s.data;
                if s.table_value != 0 {
                    break;
                }
            }
        }

        // Terminal or non-terminal: the table value is either an actions index or a successor state.
        if u32::from(s.symbol) < (*s.language).token_count {
            let entry = (*s.language).parse_actions.add(usize::from(s.table_value));
            s.action_count = u16::from((*entry).entry.count);
            s.actions = entry.add(1) as *const TSParseAction;
            s.next_state = 0;
        } else {
            s.action_count = 0;
            s.next_state = s.table_value;
        }
        true
    }
}

pub unsafe fn ts_language_state_is_primary(this: *const TSLanguage, state: TSStateId) -> bool {
    unsafe {
        if (*this).abi_version >= LANGUAGE_VERSION_WITH_PRIMARY_STATES {
            state == *(*this).primary_state_ids.add(usize::from(state))
        } else {
            true
        }
    }
}

pub unsafe fn ts_language_enabled_external_tokens(
    this: *const TSLanguage,
    external_scanner_state: u32,
) -> *const bool {
    unsafe {
        if external_scanner_state == 0 {
            ptr::null()
        } else {
            (*this).external_scanner.states.add(
                ((*this)
                    .external_token_count
                    .wrapping_mul(external_scanner_state)) as usize,
            )
        }
    }
}

pub unsafe fn ts_language_alias_sequence(
    this: *const TSLanguage,
    production_id: u32,
) -> *const TSSymbol {
    unsafe {
        if production_id != 0 {
            (*this).alias_sequences.add(
                production_id.wrapping_mul(u32::from((*this).max_alias_sequence_length)) as usize,
            )
        } else {
            ptr::null()
        }
    }
}

pub unsafe fn ts_language_alias_at(
    this: *const TSLanguage,
    production_id: u32,
    child_index: u32,
) -> TSSymbol {
    unsafe {
        if production_id != 0 {
            *(*this).alias_sequences.add(
                production_id
                    .wrapping_mul(u32::from((*this).max_alias_sequence_length))
                    .wrapping_add(child_index) as usize,
            )
        } else {
            0
        }
    }
}

pub unsafe fn ts_language_field_map(
    this: *const TSLanguage,
    production_id: u32,
    start: *mut *const TSFieldMapEntry,
    end: *mut *const TSFieldMapEntry,
) {
    unsafe {
        let l = &*this;
        if l.field_count == 0 {
            *start = ptr::null();
            *end = ptr::null();
            return;
        }
        let slice = *l.field_map_slices.add(production_id as usize);
        *start = l.field_map_entries.add(usize::from(slice.index));
        *end = l
            .field_map_entries
            .add(usize::from(slice.index))
            .add(usize::from(slice.length));
    }
}

pub unsafe fn ts_language_aliases_for_symbol(
    this: *const TSLanguage,
    original_symbol: TSSymbol,
    start: *mut *const TSSymbol,
    end: *mut *const TSSymbol,
) {
    unsafe {
        let l = &*this;
        *start = l.public_symbol_map.add(usize::from(original_symbol));
        *end = (*start).add(1);

        let mut idx = 0usize;
        loop {
            let symbol = *l.alias_map.add(idx);
            idx += 1;
            if symbol == 0 || symbol > original_symbol {
                break;
            }
            let count = *l.alias_map.add(idx);
            idx += 1;
            if symbol == original_symbol {
                *start = l.alias_map.add(idx);
                *end = l.alias_map.add(idx + usize::from(count));
                break;
            }
            idx += usize::from(count);
        }
    }
}

// ---- language.c ----

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_language_copy(this: *const TSLanguage) -> *const TSLanguage {
    unsafe {
        if !this.is_null() && ts_language_is_wasm(this) {
            ts_wasm_language_retain(this);
        }
    }
    this
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_language_delete(this: *const TSLanguage) {
    unsafe {
        if !this.is_null() && ts_language_is_wasm(this) {
            ts_wasm_language_release(this);
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_language_symbol_count(this: *const TSLanguage) -> u32 {
    unsafe { (*this).symbol_count.wrapping_add((*this).alias_count) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_language_state_count(this: *const TSLanguage) -> u32 {
    unsafe { (*this).state_count }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_language_supertypes(
    this: *const TSLanguage,
    length: *mut u32,
) -> *const TSSymbol {
    unsafe {
        if (*this).abi_version >= LANGUAGE_VERSION_WITH_RESERVED_WORDS {
            *length = (*this).supertype_count;
            (*this).supertype_symbols
        } else {
            *length = 0;
            ptr::null()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_language_subtypes(
    this: *const TSLanguage,
    supertype: TSSymbol,
    length: *mut u32,
) -> *const TSSymbol {
    unsafe {
        if (*this).abi_version < LANGUAGE_VERSION_WITH_RESERVED_WORDS
            || !ts_language_symbol_metadata(this, supertype).supertype
        {
            *length = 0;
            return ptr::null();
        }
        let slice = *(*this).supertype_map_slices.add(usize::from(supertype));
        *length = u32::from(slice.length);
        (*this).supertype_map_entries.add(usize::from(slice.index))
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_language_abi_version(this: *const TSLanguage) -> u32 {
    unsafe { (*this).abi_version }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_language_metadata(
    this: *const TSLanguage,
) -> *const TSLanguageMetadata {
    unsafe {
        if (*this).abi_version >= LANGUAGE_VERSION_WITH_RESERVED_WORDS {
            &(*this).metadata
        } else {
            ptr::null()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_language_name(this: *const TSLanguage) -> *const c_char {
    unsafe {
        if (*this).abi_version >= LANGUAGE_VERSION_WITH_RESERVED_WORDS {
            (*this).name
        } else {
            ptr::null()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_language_field_count(this: *const TSLanguage) -> u32 {
    unsafe { (*this).field_count }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_language_table_entry(
    this: *const TSLanguage,
    state: TSStateId,
    symbol: TSSymbol,
    result: *mut TableEntry,
) {
    unsafe {
        if symbol == ts_builtin_sym_error || symbol == ts_builtin_sym_error_repeat {
            (*result).action_count = 0;
            (*result).is_reusable = false;
            (*result).actions = ptr::null();
        } else {
            ts_assert!(u32::from(symbol) < (*this).token_count);
            let action_index = ts_language_lookup(this, state, symbol);
            let entry = (*this).parse_actions.add(usize::from(action_index));
            (*result).action_count = u32::from((*entry).entry.count);
            (*result).is_reusable = (*entry).entry.reusable;
            (*result).actions = entry.add(1) as *const TSParseAction;
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_language_lex_mode_for_state(
    this: *const TSLanguage,
    state: TSStateId,
) -> TSLexerMode {
    unsafe {
        if (*this).abi_version < 15 {
            // ABI < 15 tables hold the two-field `TSLexMode`.
            let mode = *((*this).lex_modes as *const [u16; 2]).add(usize::from(state));
            TSLexerMode {
                lex_state: mode[0],
                external_lex_state: mode[1],
                reserved_word_set_id: 0,
            }
        } else {
            *(*this).lex_modes.add(usize::from(state))
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_language_is_reserved_word(
    this: *const TSLanguage,
    state: TSStateId,
    symbol: TSSymbol,
) -> bool {
    unsafe {
        let lex_mode = ts_language_lex_mode_for_state(this, state);
        if lex_mode.reserved_word_set_id > 0 {
            let size = u32::from((*this).max_reserved_word_set_size);
            let start = u32::from(lex_mode.reserved_word_set_id) * size;
            let end = start + size;
            for i in start..end {
                let w = *(*this).reserved_words.add(i as usize);
                if w == symbol {
                    return true;
                }
                if w == 0 {
                    break;
                }
            }
        }
        false
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_language_symbol_metadata(
    this: *const TSLanguage,
    symbol: TSSymbol,
) -> TSSymbolMetadata {
    unsafe {
        if symbol == ts_builtin_sym_error {
            TSSymbolMetadata {
                visible: true,
                named: true,
                supertype: false,
            }
        } else if symbol == ts_builtin_sym_error_repeat {
            TSSymbolMetadata {
                visible: false,
                named: false,
                supertype: false,
            }
        } else {
            *(*this).symbol_metadata.add(usize::from(symbol))
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_language_public_symbol(
    this: *const TSLanguage,
    symbol: TSSymbol,
) -> TSSymbol {
    unsafe {
        if symbol == ts_builtin_sym_error {
            return symbol;
        }
        *(*this).public_symbol_map.add(usize::from(symbol))
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_language_next_state(
    this: *const TSLanguage,
    state: TSStateId,
    symbol: TSSymbol,
) -> TSStateId {
    unsafe {
        if symbol == ts_builtin_sym_error || symbol == ts_builtin_sym_error_repeat {
            0
        } else if u32::from(symbol) < (*this).token_count {
            let mut count = 0u32;
            let actions = ts_language_actions(this, state, symbol, &mut count);
            if count > 0 {
                let action = *actions.add(count as usize - 1);
                if action.kind == TS_PARSE_ACTION_TYPE_SHIFT {
                    return if action.shift.extra {
                        state
                    } else {
                        action.shift.state
                    };
                }
            }
            0
        } else {
            ts_language_lookup(this, state, symbol)
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_language_symbol_name(
    this: *const TSLanguage,
    symbol: TSSymbol,
) -> *const c_char {
    unsafe {
        if symbol == ts_builtin_sym_error {
            c"ERROR".as_ptr()
        } else if symbol == ts_builtin_sym_error_repeat {
            c"_ERROR".as_ptr()
        } else if u32::from(symbol) < ts_language_symbol_count(this) {
            *(*this).symbol_names.add(usize::from(symbol))
        } else {
            ptr::null()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_language_symbol_for_name(
    this: *const TSLanguage,
    string: *const c_char,
    length: u32,
    is_named: bool,
) -> TSSymbol {
    unsafe {
        if is_named && strncmp(string, c"ERROR".as_ptr(), length as usize) == 0 {
            return ts_builtin_sym_error;
        }
        let count = ts_language_symbol_count(this) as u16;
        let mut i: TSSymbol = 0;
        while i < count {
            let metadata = ts_language_symbol_metadata(this, i);
            if (!metadata.visible && !metadata.supertype) || metadata.named != is_named {
                i = i.wrapping_add(1);
                continue;
            }
            let symbol_name = *(*this).symbol_names.add(usize::from(i));
            if strncmp(symbol_name, string, length as usize) == 0
                && *symbol_name.add(length as usize) == 0
            {
                return *(*this).public_symbol_map.add(usize::from(i));
            }
            i = i.wrapping_add(1);
        }
        0
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_language_symbol_type(
    this: *const TSLanguage,
    symbol: TSSymbol,
) -> TSSymbolType {
    unsafe {
        let metadata = ts_language_symbol_metadata(this, symbol);
        if metadata.named && metadata.visible {
            TSSymbolTypeRegular
        } else if metadata.visible {
            TSSymbolTypeAnonymous
        } else if metadata.supertype {
            TSSymbolTypeSupertype
        } else {
            TSSymbolTypeAuxiliary
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_language_field_name_for_id(
    this: *const TSLanguage,
    id: TSFieldId,
) -> *const c_char {
    unsafe {
        let count = ts_language_field_count(this);
        if count != 0 && u32::from(id) <= count {
            *(*this).field_names.add(usize::from(id))
        } else {
            ptr::null()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_language_field_id_for_name(
    this: *const TSLanguage,
    name: *const c_char,
    name_length: u32,
) -> TSFieldId {
    unsafe {
        let count = ts_language_field_count(this) as u16;
        let mut i: TSSymbol = 1;
        // `i < count + 1` is evaluated as int in C, so a count of 65535 does not stop at 0.
        while u32::from(i) < u32::from(count) + 1 {
            let field = *(*this).field_names.add(usize::from(i));
            match strncmp(name, field, name_length as usize) {
                0 => {
                    if *field.add(name_length as usize) == 0 {
                        return i;
                    }
                }
                -1 => return 0,
                _ => {}
            }
            i = i.wrapping_add(1);
        }
        0
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_lookahead_iterator_new(
    this: *const TSLanguage,
    state: TSStateId,
) -> *mut TSLookaheadIterator {
    unsafe {
        if u32::from(state) >= (*this).state_count {
            return ptr::null_mut();
        }
        let iterator = ts_malloc(size_of::<LookaheadIterator>()) as *mut LookaheadIterator;
        ptr::write(iterator, ts_language_lookaheads(this, state));
        iterator as *mut TSLookaheadIterator
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_lookahead_iterator_delete(this: *mut TSLookaheadIterator) {
    unsafe { ts_free(this as *mut c_void) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_lookahead_iterator_reset_state(
    this: *mut TSLookaheadIterator,
    state: TSStateId,
) -> bool {
    unsafe {
        let iterator = this as *mut LookaheadIterator;
        if u32::from(state) >= (*(*iterator).language).state_count {
            return false;
        }
        *iterator = ts_language_lookaheads((*iterator).language, state);
        true
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_lookahead_iterator_language(
    this: *const TSLookaheadIterator,
) -> *const TSLanguage {
    unsafe { (*(this as *const LookaheadIterator)).language }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_lookahead_iterator_reset(
    this: *mut TSLookaheadIterator,
    language: *const TSLanguage,
    state: TSStateId,
) -> bool {
    unsafe {
        if u32::from(state) >= (*language).state_count {
            return false;
        }
        *(this as *mut LookaheadIterator) = ts_language_lookaheads(language, state);
        true
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_lookahead_iterator_next(this: *mut TSLookaheadIterator) -> bool {
    unsafe { ts_lookahead_iterator_next_inner(this as *mut LookaheadIterator) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_lookahead_iterator_current_symbol(
    this: *const TSLookaheadIterator,
) -> TSSymbol {
    unsafe { (*(this as *const LookaheadIterator)).symbol }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_lookahead_iterator_current_symbol_name(
    this: *const TSLookaheadIterator,
) -> *const c_char {
    unsafe {
        let iterator = &*(this as *const LookaheadIterator);
        ts_language_symbol_name(iterator.language, iterator.symbol)
    }
}
