//! Port of `src/parser.c`: the GLR parse loop, error recovery and the public parser API.
//!
//! Faithful translation: every function keeps the C's name (`ts_parser__x` becomes `parser_x`) and
//! order of operations, because parse results (trees, error recovery choices, log output) must be
//! identical to the C. The parser is handled through raw pointers, as in the C.
#![allow(clippy::missing_safety_doc)]
use crate::alloc::{ts_calloc, ts_free};
use crate::api::*;
use crate::array::Array;
use crate::consts::*;
use crate::get_changed_ranges::{ts_range_array_get_changed_ranges, ts_range_array_intersects};
use crate::language::*;
use crate::length::*;
use crate::lexer::*;
use crate::parser_abi::*;
use crate::reduce_action::*;
use crate::reusable_node::*;
use crate::stack::*;
use crate::subtree::*;
use crate::tree::{TSTree, ts_tree_new};
use crate::ts_assert;
use core::ffi::{CStr, c_char, c_int, c_void};
use core::ptr;

unsafe extern "C" {
    fn snprintf(buf: *mut c_char, n: usize, fmt: *const c_char, ...) -> c_int;
    fn fprintf(f: *mut c_void, fmt: *const c_char, ...) -> c_int;
    fn fputc(c: c_int, f: *mut c_void) -> c_int;
    fn fputs(s: *const c_char, f: *mut c_void) -> c_int;
    fn fdopen(fd: c_int, mode: *const c_char) -> *mut c_void;
    fn fclose(f: *mut c_void) -> c_int;

    fn ts_language_is_wasm(this: *const TSLanguage) -> bool;
    fn ts_wasm_store_delete(this: *mut c_void);
    fn ts_wasm_store_start(
        this: *mut c_void,
        lexer: *mut TSLexer,
        language: *const TSLanguage,
    ) -> bool;
    fn ts_wasm_store_reset(this: *mut c_void);
    fn ts_wasm_store_has_error(this: *const c_void) -> bool;
    fn ts_wasm_store_call_lex_main(this: *mut c_void, state: TSStateId) -> bool;
    fn ts_wasm_store_call_lex_keyword(this: *mut c_void, state: TSStateId) -> bool;
    fn ts_wasm_store_call_scanner_create(this: *mut c_void) -> u32;
    fn ts_wasm_store_call_scanner_scan(
        this: *mut c_void,
        address: u32,
        valid_tokens_ix: u32,
    ) -> bool;
    fn ts_wasm_store_call_scanner_serialize(
        this: *mut c_void,
        address: u32,
        buffer: *mut c_char,
    ) -> u32;
    fn ts_wasm_store_call_scanner_deserialize(
        this: *mut c_void,
        address: u32,
        buffer: *const c_char,
        length: u32,
    );
}

const MAX_VERSION_COUNT: u32 = 6;
const MAX_VERSION_COUNT_OVERFLOW: u32 = 4;
const MAX_SUMMARY_DEPTH: u32 = 16;
const MAX_COST_DIFFERENCE: u32 = 18 * ERROR_COST_PER_SKIPPED_TREE;
const OP_COUNT_PER_PARSER_CALLBACK_CHECK: u32 = 100;

type TSRangeArray = Array<TSRange>;

#[repr(C)]
struct TokenCache {
    token: Subtree,
    last_external_token: Subtree,
    byte_index: u32,
}

/// `struct TSParser`.
#[repr(C)]
pub struct Parser {
    lexer: Lexer,
    stack: *mut Stack,
    tree_pool: SubtreePool,
    language: *const TSLanguage,
    wasm_store: *mut c_void,
    reduce_actions: ReduceActionSet,
    finished_tree: Subtree,
    trailing_extras: SubtreeArray,
    trailing_extras2: SubtreeArray,
    scratch_trees: SubtreeArray,
    token_cache: TokenCache,
    reusable_node: ReusableNode,
    external_scanner_payload: *mut c_void,
    dot_graph_file: *mut c_void,
    accept_count: u32,
    operation_count: u32,
    old_tree: Subtree,
    included_range_differences: TSRangeArray,
    parse_options: TSParseOptions,
    parse_state: TSParseState,
    included_range_difference_index: u32,
    has_scanner_error: bool,
    canceled_balancing: bool,
    has_error: bool,
}

#[derive(Clone, Copy)]
struct ErrorStatus {
    cost: u32,
    node_count: u32,
    dynamic_precedence: i32,
    is_in_error: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ErrorComparison {
    TakeLeft,
    PreferLeft,
    None,
    PreferRight,
    TakeRight,
}

#[repr(C)]
struct TSStringInput {
    string: *const c_char,
    length: u32,
}

#[inline]
fn is_null(s: Subtree) -> bool {
    // SAFETY: both union arms are plain data; the pointer arm is the one the C tests.
    unsafe { s.ptr.is_null() }
}

// ---- logging macros (LOG, LOG_LOOKAHEAD, LOG_STACK, LOG_TREE) ----

macro_rules! log {
    ($this:expr, $fmt:literal $(, $arg:expr)*) => {
        if (*$this).lexer.logger.log.is_some() || !(*$this).dot_graph_file.is_null() {
            snprintf(
                (*$this).lexer.debug_buffer.as_mut_ptr(),
                TREE_SITTER_SERIALIZATION_BUFFER_SIZE,
                $fmt.as_ptr()
                $(, $arg)*
            );
            parser_log($this);
        }
    };
}

macro_rules! sym_name {
    ($this:expr, $sym:expr) => {
        ts_language_symbol_name((*$this).language, $sym)
    };
}

macro_rules! tree_name {
    ($this:expr, $tree:expr) => {
        sym_name!($this, ts_subtree_symbol($tree))
    };
}

macro_rules! log_stack {
    ($this:expr) => {
        if !(*$this).dot_graph_file.is_null() {
            ts_stack_print_dot_graph((*$this).stack, (*$this).language, (*$this).dot_graph_file);
            fputs(c"\n\n".as_ptr(), (*$this).dot_graph_file);
        }
    };
}

macro_rules! log_tree {
    ($this:expr, $tree:expr) => {
        if !(*$this).dot_graph_file.is_null() {
            ts_subtree_print_dot_graph($tree, (*$this).language, (*$this).dot_graph_file);
            fputs(c"\n".as_ptr(), (*$this).dot_graph_file);
        }
    };
}

unsafe fn log_lookahead(this: *mut Parser, symbol_name: *const c_char, size: u32) {
    unsafe {
        if (*this).lexer.logger.log.is_some() || !(*this).dot_graph_file.is_null() {
            let buf = (*this).lexer.debug_buffer.as_mut_ptr();
            let cap = TREE_SITTER_SERIALIZATION_BUFFER_SIZE as c_int;
            let mut off = snprintf(
                buf,
                TREE_SITTER_SERIALIZATION_BUFFER_SIZE,
                c"lexed_lookahead sym:".as_ptr(),
            );
            let mut i = 0usize;
            while *symbol_name.add(i) != 0 && off < cap {
                let ch = *symbol_name.add(i) as u8;
                let escaped = match ch {
                    b'\t' => Some(b't'),
                    b'\n' => Some(b'n'),
                    0x0b => Some(b'v'),
                    0x0c => Some(b'f'),
                    b'\r' => Some(b'r'),
                    b'\\' => Some(b'\\'),
                    _ => None,
                };
                if let Some(e) = escaped {
                    *buf.add(off as usize) = b'\\' as c_char;
                    off += 1;
                    *buf.add(off as usize) = e as c_char;
                    off += 1;
                } else {
                    *buf.add(off as usize) = ch as c_char;
                    off += 1;
                }
                i += 1;
            }
            snprintf(
                buf.add(off as usize),
                (cap - off) as usize,
                c", size:%u".as_ptr(),
                size,
            );
            parser_log(this);
        }
    }
}

// ---- StringInput ----

unsafe extern "C" fn ts_string_input_read(
    payload: *mut c_void,
    byte: u32,
    _point: TSPoint,
    length: *mut u32,
) -> *const c_char {
    unsafe {
        let this = payload as *mut TSStringInput;
        if byte >= (*this).length {
            *length = 0;
            c"".as_ptr()
        } else {
            *length = (*this).length - byte;
            (*this).string.add(byte as usize)
        }
    }
}

// ---- Parser - private ----

unsafe fn parser_log(this: *mut Parser) {
    unsafe {
        if let Some(log) = (*this).lexer.logger.log {
            log(
                (*this).lexer.logger.payload,
                TSLogTypeParse,
                (*this).lexer.debug_buffer.as_ptr(),
            );
        }

        if !(*this).dot_graph_file.is_null() {
            let f = (*this).dot_graph_file;
            fprintf(f, c"graph {\nlabel=\"".as_ptr());
            let mut chr = (*this).lexer.debug_buffer.as_ptr();
            while *chr != 0 {
                if *chr == b'"' as c_char || *chr == b'\\' as c_char {
                    fputc(c_int::from(b'\\'), f);
                }
                fputc(c_int::from(*chr as u8), f);
                chr = chr.add(1);
            }
            fprintf(f, c"\"\n}\n\n".as_ptr());
        }
    }
}

unsafe fn parser_breakdown_top_of_stack(this: *mut Parser, version: StackVersion) -> bool {
    unsafe {
        let mut did_break_down = false;
        let mut pending;

        loop {
            let pop = ts_stack_pop_pending((*this).stack, version);
            if pop.size == 0 {
                break;
            }

            did_break_down = true;
            pending = false;
            for i in 0..pop.size {
                let mut slice = *pop.get(i);
                let mut state = ts_stack_state((*this).stack, slice.version);
                let parent = *slice.subtrees.front();

                let n = ts_subtree_child_count(parent);
                for j in 0..n {
                    let child = *ts_subtree_children(parent).add(j as usize);
                    pending = ts_subtree_child_count(child) > 0;

                    if ts_subtree_is_error(child) {
                        state = ERROR_STATE;
                    } else if !ts_subtree_extra(child) {
                        state = ts_language_next_state(
                            (*this).language,
                            state,
                            ts_subtree_symbol(child),
                        );
                    }

                    ts_subtree_retain(child);
                    ts_stack_push((*this).stack, slice.version, child, pending, state);
                }

                for j in 1..slice.subtrees.size {
                    let tree = *slice.subtrees.get(j);
                    ts_stack_push((*this).stack, slice.version, tree, false, state);
                }

                ts_subtree_release(&mut (*this).tree_pool, parent);
                slice.subtrees.delete();

                log!(
                    this,
                    c"breakdown_top_of_stack tree:%s",
                    tree_name!(this, parent)
                );
                log_stack!(this);
            }
            if !pending {
                break;
            }
        }

        did_break_down
    }
}

unsafe fn parser_breakdown_lookahead(
    this: *mut Parser,
    lookahead: *mut Subtree,
    state: TSStateId,
    reusable_node: *mut ReusableNode,
) {
    unsafe {
        let mut did_descend = false;
        let mut tree = reusable_node_tree(&*reusable_node);
        while ts_subtree_child_count(tree) > 0 && ts_subtree_parse_state(tree) != state {
            log!(this, c"state_mismatch sym:%s", tree_name!(this, tree));
            reusable_node_descend(&mut *reusable_node);
            tree = reusable_node_tree(&*reusable_node);
            did_descend = true;
        }

        if did_descend {
            ts_subtree_release(&mut (*this).tree_pool, *lookahead);
            *lookahead = tree;
            ts_subtree_retain(*lookahead);
        }
    }
}

fn parser_compare_versions(a: ErrorStatus, b: ErrorStatus) -> ErrorComparison {
    if !a.is_in_error && b.is_in_error {
        if a.cost < b.cost {
            return ErrorComparison::TakeLeft;
        } else {
            return ErrorComparison::PreferLeft;
        }
    }

    if a.is_in_error && !b.is_in_error {
        if b.cost < a.cost {
            return ErrorComparison::TakeRight;
        } else {
            return ErrorComparison::PreferRight;
        }
    }

    if a.cost < b.cost {
        if (b.cost - a.cost).wrapping_mul(1u32.wrapping_add(a.node_count)) > MAX_COST_DIFFERENCE {
            return ErrorComparison::TakeLeft;
        } else {
            return ErrorComparison::PreferLeft;
        }
    }

    if b.cost < a.cost {
        if (a.cost - b.cost).wrapping_mul(1u32.wrapping_add(b.node_count)) > MAX_COST_DIFFERENCE {
            return ErrorComparison::TakeRight;
        } else {
            return ErrorComparison::PreferRight;
        }
    }

    if a.dynamic_precedence > b.dynamic_precedence {
        return ErrorComparison::PreferLeft;
    }
    if b.dynamic_precedence > a.dynamic_precedence {
        return ErrorComparison::PreferRight;
    }
    ErrorComparison::None
}

unsafe fn parser_version_status(this: *mut Parser, version: StackVersion) -> ErrorStatus {
    unsafe {
        let mut cost = ts_stack_error_cost((*this).stack, version);
        let is_paused = ts_stack_is_paused((*this).stack, version);
        if is_paused {
            cost = cost.wrapping_add(ERROR_COST_PER_SKIPPED_TREE);
        }
        ErrorStatus {
            cost,
            node_count: ts_stack_node_count_since_error((*this).stack, version),
            dynamic_precedence: ts_stack_dynamic_precedence((*this).stack, version),
            is_in_error: is_paused || ts_stack_state((*this).stack, version) == ERROR_STATE,
        }
    }
}

unsafe fn parser_better_version_exists(
    this: *mut Parser,
    version: StackVersion,
    is_in_error: bool,
    cost: u32,
) -> bool {
    unsafe {
        if !is_null((*this).finished_tree) && ts_subtree_error_cost((*this).finished_tree) <= cost {
            return true;
        }

        let position = ts_stack_position((*this).stack, version);
        let status = ErrorStatus {
            cost,
            is_in_error,
            dynamic_precedence: ts_stack_dynamic_precedence((*this).stack, version),
            node_count: ts_stack_node_count_since_error((*this).stack, version),
        };

        let n = ts_stack_version_count((*this).stack);
        for i in 0..n {
            if i == version
                || !ts_stack_is_active((*this).stack, i)
                || ts_stack_position((*this).stack, i).bytes < position.bytes
            {
                continue;
            }
            let status_i = parser_version_status(this, i);
            match parser_compare_versions(status, status_i) {
                ErrorComparison::TakeRight => return true,
                ErrorComparison::PreferRight if ts_stack_can_merge((*this).stack, i, version) => {
                    return true;
                }
                _ => {}
            }
        }

        false
    }
}

unsafe fn parser_call_main_lex_fn(this: *mut Parser, lex_mode: TSLexerMode) -> bool {
    unsafe {
        if ts_language_is_wasm((*this).language) {
            ts_wasm_store_call_lex_main((*this).wasm_store, lex_mode.lex_state)
        } else {
            ((*(*this).language).lex_fn.unwrap_unchecked())(
                &mut (*this).lexer.data,
                lex_mode.lex_state,
            )
        }
    }
}

unsafe fn parser_call_keyword_lex_fn(this: *mut Parser) -> bool {
    unsafe {
        if ts_language_is_wasm((*this).language) {
            ts_wasm_store_call_lex_keyword((*this).wasm_store, 0)
        } else {
            ((*(*this).language).keyword_lex_fn.unwrap_unchecked())(&mut (*this).lexer.data, 0)
        }
    }
}

unsafe fn parser_external_scanner_create(this: *mut Parser) {
    unsafe {
        if !(*this).language.is_null() && !(*(*this).language).external_scanner.states.is_null() {
            if ts_language_is_wasm((*this).language) {
                (*this).external_scanner_payload =
                    ts_wasm_store_call_scanner_create((*this).wasm_store) as usize as *mut c_void;
                if ts_wasm_store_has_error((*this).wasm_store) {
                    (*this).has_scanner_error = true;
                }
            } else if let Some(create) = (*(*this).language).external_scanner.create {
                (*this).external_scanner_payload = create();
            }
        }
    }
}

unsafe fn parser_external_scanner_destroy(this: *mut Parser) {
    unsafe {
        if !(*this).language.is_null()
            && !(*this).external_scanner_payload.is_null()
            && (*(*this).language).external_scanner.destroy.is_some()
            && !ts_language_is_wasm((*this).language)
        {
            ((*(*this).language)
                .external_scanner
                .destroy
                .unwrap_unchecked())((*this).external_scanner_payload);
        }
        (*this).external_scanner_payload = ptr::null_mut();
    }
}

unsafe fn parser_external_scanner_serialize(this: *mut Parser) -> u32 {
    unsafe {
        let length;
        if ts_language_is_wasm((*this).language) {
            length = ts_wasm_store_call_scanner_serialize(
                (*this).wasm_store,
                (*this).external_scanner_payload as usize as u32,
                (*this).lexer.debug_buffer.as_mut_ptr(),
            );
            if ts_wasm_store_has_error((*this).wasm_store) {
                (*this).has_scanner_error = true;
            }
        } else {
            length = ((*(*this).language)
                .external_scanner
                .serialize
                .unwrap_unchecked())(
                (*this).external_scanner_payload,
                (*this).lexer.debug_buffer.as_mut_ptr(),
            );
        }
        ts_assert!(length as usize <= TREE_SITTER_SERIALIZATION_BUFFER_SIZE);
        length
    }
}

unsafe fn parser_external_scanner_deserialize(this: *mut Parser, external_token: Subtree) {
    unsafe {
        let mut data: *const c_char = ptr::null();
        let mut length = 0u32;
        if !is_null(external_token) {
            let state =
                &(*external_token.ptr).u.external_scanner_state as *const ExternalScannerState;
            data = ts_external_scanner_state_data(state as *mut ExternalScannerState);
            length = (*state).length;
        }

        if ts_language_is_wasm((*this).language) {
            ts_wasm_store_call_scanner_deserialize(
                (*this).wasm_store,
                (*this).external_scanner_payload as usize as u32,
                data,
                length,
            );
            if ts_wasm_store_has_error((*this).wasm_store) {
                (*this).has_scanner_error = true;
            }
        } else {
            ((*(*this).language)
                .external_scanner
                .deserialize
                .unwrap_unchecked())((*this).external_scanner_payload, data, length);
        }
    }
}

unsafe fn parser_external_scanner_scan(this: *mut Parser, external_lex_state: TSStateId) -> bool {
    unsafe {
        if ts_language_is_wasm((*this).language) {
            let result = ts_wasm_store_call_scanner_scan(
                (*this).wasm_store,
                (*this).external_scanner_payload as usize as u32,
                u32::from(external_lex_state)
                    .wrapping_mul((*(*this).language).external_token_count),
            );
            if ts_wasm_store_has_error((*this).wasm_store) {
                (*this).has_scanner_error = true;
            }
            result
        } else {
            let valid_external_tokens = ts_language_enabled_external_tokens(
                (*this).language,
                u32::from(external_lex_state),
            );
            ((*(*this).language).external_scanner.scan.unwrap_unchecked())(
                (*this).external_scanner_payload,
                &mut (*this).lexer.data,
                valid_external_tokens,
            )
        }
    }
}

unsafe fn parser_can_reuse_first_leaf(
    this: *mut Parser,
    state: TSStateId,
    tree: Subtree,
    table_entry: *mut TableEntry,
) -> bool {
    unsafe {
        let leaf_symbol = ts_subtree_leaf_symbol(tree);
        let leaf_state = ts_subtree_leaf_parse_state(tree);
        let current_lex_mode = ts_language_lex_mode_for_state((*this).language, state);
        let leaf_lex_mode = ts_language_lex_mode_for_state((*this).language, leaf_state);

        // At the end of a non-terminal extra node, the lexer normally returns
        // NULL, which indicates that the parser should look for a reduce action
        // at symbol `0`. Avoid reusing tokens in this situation to ensure that
        // the same thing happens when incrementally reparsing.
        if current_lex_mode.lex_state == u16::MAX {
            return false;
        }

        // If the token was created in a state with the same set of lookaheads, it is reusable.
        if (*table_entry).action_count > 0
            && leaf_lex_mode.lex_state == current_lex_mode.lex_state
            && leaf_lex_mode.external_lex_state == current_lex_mode.external_lex_state
            && leaf_lex_mode.reserved_word_set_id == current_lex_mode.reserved_word_set_id
            && (leaf_symbol != (*(*this).language).keyword_capture_token
                || (!ts_subtree_is_keyword(tree) && ts_subtree_parse_state(tree) == state))
        {
            return true;
        }

        // Empty tokens are not reusable in states with different lookaheads.
        if ts_subtree_size(tree).bytes == 0 && leaf_symbol != ts_builtin_sym_end {
            return false;
        }

        // If the current state allows external tokens or other tokens that conflict with this
        // token, this token is not reusable.
        current_lex_mode.external_lex_state == 0 && (*table_entry).is_reusable
    }
}

unsafe fn parser_lex(this: *mut Parser, version: StackVersion, parse_state: TSStateId) -> Subtree {
    unsafe {
        let mut lex_mode = ts_language_lex_mode_for_state((*this).language, parse_state);
        if lex_mode.lex_state == u16::MAX {
            log!(this, c"no_lookahead_after_non_terminal_extra");
            return NULL_SUBTREE;
        }

        let start_position = ts_stack_position((*this).stack, version);
        let external_token = ts_stack_last_external_token((*this).stack, version);

        let mut found_external_token = false;
        let mut error_mode = parse_state == ERROR_STATE;
        let mut skipped_error = false;
        let mut called_get_column = false;
        let mut first_error_character = 0i32;
        let mut error_start_position = length_zero();
        let mut error_end_position = length_zero();
        let mut lookahead_end_byte = 0u32;
        let mut external_scanner_state_len = 0u32;
        let mut external_scanner_state_changed = false;
        ts_lexer_reset(&mut (*this).lexer, start_position);

        loop {
            let mut found_token;
            let current_position = (*this).lexer.current_position;
            let column_data = (*this).lexer.column_data;

            if lex_mode.external_lex_state != 0 {
                log!(
                    this,
                    c"lex_external state:%d, row:%u, column:%u",
                    c_int::from(lex_mode.external_lex_state),
                    current_position.extent.row,
                    current_position.extent.column
                );
                ts_lexer_start(&mut (*this).lexer);
                parser_external_scanner_deserialize(this, external_token);
                found_token = parser_external_scanner_scan(this, lex_mode.external_lex_state);
                called_get_column |= (*this).lexer.did_get_column;
                if (*this).has_scanner_error {
                    return NULL_SUBTREE;
                }
                ts_lexer_finish(&mut (*this).lexer, &mut lookahead_end_byte);

                if found_token {
                    external_scanner_state_len = parser_external_scanner_serialize(this);
                    external_scanner_state_changed = !ts_external_scanner_state_eq(
                        ts_subtree_external_scanner_state(external_token),
                        (*this).lexer.debug_buffer.as_ptr(),
                        external_scanner_state_len,
                    );

                    // Avoid infinite loops caused by the external scanner returning empty tokens.
                    // Empty tokens are needed in some circumstances, e.g. indent/dedent tokens
                    // in Python. Ignore the following classes of empty tokens:
                    //
                    // * Tokens produced during error recovery. When recovering from an error,
                    //   all tokens are allowed, so it's easy to accidentally return unwanted
                    //   empty tokens.
                    // * Tokens that are marked as 'extra' in the grammar. These don't change
                    //   the parse state, so they would definitely cause an infinite loop.
                    if (*this).lexer.token_end_position.bytes <= current_position.bytes
                        && !external_scanner_state_changed
                    {
                        let symbol = *(*(*this).language)
                            .external_scanner
                            .symbol_map
                            .add(usize::from((*this).lexer.data.result_symbol));
                        let next_parse_state =
                            ts_language_next_state((*this).language, parse_state, symbol);
                        let token_is_extra = next_parse_state == parse_state;
                        if error_mode
                            || !ts_stack_has_advanced_since_error((*this).stack, version)
                            || token_is_extra
                        {
                            log!(
                                this,
                                c"ignore_empty_external_token symbol:%s",
                                sym_name!(
                                    this,
                                    *(*(*this).language)
                                        .external_scanner
                                        .symbol_map
                                        .add(usize::from((*this).lexer.data.result_symbol))
                                )
                            );
                            found_token = false;
                        }
                    }
                }

                if found_token {
                    found_external_token = true;
                    break;
                }

                ts_lexer_reset(&mut (*this).lexer, current_position);
                (*this).lexer.column_data = column_data;
            }

            log!(
                this,
                c"lex_internal state:%d, row:%u, column:%u",
                c_int::from(lex_mode.lex_state),
                current_position.extent.row,
                current_position.extent.column
            );
            ts_lexer_start(&mut (*this).lexer);
            found_token = parser_call_main_lex_fn(this, lex_mode);
            ts_lexer_finish(&mut (*this).lexer, &mut lookahead_end_byte);
            if found_token {
                break;
            }

            if !error_mode {
                error_mode = true;
                lex_mode = ts_language_lex_mode_for_state((*this).language, ERROR_STATE);
                ts_lexer_reset(&mut (*this).lexer, start_position);
                continue;
            }

            if !skipped_error {
                log!(this, c"skip_unrecognized_character");
                skipped_error = true;
                error_start_position = (*this).lexer.token_start_position;
                error_end_position = (*this).lexer.token_start_position;
                first_error_character = (*this).lexer.data.lookahead;
            }

            if (*this).lexer.current_position.bytes == error_end_position.bytes {
                if ((*this).lexer.data.eof)(&(*this).lexer.data) {
                    (*this).lexer.data.result_symbol = ts_builtin_sym_error;
                    break;
                }
                ((*this).lexer.data.advance)(&mut (*this).lexer.data, false);
            }

            error_end_position = (*this).lexer.current_position;
        }

        let result;
        if skipped_error {
            let padding = length_sub(error_start_position, start_position);
            let size = length_sub(error_end_position, error_start_position);
            let lookahead_bytes = lookahead_end_byte.wrapping_sub(error_end_position.bytes);
            result = ts_subtree_new_error(
                &mut (*this).tree_pool,
                first_error_character,
                padding,
                size,
                lookahead_bytes,
                parse_state,
                (*this).language,
            );
        } else {
            let mut is_keyword = false;
            let mut symbol = (*this).lexer.data.result_symbol;
            let padding = length_sub((*this).lexer.token_start_position, start_position);
            let size = length_sub(
                (*this).lexer.token_end_position,
                (*this).lexer.token_start_position,
            );
            let lookahead_bytes =
                lookahead_end_byte.wrapping_sub((*this).lexer.token_end_position.bytes);

            if found_external_token {
                symbol = *(*(*this).language)
                    .external_scanner
                    .symbol_map
                    .add(usize::from(symbol));
            } else if symbol == (*(*this).language).keyword_capture_token && symbol != 0 {
                let end_byte = (*this).lexer.token_end_position.bytes;
                ts_lexer_reset(&mut (*this).lexer, (*this).lexer.token_start_position);
                ts_lexer_start(&mut (*this).lexer);

                is_keyword = parser_call_keyword_lex_fn(this);

                if is_keyword
                    && (*this).lexer.token_end_position.bytes == end_byte
                    && (ts_language_has_actions(
                        (*this).language,
                        parse_state,
                        (*this).lexer.data.result_symbol,
                    ) || ts_language_is_reserved_word(
                        (*this).language,
                        parse_state,
                        (*this).lexer.data.result_symbol,
                    ))
                {
                    symbol = (*this).lexer.data.result_symbol;
                }
            }

            result = ts_subtree_new_leaf(
                &mut (*this).tree_pool,
                symbol,
                padding,
                size,
                lookahead_bytes,
                parse_state,
                found_external_token,
                called_get_column,
                is_keyword,
                (*this).language,
            );

            if found_external_token {
                let mut_result = ts_subtree_to_mut_unsafe(result);
                ts_external_scanner_state_init(
                    &mut (*mut_result.ptr).u.external_scanner_state,
                    (*this).lexer.debug_buffer.as_ptr(),
                    external_scanner_state_len,
                );
                (*mut_result.ptr)
                    .set_has_external_scanner_state_change(external_scanner_state_changed);
            }
        }

        log_lookahead(
            this,
            sym_name!(this, ts_subtree_symbol(result)),
            ts_subtree_total_size(result).bytes,
        );
        result
    }
}

unsafe fn parser_get_cached_token(
    this: *mut Parser,
    state: TSStateId,
    position: usize,
    last_external_token: Subtree,
    table_entry: *mut TableEntry,
) -> Subtree {
    unsafe {
        let cache = &mut (*this).token_cache;
        if !is_null(cache.token)
            && cache.byte_index as usize == position
            && ts_subtree_external_scanner_state_eq(cache.last_external_token, last_external_token)
        {
            ts_language_table_entry(
                (*this).language,
                state,
                ts_subtree_symbol(cache.token),
                table_entry,
            );
            if parser_can_reuse_first_leaf(this, state, cache.token, table_entry) {
                ts_subtree_retain(cache.token);
                return cache.token;
            }
        }
        NULL_SUBTREE
    }
}

unsafe fn parser_set_cached_token(
    this: *mut Parser,
    byte_index: u32,
    last_external_token: Subtree,
    token: Subtree,
) {
    unsafe {
        let cache = &mut (*this).token_cache;
        if !is_null(token) {
            ts_subtree_retain(token);
        }
        if !is_null(last_external_token) {
            ts_subtree_retain(last_external_token);
        }
        if !is_null(cache.token) {
            ts_subtree_release(&mut (*this).tree_pool, cache.token);
        }
        if !is_null(cache.last_external_token) {
            ts_subtree_release(&mut (*this).tree_pool, cache.last_external_token);
        }
        cache.token = token;
        cache.byte_index = byte_index;
        cache.last_external_token = last_external_token;
    }
}

unsafe fn parser_has_included_range_difference(
    this: *const Parser,
    start_position: u32,
    end_position: u32,
) -> bool {
    unsafe {
        ts_range_array_intersects(
            &(*this).included_range_differences,
            (*this).included_range_difference_index,
            start_position,
            end_position,
        )
    }
}

unsafe fn parser_reuse_node(
    this: *mut Parser,
    version: StackVersion,
    state: *mut TSStateId,
    position: u32,
    last_external_token: Subtree,
    table_entry: *mut TableEntry,
) -> Subtree {
    unsafe {
        loop {
            let result = reusable_node_tree(&(*this).reusable_node);
            if is_null(result) {
                break;
            }
            let byte_offset = reusable_node_byte_offset(&(*this).reusable_node);
            let mut end_byte_offset = byte_offset.wrapping_add(ts_subtree_total_bytes(result));

            // Do not reuse an EOF node if the included ranges array has changes
            // later on in the file.
            if ts_subtree_is_eof(result) {
                end_byte_offset = u32::MAX;
            }

            if byte_offset > position {
                log!(
                    this,
                    c"before_reusable_node symbol:%s",
                    tree_name!(this, result)
                );
                break;
            }

            if byte_offset < position {
                log!(
                    this,
                    c"past_reusable_node symbol:%s",
                    tree_name!(this, result)
                );
                if end_byte_offset <= position || !reusable_node_descend(&mut (*this).reusable_node)
                {
                    reusable_node_advance(&mut (*this).reusable_node);
                }
                continue;
            }

            if !ts_subtree_external_scanner_state_eq(
                (*this).reusable_node.last_external_token,
                last_external_token,
            ) {
                log!(
                    this,
                    c"reusable_node_has_different_external_scanner_state symbol:%s",
                    tree_name!(this, result)
                );
                reusable_node_advance(&mut (*this).reusable_node);
                continue;
            }

            let reason: Option<&'static CStr> = if ts_subtree_has_changes(result) {
                Some(c"has_changes")
            } else if ts_subtree_is_error(result) {
                Some(c"is_error")
            } else if ts_subtree_missing(result) {
                Some(c"is_missing")
            } else if ts_subtree_is_fragile(result) {
                Some(c"is_fragile")
            } else if parser_has_included_range_difference(
                this,
                byte_offset,
                if ts_subtree_is_eof(result) {
                    end_byte_offset
                } else {
                    end_byte_offset.wrapping_add(ts_subtree_lookahead_bytes(result))
                },
            ) {
                Some(c"contains_different_included_range")
            } else {
                None
            };

            if let Some(reason) = reason {
                log!(
                    this,
                    c"cant_reuse_node_%s tree:%s",
                    reason.as_ptr(),
                    tree_name!(this, result)
                );
                if !reusable_node_descend(&mut (*this).reusable_node) {
                    reusable_node_advance(&mut (*this).reusable_node);
                    parser_breakdown_top_of_stack(this, version);
                    *state = ts_stack_state((*this).stack, version);
                }
                continue;
            }

            let leaf_symbol = ts_subtree_leaf_symbol(result);
            ts_language_table_entry((*this).language, *state, leaf_symbol, table_entry);
            if !parser_can_reuse_first_leaf(this, *state, result, table_entry) {
                log!(
                    this,
                    c"cant_reuse_node symbol:%s, first_leaf_symbol:%s",
                    tree_name!(this, result),
                    sym_name!(this, leaf_symbol)
                );
                reusable_node_advance_past_leaf(&mut (*this).reusable_node);
                break;
            }

            log!(this, c"reuse_node symbol:%s", tree_name!(this, result));
            ts_subtree_retain(result);
            return result;
        }

        NULL_SUBTREE
    }
}

/// Determine if a given tree should be replaced by an alternative tree.
///
/// The decision is based on the trees' error costs (if any), their dynamic precedence,
/// and finally, as a default, by a recursive comparison of the trees' symbols.
unsafe fn parser_select_tree(this: *mut Parser, left: Subtree, right: Subtree) -> bool {
    unsafe {
        if is_null(left) {
            return true;
        }
        if is_null(right) {
            return false;
        }

        if ts_subtree_error_cost(right) < ts_subtree_error_cost(left) {
            log!(
                this,
                c"select_smaller_error symbol:%s, over_symbol:%s",
                tree_name!(this, right),
                tree_name!(this, left)
            );
            return true;
        }

        if ts_subtree_error_cost(left) < ts_subtree_error_cost(right) {
            log!(
                this,
                c"select_smaller_error symbol:%s, over_symbol:%s",
                tree_name!(this, left),
                tree_name!(this, right)
            );
            return false;
        }

        if ts_subtree_dynamic_precedence(right) > ts_subtree_dynamic_precedence(left) {
            log!(
                this,
                c"select_higher_precedence symbol:%s, prec:%d, over_symbol:%s, other_prec:%d",
                tree_name!(this, right),
                ts_subtree_dynamic_precedence(right),
                tree_name!(this, left),
                ts_subtree_dynamic_precedence(left)
            );
            return true;
        }

        if ts_subtree_dynamic_precedence(left) > ts_subtree_dynamic_precedence(right) {
            log!(
                this,
                c"select_higher_precedence symbol:%s, prec:%d, over_symbol:%s, other_prec:%d",
                tree_name!(this, left),
                ts_subtree_dynamic_precedence(left),
                tree_name!(this, right),
                ts_subtree_dynamic_precedence(right)
            );
            return false;
        }

        if ts_subtree_error_cost(left) > 0 {
            return true;
        }

        let comparison = ts_subtree_compare(left, right, &mut (*this).tree_pool);
        match comparison {
            -1 => {
                log!(
                    this,
                    c"select_earlier symbol:%s, over_symbol:%s",
                    tree_name!(this, left),
                    tree_name!(this, right)
                );
                false
            }
            1 => {
                log!(
                    this,
                    c"select_earlier symbol:%s, over_symbol:%s",
                    tree_name!(this, right),
                    tree_name!(this, left)
                );
                true
            }
            _ => {
                log!(
                    this,
                    c"select_existing symbol:%s, over_symbol:%s",
                    tree_name!(this, left),
                    tree_name!(this, right)
                );
                false
            }
        }
    }
}

/// Determine if a given tree's children should be replaced by an alternative array of children.
unsafe fn parser_select_children(
    this: *mut Parser,
    left: Subtree,
    children: *const SubtreeArray,
) -> bool {
    unsafe {
        (*this).scratch_trees.assign(&*children);

        // Create a temporary subtree using the scratch trees array. This node does
        // not perform any allocation except for possibly growing the array to make
        // room for its own heap data. The scratch tree is never explicitly released,
        // so the same 'scratch trees' array can be reused again later.
        let scratch_tree = ts_subtree_new_node(
            ts_subtree_symbol(left),
            &mut (*this).scratch_trees,
            0,
            (*this).language,
        );

        parser_select_tree(this, left, ts_subtree_from_mut(scratch_tree))
    }
}

unsafe fn parser_shift(
    this: *mut Parser,
    version: StackVersion,
    state: TSStateId,
    lookahead: Subtree,
    extra: bool,
) {
    unsafe {
        let is_leaf = ts_subtree_child_count(lookahead) == 0;
        let mut subtree_to_push = lookahead;
        if extra != ts_subtree_extra(lookahead) && is_leaf {
            let mut result = ts_subtree_make_mut(&mut (*this).tree_pool, lookahead);
            ts_subtree_set_extra(&mut result, extra);
            subtree_to_push = ts_subtree_from_mut(result);
        }

        ts_stack_push((*this).stack, version, subtree_to_push, !is_leaf, state);
        if ts_subtree_has_external_tokens(subtree_to_push) {
            ts_stack_set_last_external_token(
                (*this).stack,
                version,
                ts_subtree_last_external_token(subtree_to_push),
            );
        }
    }
}

#[expect(clippy::too_many_arguments, reason = "mirrors ts_parser__reduce")]
unsafe fn parser_reduce(
    this: *mut Parser,
    version: StackVersion,
    symbol: TSSymbol,
    count: u32,
    dynamic_precedence: i32,
    production_id: u16,
    is_fragile: bool,
    end_of_non_terminal_extra: bool,
) -> StackVersion {
    unsafe {
        let initial_version_count = ts_stack_version_count((*this).stack);

        // Pop the given number of nodes from the given version of the parse stack.
        // If stack versions have previously merged, then there may be more than one
        // path back through the stack. For each path, create a new parent node to
        // contain the popped children, and push it onto the stack in place of the
        // children.
        let pop = ts_stack_pop_count((*this).stack, version, count);
        let mut removed_version_count = 0u32;
        let halted_version_count = ts_stack_halted_version_count((*this).stack);
        let mut i = 0u32;
        while i < pop.size {
            let mut slice = *pop.get(i);
            let slice_version = slice.version.wrapping_sub(removed_version_count);

            // This is where new versions are added to the parse stack. The versions
            // will all be sorted and truncated at the end of the outer parsing loop.
            // Allow the maximum version count to be temporarily exceeded, but only
            // by a limited threshold.
            if slice_version > MAX_VERSION_COUNT + MAX_VERSION_COUNT_OVERFLOW + halted_version_count
            {
                ts_stack_remove_version((*this).stack, slice_version);
                ts_subtree_array_delete(&mut (*this).tree_pool, &mut slice.subtrees);
                removed_version_count += 1;
                while i + 1 < pop.size {
                    log!(this, c"aborting reduce with too many versions");
                    let mut next_slice = *pop.get(i + 1);
                    if next_slice.version != slice.version {
                        break;
                    }
                    ts_subtree_array_delete(&mut (*this).tree_pool, &mut next_slice.subtrees);
                    i += 1;
                }
                i += 1;
                continue;
            }

            // Extra tokens on top of the stack should not be included in this new parent
            // node. They will be re-pushed onto the stack after the parent node is
            // created and pushed.
            let mut children = slice.subtrees;
            ts_subtree_array_remove_trailing_extras(&mut children, &mut (*this).trailing_extras);

            let mut parent = ts_subtree_new_node(
                symbol,
                &mut children,
                u32::from(production_id),
                (*this).language,
            );

            // This pop operation may have caused multiple stack versions to collapse
            // into one, because they all diverged from a common state. In that case,
            // choose one of the arrays of trees to be the parent node's children, and
            // delete the rest of the tree arrays.
            while i + 1 < pop.size {
                let mut next_slice = *pop.get(i + 1);
                if next_slice.version != slice.version {
                    break;
                }
                i += 1;

                let mut next_slice_children = next_slice.subtrees;
                ts_subtree_array_remove_trailing_extras(
                    &mut next_slice_children,
                    &mut (*this).trailing_extras2,
                );

                if parser_select_children(this, ts_subtree_from_mut(parent), &next_slice_children) {
                    ts_subtree_array_clear(&mut (*this).tree_pool, &mut (*this).trailing_extras);
                    ts_subtree_release(&mut (*this).tree_pool, ts_subtree_from_mut(parent));
                    let this_ref = &mut *this;
                    this_ref
                        .trailing_extras
                        .swap(&mut this_ref.trailing_extras2);
                    parent = ts_subtree_new_node(
                        symbol,
                        &mut next_slice_children,
                        u32::from(production_id),
                        (*this).language,
                    );
                } else {
                    (*this).trailing_extras2.clear();
                    ts_subtree_array_delete(&mut (*this).tree_pool, &mut next_slice.subtrees);
                }
            }

            let state = ts_stack_state((*this).stack, slice_version);
            let next_state = ts_language_next_state((*this).language, state, symbol);
            if end_of_non_terminal_extra && next_state == state {
                (*parent.ptr).set_extra(true);
            }
            if is_fragile || pop.size > 1 || initial_version_count > 1 {
                (*parent.ptr).set_fragile_left(true);
                (*parent.ptr).set_fragile_right(true);
                (*parent.ptr).parse_state = TS_TREE_STATE_NONE;
            } else {
                (*parent.ptr).parse_state = state;
            }
            (*parent.ptr).u.node.dynamic_precedence = (*parent.ptr)
                .u
                .node
                .dynamic_precedence
                .wrapping_add(dynamic_precedence);

            // Push the parent node onto the stack, along with any extra tokens that
            // were previously on top of the stack.
            ts_stack_push(
                (*this).stack,
                slice_version,
                ts_subtree_from_mut(parent),
                false,
                next_state,
            );
            for j in 0..(*this).trailing_extras.size {
                ts_stack_push(
                    (*this).stack,
                    slice_version,
                    *(*this).trailing_extras.get(j),
                    false,
                    next_state,
                );
            }

            for j in 0..slice_version {
                if j == version {
                    continue;
                }
                if ts_stack_merge((*this).stack, j, slice_version) {
                    removed_version_count += 1;
                    break;
                }
            }
            i += 1;
        }

        // Return the first new stack version that was created.
        if ts_stack_version_count((*this).stack) > initial_version_count {
            initial_version_count
        } else {
            STACK_VERSION_NONE
        }
    }
}

unsafe fn parser_accept(this: *mut Parser, version: StackVersion, lookahead: Subtree) {
    unsafe {
        ts_assert!(ts_subtree_is_eof(lookahead));
        ts_stack_push((*this).stack, version, lookahead, false, 1);

        let pop = ts_stack_pop_all((*this).stack, version);
        for i in 0..pop.size {
            let mut trees = (*pop.get(i)).subtrees;

            let mut root = NULL_SUBTREE;
            let mut j = trees.size.wrapping_sub(1);
            while j.wrapping_add(1) > 0 {
                let tree = *trees.get(j);
                if !ts_subtree_extra(tree) {
                    ts_assert!(!tree.data.is_inline());
                    let child_count = ts_subtree_child_count(tree);
                    let children = ts_subtree_children(tree);
                    for k in 0..child_count {
                        ts_subtree_retain(*children.add(k as usize));
                    }
                    trees.splice(j, 1, child_count, children);
                    root = ts_subtree_from_mut(ts_subtree_new_node(
                        ts_subtree_symbol(tree),
                        &mut trees,
                        u32::from((*tree.ptr).u.node.production_id),
                        (*this).language,
                    ));
                    ts_subtree_release(&mut (*this).tree_pool, tree);
                    break;
                }
                j = j.wrapping_sub(1);
            }

            ts_assert!(!is_null(root));
            (*this).accept_count += 1;

            if !is_null((*this).finished_tree) {
                if parser_select_tree(this, (*this).finished_tree, root) {
                    ts_subtree_release(&mut (*this).tree_pool, (*this).finished_tree);
                    (*this).finished_tree = root;
                } else {
                    ts_subtree_release(&mut (*this).tree_pool, root);
                }
            } else {
                (*this).finished_tree = root;
            }
        }

        ts_stack_remove_version((*this).stack, (*pop.get(0)).version);
        ts_stack_halt((*this).stack, version);
    }
}

unsafe fn parser_process_candidate_recovery_actions(
    this: *mut Parser,
    actions: *const TSParseAction,
    action_count: u32,
) -> bool {
    unsafe {
        let mut has_shift_action = false;
        for i in 0..action_count {
            let action = *actions.add(i as usize);
            match action.kind {
                TS_PARSE_ACTION_TYPE_SHIFT | TS_PARSE_ACTION_TYPE_RECOVER => {
                    if !action.shift.extra && !action.shift.repetition {
                        has_shift_action = true;
                    }
                }
                TS_PARSE_ACTION_TYPE_REDUCE if action.reduce.child_count > 0 => {
                    ts_reduce_action_set_add(
                        &mut (*this).reduce_actions,
                        ReduceAction {
                            symbol: action.reduce.symbol,
                            count: u32::from(action.reduce.child_count),
                            dynamic_precedence: i32::from(action.reduce.dynamic_precedence),
                            production_id: action.reduce.production_id,
                        },
                    );
                }
                _ => {}
            }
        }
        has_shift_action
    }
}

unsafe fn parser_do_all_potential_reductions(
    this: *mut Parser,
    starting_version: StackVersion,
    lookahead_symbol: TSSymbol,
) -> bool {
    unsafe {
        let initial_version_count = ts_stack_version_count((*this).stack);

        let mut can_shift_lookahead_symbol = false;
        let mut version = starting_version;
        let mut i = 0u32;
        loop {
            let version_count = ts_stack_version_count((*this).stack);
            if version >= version_count {
                break;
            }

            let mut merged = false;
            let mut j = initial_version_count;
            while j < version {
                if ts_stack_merge((*this).stack, j, version) {
                    merged = true;
                    break;
                }
                j += 1;
            }
            if merged {
                i += 1;
                continue;
            }

            let state = ts_stack_state((*this).stack, version);
            let mut has_shift_action = false;
            (*this).reduce_actions.clear();

            if lookahead_symbol != 0 {
                let mut entry = TableEntry {
                    actions: ptr::null(),
                    action_count: 0,
                    is_reusable: false,
                };
                ts_language_table_entry((*this).language, state, lookahead_symbol, &mut entry);
                has_shift_action = parser_process_candidate_recovery_actions(
                    this,
                    entry.actions,
                    entry.action_count,
                );
            } else {
                let mut iter = ts_language_lookaheads((*this).language, state);
                while ts_lookahead_iterator_next_inner(&mut iter) {
                    // only terminal tokens are valid lookaheads for reduction decisions
                    if iter.symbol == ts_builtin_sym_end
                        || u32::from(iter.symbol) >= (*(*this).language).token_count
                    {
                        continue;
                    }
                    if parser_process_candidate_recovery_actions(
                        this,
                        iter.actions,
                        u32::from(iter.action_count),
                    ) {
                        has_shift_action = true;
                    }
                }

                // Sort reduce_actions by symbol descending to ensure deterministic
                // ordering. The LookaheadIterator may visit symbols in a different
                // order than the original linear scan (group order vs symbol order
                // for small parse states), which can produce different orderings.
                // Since reductions are applied sequentially and the last reduction
                // version survives, the order affects error recovery outcomes.
                let contents = (*this).reduce_actions.contents;
                for j in 1..(*this).reduce_actions.size {
                    let key = *contents.add(j as usize);
                    let mut k = j as i32 - 1;
                    while k >= 0 && (*contents.add(k as usize)).symbol < key.symbol {
                        *contents.add(k as usize + 1) = *contents.add(k as usize);
                        k -= 1;
                    }
                    *contents.add((k + 1) as usize) = key;
                }
            }

            let mut reduction_version = STACK_VERSION_NONE;
            for j in 0..(*this).reduce_actions.size {
                let action = *(*this).reduce_actions.get(j);

                reduction_version = parser_reduce(
                    this,
                    version,
                    action.symbol,
                    action.count,
                    action.dynamic_precedence,
                    action.production_id,
                    true,
                    false,
                );
            }

            if has_shift_action {
                can_shift_lookahead_symbol = true;
            } else if reduction_version != STACK_VERSION_NONE && i < MAX_VERSION_COUNT {
                ts_stack_renumber_version((*this).stack, reduction_version, version);
                i += 1;
                continue;
            } else if lookahead_symbol != 0 {
                ts_stack_remove_version((*this).stack, version);
                // Removing this version shifts the next reduction version into its slot.
                // Check that version before advancing.
                i += 1;
                continue;
            }

            if version == starting_version {
                version = version_count;
            } else {
                version += 1;
            }
            i += 1;
        }

        can_shift_lookahead_symbol
    }
}

unsafe fn parser_recover_to_state(
    this: *mut Parser,
    version: StackVersion,
    depth: u32,
    goal_state: TSStateId,
) -> bool {
    unsafe {
        let mut pop = ts_stack_pop_count((*this).stack, version, depth);
        let mut previous_version = STACK_VERSION_NONE;

        let mut i = 0u32;
        while i < pop.size {
            let mut slice = *pop.get(i);

            if slice.version == previous_version {
                ts_subtree_array_delete(&mut (*this).tree_pool, &mut slice.subtrees);
                pop.erase(i);
                continue;
            }

            if ts_stack_state((*this).stack, slice.version) != goal_state {
                ts_stack_halt((*this).stack, slice.version);
                ts_subtree_array_delete(&mut (*this).tree_pool, &mut slice.subtrees);
                pop.erase(i);
                continue;
            }

            let mut error_trees = ts_stack_pop_error((*this).stack, slice.version);
            if error_trees.size > 0 {
                ts_assert!(error_trees.size == 1);
                let error_tree = *error_trees.get(0);
                let error_child_count = ts_subtree_child_count(error_tree);
                if error_child_count > 0 {
                    let mut nested: SubtreeArray = Array::new();
                    nested.reserve(error_child_count);
                    for j in 0..error_child_count {
                        let child = *ts_subtree_children(error_tree).add(j as usize);
                        ts_subtree_retain(child);
                        nested.push(child);
                    }
                    let nested_error = ts_subtree_from_mut(ts_subtree_new_node(
                        ts_builtin_sym_error_repeat,
                        &mut nested,
                        0,
                        (*this).language,
                    ));
                    slice.subtrees.insert(0, nested_error);
                }
                ts_subtree_array_delete(&mut (*this).tree_pool, &mut error_trees);
            }

            ts_subtree_array_remove_trailing_extras(
                &mut slice.subtrees,
                &mut (*this).trailing_extras,
            );

            if slice.subtrees.size > 0 {
                let error = ts_subtree_new_error_node(&mut slice.subtrees, true, (*this).language);
                ts_stack_push((*this).stack, slice.version, error, false, goal_state);
            } else {
                slice.subtrees.delete();
            }

            for j in 0..(*this).trailing_extras.size {
                let tree = *(*this).trailing_extras.get(j);
                ts_stack_push((*this).stack, slice.version, tree, false, goal_state);
            }

            previous_version = slice.version;
            i += 1;
        }

        previous_version != STACK_VERSION_NONE
    }
}

unsafe fn parser_recover(this: *mut Parser, version: StackVersion, mut lookahead: Subtree) {
    unsafe {
        let mut did_recover = false;
        let previous_version_count = ts_stack_version_count((*this).stack);
        let position = ts_stack_position((*this).stack, version);
        let summary = ts_stack_get_summary((*this).stack, version);
        let node_count_since_error = ts_stack_node_count_since_error((*this).stack, version);
        let current_error_cost = ts_stack_error_cost((*this).stack, version);

        // When the parser is in the error state, there are two strategies for recovering with a
        // given lookahead token:
        // 1. Find a previous state on the stack in which that lookahead token would be valid. Then,
        //    create a new stack version that is in that state again. This entails popping all of the
        //    subtrees that have been pushed onto the stack since that previous state, and wrapping
        //    them in an ERROR node.
        // 2. Wrap the lookahead token in an ERROR node, push that ERROR node onto the stack, and
        //    move on to the next lookahead token, remaining in the error state.
        //
        // First, try the strategy 1. Upon entering the error state, the parser recorded a summary
        // of the previous parse states and their depths. Look at each state in the summary, to see
        // if the current lookahead token would be valid in that state.
        if !summary.is_null() && !ts_subtree_is_error(lookahead) {
            for i in 0..(*summary).size {
                let entry = *(*summary).get(i);

                if entry.state == ERROR_STATE {
                    continue;
                }
                if entry.position.bytes == position.bytes {
                    continue;
                }
                let mut depth = entry.depth;
                if node_count_since_error > 0 {
                    depth += 1;
                }

                // Do not recover in ways that create redundant stack versions.
                let mut would_merge = false;
                for j in 0..previous_version_count {
                    if ts_stack_state((*this).stack, j) == entry.state
                        && ts_stack_position((*this).stack, j).bytes == position.bytes
                    {
                        would_merge = true;
                        break;
                    }
                }
                if would_merge {
                    continue;
                }

                // Do not recover if the result would clearly be worse than some existing stack version.
                let new_cost = current_error_cost
                    .wrapping_add(entry.depth.wrapping_mul(ERROR_COST_PER_SKIPPED_TREE))
                    .wrapping_add(
                        position
                            .bytes
                            .wrapping_sub(entry.position.bytes)
                            .wrapping_mul(ERROR_COST_PER_SKIPPED_CHAR),
                    )
                    .wrapping_add(
                        position
                            .extent
                            .row
                            .wrapping_sub(entry.position.extent.row)
                            .wrapping_mul(ERROR_COST_PER_SKIPPED_LINE),
                    );
                if parser_better_version_exists(this, version, false, new_cost) {
                    break;
                }

                // If the current lookahead token is valid in some previous state, recover to that state.
                // Then stop looking for further recoveries.
                if ts_language_has_actions(
                    (*this).language,
                    entry.state,
                    ts_subtree_symbol(lookahead),
                ) && parser_recover_to_state(this, version, depth, entry.state)
                {
                    did_recover = true;
                    log!(
                        this,
                        c"recover_to_previous state:%u, depth:%u",
                        u32::from(entry.state),
                        depth
                    );
                    log_stack!(this);
                    break;
                }
            }
        }

        // In the process of attempting to recover, some stack versions may have been created
        // and subsequently halted. Remove those versions.
        let mut i = previous_version_count;
        while i < ts_stack_version_count((*this).stack) {
            if !ts_stack_is_active((*this).stack, i) {
                log!(this, c"removed paused version:%u", i);
                ts_stack_remove_version((*this).stack, i);
                i = i.wrapping_sub(1);
                log_stack!(this);
            }
            i = i.wrapping_add(1);
        }

        // If the parser is still in the error state at the end of the file, just wrap everything
        // in an ERROR node and terminate.
        if ts_subtree_is_eof(lookahead) {
            log!(this, c"recover_eof");
            let mut children: SubtreeArray = Array::new();
            let parent = ts_subtree_new_error_node(&mut children, false, (*this).language);
            ts_stack_push((*this).stack, version, parent, false, 1);
            parser_accept(this, version, lookahead);
            return;
        }

        // If strategy 1 succeeded, a new stack version will have been created which is able to handle
        // the current lookahead token. Now, in addition, try strategy 2 described above: skip the
        // current lookahead token by wrapping it in an ERROR node.

        // Don't pursue this additional strategy if there are already too many stack versions.
        if did_recover && ts_stack_version_count((*this).stack) > MAX_VERSION_COUNT {
            ts_stack_halt((*this).stack, version);
            ts_subtree_release(&mut (*this).tree_pool, lookahead);
            return;
        }

        if did_recover && ts_subtree_has_external_scanner_state_change(lookahead) {
            ts_stack_halt((*this).stack, version);
            ts_subtree_release(&mut (*this).tree_pool, lookahead);
            return;
        }

        // Do not recover if the result would clearly be worse than some existing stack version.
        let new_cost = current_error_cost
            .wrapping_add(ERROR_COST_PER_SKIPPED_TREE)
            .wrapping_add(
                ts_subtree_total_bytes(lookahead).wrapping_mul(ERROR_COST_PER_SKIPPED_CHAR),
            )
            .wrapping_add(
                ts_subtree_total_size(lookahead)
                    .extent
                    .row
                    .wrapping_mul(ERROR_COST_PER_SKIPPED_LINE),
            );
        if parser_better_version_exists(this, version, false, new_cost) {
            ts_stack_halt((*this).stack, version);
            ts_subtree_release(&mut (*this).tree_pool, lookahead);
            return;
        }

        // If the current lookahead token is an extra token, mark it as extra. This means it won't
        // be counted in error cost calculations.
        let mut n = 0u32;
        let actions =
            ts_language_actions((*this).language, 1, ts_subtree_symbol(lookahead), &mut n);
        if n > 0 {
            let last = *actions.add(n as usize - 1);
            if last.kind == TS_PARSE_ACTION_TYPE_SHIFT && last.shift.extra {
                let mut mutable_lookahead = ts_subtree_make_mut(&mut (*this).tree_pool, lookahead);
                ts_subtree_set_extra(&mut mutable_lookahead, true);
                lookahead = ts_subtree_from_mut(mutable_lookahead);
            }
        }

        // Wrap the lookahead token in an ERROR.
        log!(this, c"skip_token symbol:%s", tree_name!(this, lookahead));
        let mut children: SubtreeArray = Array::new();
        children.reserve(1);
        children.push(lookahead);
        let mut error_repeat = ts_subtree_new_node(
            ts_builtin_sym_error_repeat,
            &mut children,
            0,
            (*this).language,
        );

        // If other tokens have already been skipped, so there is already an ERROR at the top of the
        // stack, then pop that ERROR off the stack and wrap the two ERRORs together into one larger
        // ERROR.
        if node_count_since_error > 0 {
            let pop = ts_stack_pop_count((*this).stack, version, 1);

            // TODO: Figure out how to make this condition occur.
            // See https://github.com/atom/atom/issues/18450#issuecomment-439579778
            // If multiple stack versions have merged at this point, just pick one of the errors
            // arbitrarily and discard the rest.
            if pop.size > 1 {
                for i in 1..pop.size {
                    ts_subtree_array_delete(&mut (*this).tree_pool, &mut (*pop.get(i)).subtrees);
                }
                while ts_stack_version_count((*this).stack) > (*pop.get(0)).version + 1 {
                    ts_stack_remove_version((*this).stack, (*pop.get(0)).version + 1);
                }
            }

            ts_stack_renumber_version((*this).stack, (*pop.get(0)).version, version);
            (*pop.get(0))
                .subtrees
                .push(ts_subtree_from_mut(error_repeat));
            error_repeat = ts_subtree_new_node(
                ts_builtin_sym_error_repeat,
                &mut (*pop.get(0)).subtrees,
                0,
                (*this).language,
            );
        }

        // Push the new ERROR onto the stack.
        ts_stack_push(
            (*this).stack,
            version,
            ts_subtree_from_mut(error_repeat),
            false,
            ERROR_STATE,
        );
        if ts_subtree_has_external_tokens(lookahead) {
            ts_stack_set_last_external_token(
                (*this).stack,
                version,
                ts_subtree_last_external_token(lookahead),
            );
        }

        let mut has_error = true;
        for i in 0..ts_stack_version_count((*this).stack) {
            let status = parser_version_status(this, i);
            if !status.is_in_error {
                has_error = false;
                break;
            }
        }
        (*this).has_error = has_error;
    }
}

unsafe fn parser_handle_error(this: *mut Parser, version: StackVersion, mut lookahead: Subtree) {
    unsafe {
        let previous_version_count = ts_stack_version_count((*this).stack);

        // Perform any reductions that can happen in this state, regardless of the lookahead. After
        // skipping one or more invalid tokens, the parser might find a token that would have allowed
        // a reduction to take place.
        parser_do_all_potential_reductions(this, version, 0);
        let version_count = ts_stack_version_count((*this).stack);
        let position = ts_stack_position((*this).stack, version);

        // Push a discontinuity onto the stack. Merge all of the stack versions that
        // were created in the previous step.
        let mut did_insert_missing_token = false;
        let mut v = version;
        while v < version_count {
            if !did_insert_missing_token {
                let state = ts_stack_state((*this).stack, v);
                let mut missing_symbol: TSSymbol = 1;
                while missing_symbol < (*(*this).language).token_count as u16 {
                    let state_after_missing_symbol =
                        ts_language_next_state((*this).language, state, missing_symbol);
                    if state_after_missing_symbol == 0 || state_after_missing_symbol == state {
                        missing_symbol += 1;
                        continue;
                    }

                    if ts_language_has_reduce_action(
                        (*this).language,
                        state_after_missing_symbol,
                        ts_subtree_leaf_symbol(lookahead),
                    ) {
                        // In case the parser is currently outside of any included range, the lexer will
                        // snap to the beginning of the next included range. The missing token's padding
                        // must be assigned to position it within the next included range.
                        ts_lexer_reset(&mut (*this).lexer, position);
                        ts_lexer_mark_end(&mut (*this).lexer);
                        let padding = length_sub((*this).lexer.token_end_position, position);
                        let lookahead_bytes = ts_subtree_total_bytes(lookahead)
                            .wrapping_add(ts_subtree_lookahead_bytes(lookahead));

                        let version_with_missing_tree = ts_stack_copy_version((*this).stack, v);
                        let missing_tree = ts_subtree_new_missing_leaf(
                            &mut (*this).tree_pool,
                            missing_symbol,
                            state,
                            padding,
                            lookahead_bytes,
                            (*this).language,
                        );
                        ts_stack_push(
                            (*this).stack,
                            version_with_missing_tree,
                            missing_tree,
                            false,
                            state_after_missing_symbol,
                        );

                        if parser_do_all_potential_reductions(
                            this,
                            version_with_missing_tree,
                            ts_subtree_leaf_symbol(lookahead),
                        ) {
                            log!(
                                this,
                                c"recover_with_missing symbol:%s, state:%u",
                                sym_name!(this, missing_symbol),
                                u32::from(ts_stack_state((*this).stack, version_with_missing_tree))
                            );
                            did_insert_missing_token = true;
                            break;
                        }
                    }
                    missing_symbol += 1;
                }
            }

            ts_stack_push((*this).stack, v, NULL_SUBTREE, false, ERROR_STATE);
            v = if v == version {
                previous_version_count
            } else {
                v + 1
            };
        }

        for _ in previous_version_count..version_count {
            let did_merge = ts_stack_merge((*this).stack, version, previous_version_count);
            ts_assert!(did_merge);
        }

        ts_stack_record_summary((*this).stack, version, MAX_SUMMARY_DEPTH);

        // Begin recovery with the current lookahead node, rather than waiting for the
        // next turn of the parse loop. This ensures that the tree accounts for the
        // current lookahead token's "lookahead bytes" value, which describes how far
        // the lexer needed to look ahead beyond the content of the token in order to
        // recognize it.
        if ts_subtree_child_count(lookahead) > 0 {
            parser_breakdown_lookahead(
                this,
                &mut lookahead,
                ERROR_STATE,
                &mut (*this).reusable_node,
            );
        }
        parser_recover(this, version, lookahead);

        log_stack!(this);
    }
}

unsafe fn parser_check_progress(
    this: *mut Parser,
    lookahead: *mut Subtree,
    position: *const u32,
    operations: u32,
) -> bool {
    unsafe {
        (*this).operation_count = (*this).operation_count.wrapping_add(operations);
        if (*this).operation_count >= OP_COUNT_PER_PARSER_CALLBACK_CHECK {
            (*this).operation_count = 0;
        }
        if !position.is_null() {
            (*this).parse_state.current_byte_offset = *position;
            (*this).parse_state.has_error = (*this).has_error;
        }
        if (*this).operation_count == 0
            && let Some(callback) = (*this).parse_options.progress_callback
            && callback(&mut (*this).parse_state)
        {
            if !lookahead.is_null() && !is_null(*lookahead) {
                ts_subtree_release(&mut (*this).tree_pool, *lookahead);
            }
            return false;
        }
        true
    }
}

unsafe fn parser_advance(this: *mut Parser, version: StackVersion, allow_node_reuse: bool) -> bool {
    unsafe {
        let mut state = ts_stack_state((*this).stack, version);
        let position = ts_stack_position((*this).stack, version).bytes;
        let last_external_token = ts_stack_last_external_token((*this).stack, version);

        let mut did_reuse = true;
        let mut lookahead = NULL_SUBTREE;
        let mut table_entry = TableEntry {
            actions: ptr::null(),
            action_count: 0,
            is_reusable: false,
        };

        // If possible, reuse a node from the previous syntax tree.
        if allow_node_reuse {
            lookahead = parser_reuse_node(
                this,
                version,
                &mut state,
                position,
                last_external_token,
                &mut table_entry,
            );
        }

        // If no node from the previous syntax tree could be reused, then try to
        // reuse the token previously returned by the lexer.
        if is_null(lookahead) {
            did_reuse = false;
            lookahead = parser_get_cached_token(
                this,
                state,
                position as usize,
                last_external_token,
                &mut table_entry,
            );
        }

        let mut needs_lex = is_null(lookahead);
        loop {
            // Otherwise, re-run the lexer.
            if needs_lex {
                needs_lex = false;
                lookahead = parser_lex(this, version, state);
                if (*this).has_scanner_error {
                    return false;
                }

                if !is_null(lookahead) {
                    parser_set_cached_token(this, position, last_external_token, lookahead);
                    ts_language_table_entry(
                        (*this).language,
                        state,
                        ts_subtree_symbol(lookahead),
                        &mut table_entry,
                    );
                }
                // When parsing a non-terminal extra, a null lookahead indicates the
                // end of the rule. The reduction is stored in the EOF table entry.
                // After the reduction, the lexer needs to be run again.
                else {
                    ts_language_table_entry(
                        (*this).language,
                        state,
                        ts_builtin_sym_end,
                        &mut table_entry,
                    );
                }
            }

            // If a progress callback was provided, then check every
            // time a fixed number of parse actions has been processed.
            if !parser_check_progress(this, &mut lookahead, &position, 1) {
                return false;
            }

            // Process each parse action for the current lookahead token in
            // the current state. If there are multiple actions, then this is
            // an ambiguous state. REDUCE actions always create a new stack
            // version, whereas SHIFT actions update the existing stack version
            // and terminate this loop.
            let mut did_reduce = false;
            let mut last_reduction_version = STACK_VERSION_NONE;
            for i in 0..table_entry.action_count {
                let action = *table_entry.actions.add(i as usize);

                match action.kind {
                    TS_PARSE_ACTION_TYPE_SHIFT => {
                        if action.shift.repetition {
                            continue;
                        }
                        let mut next_state;
                        if action.shift.extra {
                            next_state = state;
                            log!(this, c"shift_extra");
                        } else {
                            next_state = action.shift.state;
                            log!(this, c"shift state:%u", u32::from(next_state));
                        }

                        if ts_subtree_child_count(lookahead) > 0 {
                            parser_breakdown_lookahead(
                                this,
                                &mut lookahead,
                                state,
                                &mut (*this).reusable_node,
                            );
                            next_state = ts_language_next_state(
                                (*this).language,
                                state,
                                ts_subtree_symbol(lookahead),
                            );
                        }

                        parser_shift(this, version, next_state, lookahead, action.shift.extra);
                        if did_reuse {
                            reusable_node_advance(&mut (*this).reusable_node);
                        }
                        return true;
                    }

                    TS_PARSE_ACTION_TYPE_REDUCE => {
                        let is_fragile = table_entry.action_count > 1;
                        let end_of_non_terminal_extra = is_null(lookahead);
                        log!(
                            this,
                            c"reduce sym:%s, child_count:%u",
                            sym_name!(this, action.reduce.symbol),
                            u32::from(action.reduce.child_count)
                        );
                        let reduction_version = parser_reduce(
                            this,
                            version,
                            action.reduce.symbol,
                            u32::from(action.reduce.child_count),
                            i32::from(action.reduce.dynamic_precedence),
                            action.reduce.production_id,
                            is_fragile,
                            end_of_non_terminal_extra,
                        );
                        did_reduce = true;
                        if reduction_version != STACK_VERSION_NONE {
                            last_reduction_version = reduction_version;
                        }
                    }

                    TS_PARSE_ACTION_TYPE_ACCEPT => {
                        log!(this, c"accept");
                        parser_accept(this, version, lookahead);
                        return true;
                    }

                    TS_PARSE_ACTION_TYPE_RECOVER => {
                        if ts_subtree_child_count(lookahead) > 0 {
                            parser_breakdown_lookahead(
                                this,
                                &mut lookahead,
                                ERROR_STATE,
                                &mut (*this).reusable_node,
                            );
                        }

                        parser_recover(this, version, lookahead);
                        if did_reuse {
                            reusable_node_advance(&mut (*this).reusable_node);
                        }
                        return true;
                    }

                    _ => {}
                }
            }

            // If a reduction was performed, then replace the current stack version
            // with one of the stack versions created by a reduction, and continue
            // processing this version of the stack with the same lookahead symbol.
            if last_reduction_version != STACK_VERSION_NONE {
                ts_stack_renumber_version((*this).stack, last_reduction_version, version);
                log_stack!(this);
                state = ts_stack_state((*this).stack, version);

                // At the end of a non-terminal extra rule, the lexer will return a
                // null subtree, because the parser needs to perform a fixed reduction
                // regardless of the lookahead node. After performing that reduction,
                // (and completing the non-terminal extra rule) run the lexer again based
                // on the current parse state.
                if is_null(lookahead) {
                    needs_lex = true;
                } else {
                    ts_language_table_entry(
                        (*this).language,
                        state,
                        ts_subtree_leaf_symbol(lookahead),
                        &mut table_entry,
                    );
                }

                continue;
            }

            // A reduction was performed, but was merged into an existing stack version.
            // This version can be discarded.
            if did_reduce {
                if !is_null(lookahead) {
                    ts_subtree_release(&mut (*this).tree_pool, lookahead);
                }
                ts_stack_halt((*this).stack, version);
                return true;
            }

            // If the current lookahead token is a keyword that is not valid, but the
            // default word token *is* valid, then treat the lookahead token as the word
            // token instead.
            if ts_subtree_is_keyword(lookahead)
                && ts_subtree_symbol(lookahead) != (*(*this).language).keyword_capture_token
                && !ts_language_is_reserved_word(
                    (*this).language,
                    state,
                    ts_subtree_symbol(lookahead),
                )
            {
                ts_language_table_entry(
                    (*this).language,
                    state,
                    (*(*this).language).keyword_capture_token,
                    &mut table_entry,
                );
                if table_entry.action_count > 0 {
                    log!(
                        this,
                        c"switch from_keyword:%s, to_word_token:%s",
                        tree_name!(this, lookahead),
                        sym_name!(this, (*(*this).language).keyword_capture_token)
                    );

                    let mut mutable_lookahead =
                        ts_subtree_make_mut(&mut (*this).tree_pool, lookahead);
                    ts_subtree_set_symbol(
                        &mut mutable_lookahead,
                        (*(*this).language).keyword_capture_token,
                        (*this).language,
                    );
                    lookahead = ts_subtree_from_mut(mutable_lookahead);
                    continue;
                }
            }

            // If the current lookahead token is not valid and the parser is
            // already in the error state, restart the error recovery process.
            if state == ERROR_STATE {
                parser_recover(this, version, lookahead);
                return true;
            }

            // If the current lookahead token is not valid and the previous subtree on
            // the stack was reused from an old tree, then it wasn't actually valid to
            // reuse that previous subtree. Remove it from the stack, and in its place,
            // push each of its children. Then try again to process the current lookahead.
            if parser_breakdown_top_of_stack(this, version) {
                state = ts_stack_state((*this).stack, version);
                ts_subtree_release(&mut (*this).tree_pool, lookahead);
                needs_lex = true;
                continue;
            }

            // Otherwise, there is definitely an error in this version of the parse stack.
            // Mark this version as paused and continue processing any other stack
            // versions that exist. If some other version advances successfully, then
            // this version can simply be removed. But if all versions end up paused,
            // then error recovery is needed.
            log!(
                this,
                c"detect_error lookahead:%s",
                tree_name!(this, lookahead)
            );
            ts_stack_pause((*this).stack, version, lookahead);
            return true;
        }
    }
}

unsafe fn parser_condense_stack(this: *mut Parser) -> u32 {
    unsafe {
        let mut made_changes = false;
        let mut min_error_cost = u32::MAX;
        let mut i: StackVersion = 0;
        while i < ts_stack_version_count((*this).stack) {
            // Prune any versions that have been marked for removal.
            if ts_stack_is_halted((*this).stack, i) {
                ts_stack_remove_version((*this).stack, i);
                i = i.wrapping_sub(1);
                i = i.wrapping_add(1);
                continue;
            }

            // Keep track of the minimum error cost of any stack version so
            // that it can be returned.
            let status_i = parser_version_status(this, i);
            if !status_i.is_in_error && status_i.cost < min_error_cost {
                min_error_cost = status_i.cost;
            }

            // Examine each pair of stack versions, removing any versions that
            // are clearly worse than another version. Ensure that the versions
            // are ordered from most promising to least promising.
            let mut j: StackVersion = 0;
            while j < i {
                let status_j = parser_version_status(this, j);

                match parser_compare_versions(status_j, status_i) {
                    ErrorComparison::TakeLeft => {
                        made_changes = true;
                        ts_stack_remove_version((*this).stack, i);
                        i = i.wrapping_sub(1);
                        j = i;
                    }

                    ErrorComparison::PreferLeft | ErrorComparison::None => {
                        if ts_stack_merge((*this).stack, j, i) {
                            made_changes = true;
                            i = i.wrapping_sub(1);
                            j = i;
                        }
                    }

                    ErrorComparison::PreferRight => {
                        made_changes = true;
                        if ts_stack_merge((*this).stack, j, i) {
                            i = i.wrapping_sub(1);
                            j = i;
                        } else {
                            ts_stack_swap_versions((*this).stack, i, j);
                        }
                    }

                    ErrorComparison::TakeRight => {
                        made_changes = true;
                        ts_stack_remove_version((*this).stack, j);
                        i = i.wrapping_sub(1);
                        j = j.wrapping_sub(1);
                    }
                }
                j = j.wrapping_add(1);
            }
            i = i.wrapping_add(1);
        }

        // Enforce a hard upper bound on the number of stack versions by
        // discarding the least promising versions.
        while ts_stack_version_count((*this).stack) > MAX_VERSION_COUNT {
            ts_stack_remove_version((*this).stack, MAX_VERSION_COUNT);
            made_changes = true;
        }

        // If the best-performing stack version is currently paused, or all
        // versions are paused, then resume the best paused version and begin
        // the error recovery process. Otherwise, remove the paused versions.
        if ts_stack_version_count((*this).stack) > 0 {
            let mut has_unpaused_version = false;
            let mut i: StackVersion = 0;
            let mut n = ts_stack_version_count((*this).stack);
            while i < n {
                if ts_stack_is_paused((*this).stack, i) {
                    if !has_unpaused_version && (*this).accept_count < MAX_VERSION_COUNT {
                        log!(this, c"resume version:%u", i);
                        min_error_cost = ts_stack_error_cost((*this).stack, i);
                        let lookahead = ts_stack_resume((*this).stack, i);
                        parser_handle_error(this, i, lookahead);
                        has_unpaused_version = true;
                    } else {
                        ts_stack_remove_version((*this).stack, i);
                        made_changes = true;
                        i = i.wrapping_sub(1);
                        n = n.wrapping_sub(1);
                    }
                } else {
                    has_unpaused_version = true;
                }
                i = i.wrapping_add(1);
            }
        }

        if made_changes {
            log!(this, c"condense");
            log_stack!(this);
        }

        min_error_cost
    }
}

unsafe fn parser_balance_subtree(this: *mut Parser) -> bool {
    unsafe {
        let finished_tree = (*this).finished_tree;

        // If we haven't canceled balancing in progress before, then we want to clear the tree stack and
        // push the initial finished tree onto it. Otherwise, if we're resuming balancing after a
        // cancellation, we don't want to clear the tree stack.
        if !(*this).canceled_balancing {
            (*this).tree_pool.tree_stack.clear();
            if ts_subtree_child_count(finished_tree) > 0 && (*finished_tree.ptr).ref_count == 1 {
                (*this)
                    .tree_pool
                    .tree_stack
                    .push(ts_subtree_to_mut_unsafe(finished_tree));
            }
        }

        while (*this).tree_pool.tree_stack.size > 0 {
            if !parser_check_progress(this, ptr::null_mut(), ptr::null(), 1) {
                return false;
            }

            let tree = *(*this)
                .tree_pool
                .tree_stack
                .get((*this).tree_pool.tree_stack.size - 1);

            if (*tree.ptr).u.node.repeat_depth > 0 {
                let child1 = *ts_subtree_children(ts_subtree_from_mut(tree));
                let child2 = *ts_subtree_children(ts_subtree_from_mut(tree))
                    .add((*tree.ptr).child_count as usize - 1);
                let repeat_delta = i64::from(ts_subtree_repeat_depth(child1))
                    - i64::from(ts_subtree_repeat_depth(child2));
                if repeat_delta > 0 {
                    let n = repeat_delta as u32;

                    let mut i = n / 2;
                    while i > 0 {
                        ts_subtree_compress(
                            tree,
                            i,
                            (*this).language,
                            &mut (*this).tree_pool.tree_stack,
                        );

                        // We scale the operation count increment in `ts_parser__check_progress` proportionately to the compression
                        // size since larger values of i take longer to process. Shifting by 4 empirically provides good check
                        // intervals (e.g. 193 operations when i=3100) to prevent blocking during large compressions.
                        let operations = if i >> 4 > 0 { (i >> 4) as u8 } else { 1u8 };
                        if !parser_check_progress(
                            this,
                            ptr::null_mut(),
                            ptr::null(),
                            u32::from(operations),
                        ) {
                            return false;
                        }
                        i /= 2;
                    }
                }
            }

            let _ = (*this).tree_pool.tree_stack.pop();

            for i in 0..(*tree.ptr).child_count {
                let child = *ts_subtree_children(ts_subtree_from_mut(tree)).add(i as usize);
                if ts_subtree_child_count(child) > 0 && (*child.ptr).ref_count == 1 {
                    (*this)
                        .tree_pool
                        .tree_stack
                        .push(ts_subtree_to_mut_unsafe(child));
                }
            }
        }

        true
    }
}

unsafe fn parser_has_outstanding_parse(this: *mut Parser) -> bool {
    unsafe {
        (*this).canceled_balancing
            || !(*this).external_scanner_payload.is_null()
            || ts_stack_state((*this).stack, 0) != 1
            || ts_stack_node_count_since_error((*this).stack, 0) != 0
    }
}

// ---- Parser - public ----

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_parser_new() -> *mut TSParser {
    unsafe {
        let this = ts_calloc(1, size_of::<Parser>()) as *mut Parser;
        ts_lexer_init(&mut (*this).lexer);
        (*this).reduce_actions = Array::new();
        (*this).reduce_actions.reserve(4);
        (*this).tree_pool = ts_subtree_pool_new(32);
        (*this).stack = ts_stack_new(&mut (*this).tree_pool);
        (*this).finished_tree = NULL_SUBTREE;
        (*this).reusable_node = reusable_node_new();
        (*this).dot_graph_file = ptr::null_mut();
        (*this).language = ptr::null();
        (*this).has_scanner_error = false;
        (*this).has_error = false;
        (*this).canceled_balancing = false;
        (*this).external_scanner_payload = ptr::null_mut();
        (*this).operation_count = 0;
        (*this).old_tree = NULL_SUBTREE;
        (*this).included_range_differences = Array::new();
        (*this).included_range_difference_index = 0;
        parser_set_cached_token(this, 0, NULL_SUBTREE, NULL_SUBTREE);
        this as *mut TSParser
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_parser_delete(parser: *mut TSParser) {
    unsafe {
        let this = parser as *mut Parser;
        if this.is_null() {
            return;
        }

        ts_parser_set_language(parser, ptr::null());
        ts_stack_delete((*this).stack);
        if !(*this).reduce_actions.contents.is_null() {
            (*this).reduce_actions.delete();
        }
        if !(*this).included_range_differences.contents.is_null() {
            (*this).included_range_differences.delete();
        }
        if !is_null((*this).old_tree) {
            ts_subtree_release(&mut (*this).tree_pool, (*this).old_tree);
            (*this).old_tree = NULL_SUBTREE;
        }
        ts_wasm_store_delete((*this).wasm_store);
        ts_lexer_delete(&mut (*this).lexer);
        parser_set_cached_token(this, 0, NULL_SUBTREE, NULL_SUBTREE);
        ts_subtree_pool_delete(&mut (*this).tree_pool);
        reusable_node_delete(&mut (*this).reusable_node);
        (*this).trailing_extras.delete();
        (*this).trailing_extras2.delete();
        (*this).scratch_trees.delete();
        ts_free(this as *mut c_void);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_parser_language(parser: *const TSParser) -> *const TSLanguage {
    unsafe { (*(parser as *const Parser)).language }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_parser_set_language(
    parser: *mut TSParser,
    language: *const TSLanguage,
) -> bool {
    unsafe {
        let this = parser as *mut Parser;
        ts_parser_reset(parser);
        ts_language_delete((*this).language);
        (*this).language = ptr::null();

        if !language.is_null() {
            if (*language).abi_version > TREE_SITTER_LANGUAGE_VERSION
                || (*language).abi_version < TREE_SITTER_MIN_COMPATIBLE_LANGUAGE_VERSION
            {
                return false;
            }

            if !ts_language_is_parseable(language) {
                return false;
            }

            if ts_language_is_wasm(language)
                && ((*this).wasm_store.is_null()
                    || !ts_wasm_store_start((*this).wasm_store, &mut (*this).lexer.data, language))
            {
                return false;
            }
        }

        (*this).language = ts_language_copy(language);
        true
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_parser_logger(parser: *const TSParser) -> TSLogger {
    unsafe { ptr::read(&(*(parser as *const Parser)).lexer.logger) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_parser_set_logger(parser: *mut TSParser, logger: TSLogger) {
    unsafe { (*(parser as *mut Parser)).lexer.logger = logger }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_parser_print_dot_graphs(parser: *mut TSParser, fd: c_int) {
    unsafe {
        let this = parser as *mut Parser;
        if !(*this).dot_graph_file.is_null() {
            fclose((*this).dot_graph_file);
        }

        if fd >= 0 {
            (*this).dot_graph_file = fdopen(fd, c"a".as_ptr());
        } else {
            (*this).dot_graph_file = ptr::null_mut();
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_parser_set_included_ranges(
    parser: *mut TSParser,
    ranges: *const TSRange,
    count: u32,
) -> bool {
    unsafe { ts_lexer_set_included_ranges(&mut (*(parser as *mut Parser)).lexer, ranges, count) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_parser_included_ranges(
    parser: *const TSParser,
    count: *mut u32,
) -> *const TSRange {
    unsafe { ts_lexer_included_ranges(&(*(parser as *const Parser)).lexer, count) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_parser_reset(parser: *mut TSParser) {
    unsafe {
        let this = parser as *mut Parser;
        parser_external_scanner_destroy(this);
        if !(*this).wasm_store.is_null() {
            ts_wasm_store_reset((*this).wasm_store);
        }

        if !is_null((*this).old_tree) {
            ts_subtree_release(&mut (*this).tree_pool, (*this).old_tree);
            (*this).old_tree = NULL_SUBTREE;
        }

        reusable_node_clear(&mut (*this).reusable_node);
        ts_lexer_reset(&mut (*this).lexer, length_zero());
        ts_stack_clear((*this).stack);
        parser_set_cached_token(this, 0, NULL_SUBTREE, NULL_SUBTREE);
        if !is_null((*this).finished_tree) {
            ts_subtree_release(&mut (*this).tree_pool, (*this).finished_tree);
            (*this).finished_tree = NULL_SUBTREE;
        }
        (*this).accept_count = 0;
        (*this).has_scanner_error = false;
        (*this).has_error = false;
        (*this).canceled_balancing = false;
        (*this).parse_options = TSParseOptions {
            payload: ptr::null_mut(),
            progress_callback: None,
        };
        (*this).parse_state = TSParseState {
            payload: ptr::null_mut(),
            current_byte_offset: 0,
            has_error: false,
        };
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_parser_parse(
    parser: *mut TSParser,
    old_tree: *const TSTree,
    input: TSInput,
) -> *mut TSTree {
    unsafe {
        let this = parser as *mut Parser;
        let mut result: *mut TSTree = ptr::null_mut();
        if (*this).language.is_null()
            || input.read.is_none()
            || (!old_tree.is_null() && (*old_tree).language != (*this).language)
        {
            return ptr::null_mut();
        }

        if ts_language_is_wasm((*this).language) {
            if (*this).wasm_store.is_null() {
                return ptr::null_mut();
            }
            ts_wasm_store_start(
                (*this).wasm_store,
                &mut (*this).lexer.data,
                (*this).language,
            );
        }

        ts_lexer_set_input(&mut (*this).lexer, input);
        (*this).included_range_differences.clear();
        (*this).included_range_difference_index = 0;

        (*this).operation_count = 0;

        'exit: {
            let mut skip_to_balance = false;
            if parser_has_outstanding_parse(this) {
                log!(this, c"resume_parsing");
                if (*this).canceled_balancing {
                    skip_to_balance = true;
                }
            } else {
                parser_external_scanner_create(this);
                if (*this).has_scanner_error {
                    break 'exit;
                }

                if !old_tree.is_null() {
                    ts_subtree_retain((*old_tree).root);
                    (*this).old_tree = (*old_tree).root;
                    ts_range_array_get_changed_ranges(
                        (*old_tree).included_ranges,
                        (*old_tree).included_range_count,
                        (*this).lexer.included_ranges,
                        (*this).lexer.included_range_count,
                        &mut (*this).included_range_differences,
                    );
                    reusable_node_reset(&mut (*this).reusable_node, (*old_tree).root);
                    log!(this, c"parse_after_edit");
                    log_tree!(this, (*this).old_tree);
                    for i in 0..(*this).included_range_differences.size {
                        let range = (*this).included_range_differences.get(i);
                        log!(
                            this,
                            c"different_included_range %u - %u",
                            (*range).start_byte,
                            (*range).end_byte
                        );
                    }
                } else {
                    reusable_node_clear(&mut (*this).reusable_node);
                    log!(this, c"new_parse");
                }
            }

            if !skip_to_balance {
                let mut position = 0u32;
                let mut last_position = 0u32;
                let mut version_count;
                loop {
                    let mut version: StackVersion = 0;
                    loop {
                        version_count = ts_stack_version_count((*this).stack);
                        if version >= version_count {
                            break;
                        }
                        let allow_node_reuse = version_count == 1;
                        while ts_stack_is_active((*this).stack, version) {
                            log!(
                                this,
                                c"process version:%u, version_count:%u, state:%d, row:%u, col:%u",
                                version,
                                ts_stack_version_count((*this).stack),
                                c_int::from(ts_stack_state((*this).stack, version)),
                                ts_stack_position((*this).stack, version).extent.row,
                                ts_stack_position((*this).stack, version).extent.column
                            );

                            if !parser_advance(this, version, allow_node_reuse) {
                                if (*this).has_scanner_error {
                                    break 'exit;
                                }
                                return ptr::null_mut();
                            }

                            log_stack!(this);

                            position = ts_stack_position((*this).stack, version).bytes;
                            if position > last_position
                                || (version > 0 && position == last_position)
                            {
                                last_position = position;
                                break;
                            }
                        }
                        version += 1;
                    }

                    // After advancing each version of the stack, re-sort the versions by their cost,
                    // removing any versions that are no longer worth pursuing.
                    let min_error_cost = parser_condense_stack(this);

                    // If there's already a finished parse tree that's better than any in-progress version,
                    // then terminate parsing. Clear the parse stack to remove any extra references to subtrees
                    // within the finished tree, ensuring that these subtrees can be safely mutated in-place
                    // for rebalancing.
                    if !is_null((*this).finished_tree)
                        && ts_subtree_error_cost((*this).finished_tree) < min_error_cost
                    {
                        ts_stack_clear((*this).stack);
                        break;
                    }

                    while (*this).included_range_difference_index
                        < (*this).included_range_differences.size
                    {
                        let range = (*this)
                            .included_range_differences
                            .get((*this).included_range_difference_index);
                        if (*range).end_byte <= position {
                            (*this).included_range_difference_index += 1;
                        } else {
                            break;
                        }
                    }

                    if version_count == 0 {
                        break;
                    }
                }
            }

            // balance:
            ts_assert!(!is_null((*this).finished_tree));
            if !parser_balance_subtree(this) {
                (*this).canceled_balancing = true;
                return ptr::null_mut();
            }
            (*this).canceled_balancing = false;
            log!(this, c"done");
            log_tree!(this, (*this).finished_tree);

            result = ts_tree_new(
                (*this).finished_tree,
                (*this).language,
                (*this).lexer.included_ranges,
                (*this).lexer.included_range_count,
            );
            (*this).finished_tree = NULL_SUBTREE;
        }

        // exit:
        ts_parser_reset(parser);
        result
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_parser_parse_with_options(
    parser: *mut TSParser,
    old_tree: *const TSTree,
    input: TSInput,
    parse_options: TSParseOptions,
) -> *mut TSTree {
    unsafe {
        let this = parser as *mut Parser;
        (*this).parse_options = parse_options;
        (*this).parse_state.payload = parse_options.payload;
        let result = ts_parser_parse(parser, old_tree, input);
        // Reset parser options before further parse calls.
        (*this).parse_options = TSParseOptions {
            payload: ptr::null_mut(),
            progress_callback: None,
        };
        result
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_parser_parse_string(
    parser: *mut TSParser,
    old_tree: *const TSTree,
    string: *const c_char,
    length: u32,
) -> *mut TSTree {
    unsafe {
        ts_parser_parse_string_encoding(parser, old_tree, string, length, TSInputEncodingUTF8)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_parser_parse_string_encoding(
    parser: *mut TSParser,
    old_tree: *const TSTree,
    string: *const c_char,
    length: u32,
    encoding: TSInputEncoding,
) -> *mut TSTree {
    unsafe {
        let mut input = TSStringInput { string, length };
        ts_parser_parse(
            parser,
            old_tree,
            TSInput {
                payload: &mut input as *mut TSStringInput as *mut c_void,
                read: Some(ts_string_input_read),
                encoding,
                decode: None,
            },
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_parser_set_wasm_store(parser: *mut TSParser, store: *mut c_void) {
    unsafe {
        let this = parser as *mut Parser;
        if !(*this).language.is_null() && ts_language_is_wasm((*this).language) {
            // Copy the assigned language into the new store.
            let copy = ts_language_copy((*this).language);
            ts_parser_set_language(parser, copy);
            ts_language_delete(copy);
        }

        ts_wasm_store_delete((*this).wasm_store);
        (*this).wasm_store = store;
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_parser_take_wasm_store(parser: *mut TSParser) -> *mut c_void {
    unsafe {
        let this = parser as *mut Parser;
        if !(*this).language.is_null() && ts_language_is_wasm((*this).language) {
            ts_parser_set_language(parser, ptr::null());
        }

        let result = (*this).wasm_store;
        (*this).wasm_store = ptr::null_mut();
        result
    }
}
