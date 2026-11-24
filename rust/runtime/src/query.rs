//! Port of `src/query.c`, part 1: data structures and their helpers (C lines 1-1310).
//!
//! The query is split over four Rust modules that all work on the types defined here:
//! - `query` (this file): structs, constants, streams, capture list pool, finished-state heap,
//!   quantifier algebra, symbol table, step helpers, analysis state helpers, pattern map.
//! - `query_analysis`: `ts_query__perform_analysis` and `ts_query__analyze_patterns`.
//! - `query_parse`: pattern/predicate parsing, `ts_query_new`/`delete`/`copy` and the small
//!   accessors up to `ts_query_disable_pattern`.
//! - `query_cursor`: `TSQueryCursor` and everything that executes a query.
//!
//! None of these structs are visible to other C units, so unlike the lower units they use Rust
//! layout. C bitfields become plain fields: where the C assigns a wider value to a narrower
//! bitfield (`consumed_capture_count: 12`, `AnalysisStateEntry.field_id: 15`,
//! `AnalysisSubgraphNode.child_index: 7`), the Rust code must mask on assignment so that
//! truncation is identical (see the `*_MASK` constants).
#![allow(clippy::missing_safety_doc, non_upper_case_globals)]
use crate::alloc::{ts_calloc, ts_free, ts_malloc};
use crate::api::*;
use crate::array::Array;
use crate::ts_assert;
use core::ffi::{c_char, c_int};
use core::ptr;

pub const MAX_STEP_CAPTURE_COUNT: usize = 3;
pub const MAX_NEGATED_FIELD_COUNT: usize = 8;
pub const MAX_STATE_PREDECESSOR_COUNT: usize = 256;
pub const MAX_ANALYSIS_STATE_DEPTH: usize = 8;
pub const MAX_ANALYSIS_ITERATION_COUNT: u32 = 256;

pub const CONSUMED_CAPTURE_COUNT_MASK: u16 = 0x0fff;
pub const ANALYSIS_FIELD_ID_MASK: u16 = 0x7fff;
pub const ANALYSIS_SUBGRAPH_CHILD_INDEX_MASK: u8 = 0x7f;

/// `PARENT_DONE` (a `TSQueryError`, which is unsigned in the bindings).
pub const PARENT_DONE: TSQueryError = TSQueryError::MAX;
pub const PATTERN_DONE_MARKER: u16 = u16::MAX;
pub const NONE: u16 = u16::MAX;
pub const CAPTURE_LIST_NONE: u32 = u32::MAX;
pub const WILDCARD_SYMBOL: TSSymbol = 0;
pub const WORK_PER_QUERY_CALLBACK_CHECK: u32 = 1000;

unsafe extern "C" {
    fn iswspace(c: i32) -> c_int;
    fn iswalnum(c: i32) -> c_int;
    fn strncmp(a: *const c_char, b: *const c_char, n: usize) -> c_int;
}

/// `Stream`: a sequence of unicode characters derived from a UTF-8 string.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Stream {
    pub input: *const c_char,
    pub start: *const c_char,
    pub end: *const c_char,
    pub next: i32,
    pub next_size: u8,
}

/// `QueryStep`. See the long comment in `query.c` for the meaning of every field.
#[derive(Clone, Copy)]
pub struct QueryStep {
    pub symbol: TSSymbol,
    pub supertype_symbol: TSSymbol,
    pub field: TSFieldId,
    pub capture_ids: [u16; MAX_STEP_CAPTURE_COUNT],
    pub depth: u16,
    pub alternative_index: u16,
    pub negated_field_list_id: u16,
    pub is_named: bool,
    pub is_immediate: bool,
    pub is_last_child: bool,
    pub is_pass_through: bool,
    pub is_dead_end: bool,
    pub is_inside_alternation: bool,
    pub contains_captures: bool,
    pub root_pattern_guaranteed: bool,
    pub parent_pattern_guaranteed: bool,
    pub is_missing: bool,
    pub alternative_is_skip: bool,
}

/// `Slice`: a slice of an external array.
#[derive(Clone, Copy)]
pub struct Slice {
    pub offset: u32,
    pub length: u32,
}

/// `SymbolTable`: a two-way mapping of strings to ids.
#[derive(Clone, Copy)]
pub struct SymbolTable {
    pub characters: Array<c_char>,
    pub slices: Array<Slice>,
}

/// `CaptureQuantifiers`: the quantifiers of pattern captures (one `TSQuantifier` byte per id).
pub type CaptureQuantifiers = Array<u8>;

#[derive(Clone, Copy)]
pub struct PatternEntry {
    pub step_index: u16,
    pub pattern_index: u16,
    pub is_rooted: bool,
}

#[derive(Clone, Copy)]
pub struct QueryPattern {
    pub steps: Slice,
    pub predicate_steps: Slice,
    pub start_byte: u32,
    pub end_byte: u32,
    pub is_non_local: bool,
}

#[derive(Clone, Copy)]
pub struct StepOffset {
    pub byte_offset: u32,
    pub step_index: u16,
}

/// `QueryState`. `consumed_capture_count` is a 12-bit bitfield in the C: mask on assignment.
#[derive(Clone, Copy)]
pub struct QueryState {
    pub id: u32,
    pub capture_list_id: u32,
    pub heap_insert_order: u32,
    pub start_depth: u16,
    pub step_index: u16,
    pub pattern_index: u16,
    pub consumed_capture_count: u16,
    pub seeking_immediate_match: bool,
    pub has_in_progress_alternatives: bool,
    pub dead: bool,
    pub needs_parent: bool,
    pub skipped_quantifier: bool,
}

pub type QueryStateList = Array<QueryState>;
pub type CaptureList = Array<TSQueryCapture>;

/// `CaptureListPool`.
#[derive(Clone, Copy)]
pub struct CaptureListPool {
    pub list: Array<CaptureList>,
    pub empty_list: CaptureList,
    pub max_capture_list_count: u32,
    pub free_id_stack: Array<u32>,
}

/// `AnalysisStateEntry`. `field_id` is a 15-bit bitfield in the C: mask on assignment.
#[derive(Clone, Copy)]
pub struct AnalysisStateEntry {
    pub parse_state: TSStateId,
    pub parent_symbol: TSSymbol,
    pub child_index: u16,
    pub field_id: u16,
    pub done: bool,
}

#[derive(Clone, Copy)]
pub struct AnalysisState {
    pub stack: [AnalysisStateEntry; MAX_ANALYSIS_STATE_DEPTH],
    pub depth: u16,
    pub step_index: u16,
    pub root_symbol: TSSymbol,
}

pub type AnalysisStateSet = Array<*mut AnalysisState>;

#[derive(Clone, Copy)]
pub struct QueryAnalysis {
    pub states: AnalysisStateSet,
    pub next_states: AnalysisStateSet,
    pub deeper_states: AnalysisStateSet,
    pub state_pool: AnalysisStateSet,
    pub final_step_indices: Array<u16>,
    pub finished_parent_symbols: Array<TSSymbol>,
    pub did_abort: bool,
}

/// `AnalysisSubgraphNode`. `child_index` is a 7-bit bitfield in the C: mask on assignment.
#[derive(Clone, Copy)]
pub struct AnalysisSubgraphNode {
    pub state: TSStateId,
    pub production_id: u16,
    pub child_index: u8,
    pub done: bool,
}

#[derive(Clone, Copy)]
pub struct AnalysisSubgraph {
    pub symbol: TSSymbol,
    pub start_states: Array<TSStateId>,
    pub nodes: Array<AnalysisSubgraphNode>,
}

pub type AnalysisSubgraphArray = Array<AnalysisSubgraph>;

/// `StatePredecessorMap`.
#[derive(Clone, Copy)]
pub struct StatePredecessorMap {
    pub contents: *mut TSStateId,
}

/// `struct TSQuery`: a tree query, compiled from a string of S-expressions.
pub struct TSQuery {
    pub captures: SymbolTable,
    pub predicate_values: SymbolTable,
    pub capture_quantifiers: Array<CaptureQuantifiers>,
    pub steps: Array<QueryStep>,
    pub pattern_map: Array<PatternEntry>,
    pub predicate_steps: Array<TSQueryPredicateStep>,
    pub patterns: Array<QueryPattern>,
    pub step_offsets: Array<StepOffset>,
    pub negated_fields: Array<TSFieldId>,
    pub string_buffer: Array<c_char>,
    pub repeat_symbols_with_rootless_patterns: Array<TSSymbol>,
    pub language: *const TSLanguage,
    pub wildcard_root_pattern_count: u16,
}

/// `struct TSQueryCursor`: a stateful struct used to execute a query on a tree.
pub struct TSQueryCursor {
    pub query: *const TSQuery,
    pub cursor: TSTreeCursor,
    pub states: QueryStateList,
    pub finished_states: QueryStateList,
    /// How much of `finished_states` is in heap order (only used by `next_capture`).
    pub finished_states_heap_size: u32,
    pub capture_list_pool: CaptureListPool,
    pub depth: u32,
    pub max_start_depth: u32,
    pub included_range: TSRange,
    pub containing_range: TSRange,
    pub next_state_id: u32,
    pub next_finished_state_id: u32,
    pub query_options: *const TSQueryCursorOptions,
    pub query_state: TSQueryCursorState,
    pub work_count: u32,
    pub on_visible_node: bool,
    pub ascending: bool,
    pub halted: bool,
    pub did_exceed_match_limit: bool,
}

// ---- Stream ----

/// Advance to the next unicode code point in the stream.
pub unsafe fn stream_advance(this: *mut Stream) -> bool {
    unsafe {
        (*this).input = (*this).input.add((*this).next_size as usize);
        if (*this).input < (*this).end {
            let len = (*this).end.offset_from((*this).input) as usize;
            let slice = core::slice::from_raw_parts((*this).input as *const u8, len);
            let (size, cp) = crate::unicode::decode_utf8(slice);
            (*this).next = cp;
            if size > 0 {
                (*this).next_size = size as u8;
                return true;
            }
        } else {
            (*this).next_size = 0;
            (*this).next = 0;
        }
        false
    }
}

/// Reset the stream to the given input position, represented as a pointer into the input string.
pub unsafe fn stream_reset(this: *mut Stream, input: *const c_char) {
    unsafe {
        (*this).input = input;
        (*this).next_size = 0;
        stream_advance(this);
    }
}

pub unsafe fn stream_new(string: *const c_char, length: u32) -> Stream {
    unsafe {
        let mut this = Stream {
            next: 0,
            input: string,
            start: string,
            end: string.add(length as usize),
            next_size: 0,
        };
        stream_advance(&mut this);
        this
    }
}

pub unsafe fn stream_skip_whitespace(this: *mut Stream) {
    unsafe {
        loop {
            if iswspace((*this).next) != 0 {
                stream_advance(this);
            } else if (*this).next == i32::from(b';') {
                // skip over comments
                stream_advance(this);
                while (*this).next != 0 && (*this).next != i32::from(b'\n') {
                    if !stream_advance(this) {
                        break;
                    }
                }
            } else {
                break;
            }
        }
    }
}

pub unsafe fn stream_is_ident_start(this: *const Stream) -> bool {
    unsafe {
        iswalnum((*this).next) != 0
            || (*this).next == i32::from(b'_')
            || (*this).next == i32::from(b'-')
    }
}

pub unsafe fn stream_scan_identifier(stream: *mut Stream) {
    unsafe {
        loop {
            stream_advance(stream);
            if !(iswalnum((*stream).next) != 0
                || (*stream).next == i32::from(b'_')
                || (*stream).next == i32::from(b'-')
                || (*stream).next == i32::from(b'.'))
            {
                break;
            }
        }
    }
}

pub unsafe fn stream_offset(this: *const Stream) -> u32 {
    unsafe { (*this).input.offset_from((*this).start) as u32 }
}

// ---- CaptureListPool ----

pub fn capture_list_pool_new() -> CaptureListPool {
    CaptureListPool {
        list: Array::new(),
        empty_list: Array::new(),
        max_capture_list_count: u32::MAX,
        free_id_stack: Array::new(),
    }
}

pub unsafe fn capture_list_pool_reset(this: *mut CaptureListPool) {
    unsafe {
        (*this).free_id_stack.clear();
        for i in 0..(*this).list.size {
            // This invalid size means that the list is not in use.
            (*(*this).list.get(i)).size = u32::MAX;
            (*this).free_id_stack.push(i);
        }
    }
}

pub unsafe fn capture_list_pool_delete(this: *mut CaptureListPool) {
    unsafe {
        for i in 0..(*this).list.size {
            (*(*this).list.get(i)).delete();
        }
        (*this).list.delete();
        (*this).free_id_stack.delete();
    }
}

pub unsafe fn capture_list_pool_get(this: *const CaptureListPool, id: u32) -> *const CaptureList {
    unsafe {
        if id >= (*this).list.size {
            return &(*this).empty_list;
        }
        (*this).list.get(id)
    }
}

pub unsafe fn capture_list_pool_get_mut(this: *mut CaptureListPool, id: u32) -> *mut CaptureList {
    unsafe {
        ts_assert!(id < (*this).list.size);
        (*this).list.get(id)
    }
}

pub unsafe fn capture_list_pool_is_empty(this: *const CaptureListPool) -> bool {
    // The capture list pool is empty if all allocated lists are in use, and we
    // have reached the maximum allowed number of allocated lists.
    unsafe {
        (*this).free_id_stack.size == 0 && (*this).list.size >= (*this).max_capture_list_count
    }
}

pub unsafe fn capture_list_pool_acquire(this: *mut CaptureListPool) -> u32 {
    unsafe {
        // First see if any already allocated capture list is currently unused.
        if (*this).free_id_stack.size > 0 {
            let id = (*this).free_id_stack.pop();
            (*(*this).list.get(id)).clear();
            return id;
        }

        // Otherwise allocate and initialize a new capture list, as long as that
        // doesn't put us over the requested maximum.
        let i = (*this).list.size;
        if i >= (*this).max_capture_list_count {
            return CAPTURE_LIST_NONE;
        }
        let list: CaptureList = Array::new();
        (*this).list.push(list);
        i
    }
}

pub unsafe fn capture_list_pool_release(this: *mut CaptureListPool, id: u32) {
    unsafe {
        if id >= (*this).list.size {
            return;
        }
        let list = (*this).list.get(id);
        if (*list).size == u32::MAX {
            return; // Guard against releasing a list twice
        }
        (*list).size = u32::MAX;
        (*this).free_id_stack.push(id);
    }
}

// ---- FinishedStateHeap ----
//
// A min-heap of finished query states, ordered by (byte offset of next unconsumed capture,
// pattern_index, insertion order). Maintained lazily: `ts_query_cursor__advance` pushes plainly,
// and `next_capture` sifts new elements into place via the tracked heap size.

unsafe fn finished_state_swap(states: *mut QueryStateList, a: u32, b: u32) {
    unsafe {
        let tmp = *(*states).get(a);
        *(*states).get(a) = *(*states).get(b);
        *(*states).get(b) = tmp;
    }
}

/// Compare two finished states by (byte offset of next unconsumed capture, pattern_index,
/// insertion order).
pub unsafe fn finished_state_precedes(
    a: *const QueryState,
    b: *const QueryState,
    pool: *const CaptureListPool,
) -> bool {
    unsafe {
        let a_caps = capture_list_pool_get(pool, (*a).capture_list_id);
        let b_caps = capture_list_pool_get(pool, (*b).capture_list_id);
        if u32::from((*a).consumed_capture_count) >= (*a_caps).size {
            return false;
        }
        if u32::from((*b).consumed_capture_count) >= (*b_caps).size {
            return true;
        }
        let a_byte = crate::node::ts_node_start_byte(
            (*(*a_caps)
                .contents
                .add(usize::from((*a).consumed_capture_count)))
            .node,
        );
        let b_byte = crate::node::ts_node_start_byte(
            (*(*b_caps)
                .contents
                .add(usize::from((*b).consumed_capture_count)))
            .node,
        );
        if a_byte != b_byte {
            return a_byte < b_byte;
        }
        if (*a).pattern_index != (*b).pattern_index {
            return (*a).pattern_index < (*b).pattern_index;
        }
        (*a).heap_insert_order < (*b).heap_insert_order
    }
}

pub unsafe fn finished_state_sift_down(
    states: *mut QueryStateList,
    mut index: u32,
    pool: *const CaptureListPool,
) {
    unsafe {
        let size = (*states).size;
        loop {
            let mut smallest = index;
            let left = 2 * index + 1;
            let right = 2 * index + 2;
            if left < size
                && finished_state_precedes((*states).get(left), (*states).get(smallest), pool)
            {
                smallest = left;
            }
            if right < size
                && finished_state_precedes((*states).get(right), (*states).get(smallest), pool)
            {
                smallest = right;
            }
            if smallest == index {
                break;
            }
            finished_state_swap(states, index, smallest);
            index = smallest;
        }
    }
}

pub unsafe fn finished_state_sift_up(
    states: *mut QueryStateList,
    mut index: u32,
    pool: *const CaptureListPool,
) {
    unsafe {
        while index > 0 {
            let parent = (index - 1) / 2;
            if finished_state_precedes((*states).get(index), (*states).get(parent), pool) {
                finished_state_swap(states, index, parent);
                index = parent;
            } else {
                break;
            }
        }
    }
}

pub unsafe fn finished_state_pop(states: *mut QueryStateList, pool: *const CaptureListPool) {
    unsafe {
        if (*states).size > 1 {
            *(*states).front() = *(*states).back();
        }
        (*states).size -= 1;
        if (*states).size > 0 {
            finished_state_sift_down(states, 0, pool);
        }
    }
}

/// Remove an element at an arbitrary index and restore heap order.
pub unsafe fn finished_state_erase(
    states: *mut QueryStateList,
    index: u32,
    pool: *const CaptureListPool,
) {
    unsafe {
        if index == (*states).size.wrapping_sub(1) {
            (*states).size -= 1;
            return;
        }
        *(*states).get(index) = *(*states).back();
        (*states).size -= 1;
        // The replacement element may need to go up or down.
        if index > 0
            && finished_state_precedes((*states).get(index), (*states).get((index - 1) / 2), pool)
        {
            finished_state_sift_up(states, index, pool);
        } else {
            finished_state_sift_down(states, index, pool);
        }
    }
}

pub unsafe fn query_cursor_push_finished_state(this: *mut TSQueryCursor, state: *mut QueryState) {
    unsafe {
        (*state).heap_insert_order = (*this).next_finished_state_id;
        (*this).next_finished_state_id = (*this).next_finished_state_id.wrapping_add(1);
        (*this).finished_states.push(*state);
    }
}

pub unsafe fn query_cursor_heapify_finished_states(this: *mut TSQueryCursor) {
    unsafe {
        while (*this).finished_states_heap_size < (*this).finished_states.size {
            finished_state_sift_up(
                &mut (*this).finished_states,
                (*this).finished_states_heap_size,
                &(*this).capture_list_pool,
            );
            (*this).finished_states_heap_size += 1;
        }
    }
}

// ---- Quantifiers ----

pub fn quantifier_mul(left: TSQuantifier, right: TSQuantifier) -> TSQuantifier {
    match left {
        TSQuantifierZero => TSQuantifierZero,
        TSQuantifierZeroOrOne => match right {
            TSQuantifierZero => TSQuantifierZero,
            TSQuantifierZeroOrOne | TSQuantifierOne => TSQuantifierZeroOrOne,
            TSQuantifierZeroOrMore | TSQuantifierOneOrMore => TSQuantifierZeroOrMore,
            _ => TSQuantifierZero,
        },
        TSQuantifierZeroOrMore => match right {
            TSQuantifierZero => TSQuantifierZero,
            TSQuantifierZeroOrOne
            | TSQuantifierZeroOrMore
            | TSQuantifierOne
            | TSQuantifierOneOrMore => TSQuantifierZeroOrMore,
            _ => TSQuantifierZero,
        },
        TSQuantifierOne => right,
        TSQuantifierOneOrMore => match right {
            TSQuantifierZero => TSQuantifierZero,
            TSQuantifierZeroOrOne | TSQuantifierZeroOrMore => TSQuantifierZeroOrMore,
            TSQuantifierOne | TSQuantifierOneOrMore => TSQuantifierOneOrMore,
            _ => TSQuantifierZero,
        },
        _ => TSQuantifierZero,
    }
}

pub fn quantifier_join(left: TSQuantifier, right: TSQuantifier) -> TSQuantifier {
    match left {
        TSQuantifierZero => match right {
            TSQuantifierZero => TSQuantifierZero,
            TSQuantifierZeroOrOne | TSQuantifierOne => TSQuantifierZeroOrOne,
            TSQuantifierZeroOrMore | TSQuantifierOneOrMore => TSQuantifierZeroOrMore,
            _ => TSQuantifierZero,
        },
        TSQuantifierZeroOrOne => match right {
            TSQuantifierZero | TSQuantifierZeroOrOne | TSQuantifierOne => TSQuantifierZeroOrOne,
            TSQuantifierZeroOrMore | TSQuantifierOneOrMore => TSQuantifierZeroOrMore,
            _ => TSQuantifierZero,
        },
        TSQuantifierZeroOrMore => TSQuantifierZeroOrMore,
        TSQuantifierOne => match right {
            TSQuantifierZero | TSQuantifierZeroOrOne => TSQuantifierZeroOrOne,
            TSQuantifierZeroOrMore => TSQuantifierZeroOrMore,
            TSQuantifierOne => TSQuantifierOne,
            TSQuantifierOneOrMore => TSQuantifierOneOrMore,
            _ => TSQuantifierZero,
        },
        TSQuantifierOneOrMore => match right {
            TSQuantifierZero | TSQuantifierZeroOrOne | TSQuantifierZeroOrMore => {
                TSQuantifierZeroOrMore
            }
            TSQuantifierOne | TSQuantifierOneOrMore => TSQuantifierOneOrMore,
            _ => TSQuantifierZero,
        },
        _ => TSQuantifierZero,
    }
}

pub fn quantifier_add(left: TSQuantifier, right: TSQuantifier) -> TSQuantifier {
    match left {
        TSQuantifierZero => right,
        TSQuantifierZeroOrOne => match right {
            TSQuantifierZero => TSQuantifierZeroOrOne,
            TSQuantifierZeroOrOne | TSQuantifierZeroOrMore => TSQuantifierZeroOrMore,
            TSQuantifierOne | TSQuantifierOneOrMore => TSQuantifierOneOrMore,
            _ => TSQuantifierZero,
        },
        TSQuantifierZeroOrMore => match right {
            TSQuantifierZero | TSQuantifierZeroOrOne | TSQuantifierZeroOrMore => {
                TSQuantifierZeroOrMore
            }
            TSQuantifierOne | TSQuantifierOneOrMore => TSQuantifierOneOrMore,
            _ => TSQuantifierZero,
        },
        TSQuantifierOne => match right {
            TSQuantifierZero => TSQuantifierOne,
            TSQuantifierZeroOrOne
            | TSQuantifierZeroOrMore
            | TSQuantifierOne
            | TSQuantifierOneOrMore => TSQuantifierOneOrMore,
            _ => TSQuantifierZero,
        },
        TSQuantifierOneOrMore => TSQuantifierOneOrMore,
        _ => TSQuantifierZero,
    }
}

pub fn capture_quantifiers_new() -> CaptureQuantifiers {
    Array::new()
}

pub unsafe fn capture_quantifiers_delete(this: *mut CaptureQuantifiers) {
    unsafe { (*this).delete() }
}

pub unsafe fn capture_quantifiers_clear(this: *mut CaptureQuantifiers) {
    unsafe { (*this).clear() }
}

/// Replace capture quantifiers with the given quantifiers.
pub unsafe fn capture_quantifiers_replace(
    this: *mut CaptureQuantifiers,
    quantifiers: *mut CaptureQuantifiers,
) {
    unsafe {
        (*this).clear();
        (*this).push_all(&*quantifiers);
    }
}

/// Return capture quantifier for the given capture id.
pub unsafe fn capture_quantifier_for_id(this: *const CaptureQuantifiers, id: u16) -> TSQuantifier {
    unsafe {
        if (*this).size <= u32::from(id) {
            TSQuantifierZero
        } else {
            TSQuantifier::from(*(*this).get(u32::from(id)))
        }
    }
}

/// Add the given quantifier to the current value for id.
pub unsafe fn capture_quantifiers_add_for_id(
    this: *mut CaptureQuantifiers,
    id: u16,
    quantifier: TSQuantifier,
) {
    unsafe {
        if (*this).size <= u32::from(id) {
            (*this).grow_by(u32::from(id) + 1 - (*this).size);
        }
        let own = (*this).get(u32::from(id));
        *own = quantifier_add(TSQuantifier::from(*own), quantifier) as u8;
    }
}

/// Point-wise add the given quantifiers to the current values.
pub unsafe fn capture_quantifiers_add_all(
    this: *mut CaptureQuantifiers,
    quantifiers: *mut CaptureQuantifiers,
) {
    unsafe {
        if (*this).size < (*quantifiers).size {
            (*this).grow_by((*quantifiers).size - (*this).size);
        }
        // `uint16_t id` against `(uint16_t)quantifiers->size`
        let n = (*quantifiers).size as u16;
        let mut id = 0u16;
        while id < n {
            let quantifier = (*quantifiers).get(u32::from(id));
            let own = (*this).get(u32::from(id));
            *own = quantifier_add(TSQuantifier::from(*own), TSQuantifier::from(*quantifier)) as u8;
            id = id.wrapping_add(1);
        }
    }
}

/// Join the given quantifier with the current values.
pub unsafe fn capture_quantifiers_mul(this: *mut CaptureQuantifiers, quantifier: TSQuantifier) {
    unsafe {
        let n = (*this).size as u16;
        let mut id = 0u16;
        while id < n {
            let own = (*this).get(u32::from(id));
            *own = quantifier_mul(TSQuantifier::from(*own), quantifier) as u8;
            id = id.wrapping_add(1);
        }
    }
}

/// Point-wise join the quantifiers from a list of alternatives with the current values.
pub unsafe fn capture_quantifiers_join_all(
    this: *mut CaptureQuantifiers,
    quantifiers: *mut CaptureQuantifiers,
) {
    unsafe {
        if (*this).size < (*quantifiers).size {
            (*this).grow_by((*quantifiers).size - (*this).size);
        }
        for id in 0..(*quantifiers).size {
            let quantifier = (*quantifiers).get(id);
            let own = (*this).get(id);
            *own = quantifier_join(TSQuantifier::from(*own), TSQuantifier::from(*quantifier)) as u8;
        }
        for id in (*quantifiers).size..(*this).size {
            let own = (*this).get(id);
            *own = quantifier_join(TSQuantifier::from(*own), TSQuantifierZero) as u8;
        }
    }
}

// ---- SymbolTable ----

pub fn symbol_table_new() -> SymbolTable {
    SymbolTable {
        characters: Array::new(),
        slices: Array::new(),
    }
}

pub unsafe fn symbol_table_delete(this: *mut SymbolTable) {
    unsafe {
        (*this).characters.delete();
        (*this).slices.delete();
    }
}

pub unsafe fn symbol_table_id_for_name(
    this: *const SymbolTable,
    name: *const c_char,
    length: u32,
) -> i32 {
    unsafe {
        for i in 0..(*this).slices.size {
            let slice = *(*this).slices.get(i);
            if slice.length == length
                && strncmp((*this).characters.get(slice.offset), name, length as usize) == 0
            {
                return i as i32;
            }
        }
        -1
    }
}

pub unsafe fn symbol_table_name_for_id(
    this: *const SymbolTable,
    id: u16,
    length: *mut u32,
) -> *const c_char {
    unsafe {
        let slice = *(*this).slices.get(u32::from(id));
        *length = slice.length;
        (*this).characters.get(slice.offset)
    }
}

pub unsafe fn symbol_table_insert_name(
    this: *mut SymbolTable,
    name: *const c_char,
    length: u32,
) -> u16 {
    unsafe {
        let id = symbol_table_id_for_name(this, name, length);
        if id >= 0 {
            return id as u16;
        }
        let slice = Slice {
            offset: (*this).characters.size,
            length,
        };
        (*this).characters.grow_by(length + 1);
        ptr::copy_nonoverlapping(name, (*this).characters.get(slice.offset), length as usize);
        *(*this).characters.get((*this).characters.size - 1) = 0;
        (*this).slices.push(slice);
        ((*this).slices.size - 1) as u16
    }
}

// ---- QueryStep ----

pub fn query_step_new(symbol: TSSymbol, depth: u16, is_immediate: bool) -> QueryStep {
    QueryStep {
        symbol,
        supertype_symbol: 0,
        field: 0,
        capture_ids: [NONE; MAX_STEP_CAPTURE_COUNT],
        depth,
        alternative_index: NONE,
        negated_field_list_id: 0,
        is_named: false,
        is_immediate,
        is_last_child: false,
        is_pass_through: false,
        is_dead_end: false,
        is_inside_alternation: false,
        contains_captures: false,
        root_pattern_guaranteed: false,
        parent_pattern_guaranteed: false,
        is_missing: false,
        alternative_is_skip: false,
    }
}

pub fn query_step_add_capture(this: &mut QueryStep, capture_id: u16) {
    for i in 0..MAX_STEP_CAPTURE_COUNT {
        if this.capture_ids[i] == NONE {
            this.capture_ids[i] = capture_id;
            break;
        }
    }
}

pub fn query_step_remove_capture(this: &mut QueryStep, capture_id: u16) {
    let mut i = 0;
    while i < MAX_STEP_CAPTURE_COUNT {
        if this.capture_ids[i] == capture_id {
            this.capture_ids[i] = NONE;
            while i + 1 < MAX_STEP_CAPTURE_COUNT {
                if this.capture_ids[i + 1] == NONE {
                    break;
                }
                this.capture_ids[i] = this.capture_ids[i + 1];
                this.capture_ids[i + 1] = NONE;
                i += 1;
            }
            break;
        }
        i += 1;
    }
}

// ---- StatePredecessorMap ----

pub unsafe fn state_predecessor_map_new(language: *const TSLanguage) -> StatePredecessorMap {
    unsafe {
        StatePredecessorMap {
            contents: ts_calloc(
                (*language).state_count as usize * (MAX_STATE_PREDECESSOR_COUNT + 1),
                size_of::<TSStateId>(),
            ) as *mut TSStateId,
        }
    }
}

pub unsafe fn state_predecessor_map_delete(this: *mut StatePredecessorMap) {
    unsafe { ts_free((*this).contents as *mut core::ffi::c_void) }
}

pub unsafe fn state_predecessor_map_add(
    this: *mut StatePredecessorMap,
    state: TSStateId,
    predecessor: TSStateId,
) {
    unsafe {
        let index = usize::from(state) * (MAX_STATE_PREDECESSOR_COUNT + 1);
        let count = (*this).contents.add(index);
        if *count == 0
            || (usize::from(*count) < MAX_STATE_PREDECESSOR_COUNT
                && *(*this).contents.add(index + usize::from(*count)) != predecessor)
        {
            *count = (*count).wrapping_add(1);
            *(*this).contents.add(index + usize::from(*count)) = predecessor;
        }
    }
}

pub unsafe fn state_predecessor_map_get(
    this: *const StatePredecessorMap,
    state: TSStateId,
    count: *mut u32,
) -> *const TSStateId {
    unsafe {
        let index = usize::from(state) * (MAX_STATE_PREDECESSOR_COUNT + 1);
        *count = u32::from(*(*this).contents.add(index));
        (*this).contents.add(index + 1)
    }
}

// ---- AnalysisState ----

pub unsafe fn analysis_state_recursion_depth(this: *const AnalysisState) -> u32 {
    unsafe {
        let mut result = 0;
        for i in 0..usize::from((*this).depth) {
            let symbol = (*this).stack[i].parent_symbol;
            for j in 0..i {
                if (*this).stack[j].parent_symbol == symbol {
                    result += 1;
                    break;
                }
            }
        }
        result
    }
}

pub unsafe fn analysis_state_compare(
    this: *const *mut AnalysisState,
    other: *const *mut AnalysisState,
) -> i32 {
    unsafe {
        let (a, b) = (&**this, &**other);
        if a.depth < b.depth {
            return 1;
        }
        for i in 0..usize::from(a.depth) {
            if i >= usize::from(b.depth) {
                return -1;
            }
            let s1 = a.stack[i];
            let s2 = b.stack[i];
            if s1.child_index < s2.child_index {
                return -1;
            }
            if s1.child_index > s2.child_index {
                return 1;
            }
            if s1.parent_symbol < s2.parent_symbol {
                return -1;
            }
            if s1.parent_symbol > s2.parent_symbol {
                return 1;
            }
            if s1.parse_state < s2.parse_state {
                return -1;
            }
            if s1.parse_state > s2.parse_state {
                return 1;
            }
            if s1.field_id < s2.field_id {
                return -1;
            }
            if s1.field_id > s2.field_id {
                return 1;
            }
        }
        if a.step_index < b.step_index {
            return -1;
        }
        if a.step_index > b.step_index {
            return 1;
        }
        0
    }
}

pub unsafe fn analysis_state_top(this: *mut AnalysisState) -> *mut AnalysisStateEntry {
    unsafe {
        if (*this).depth == 0 {
            return &mut (*this).stack[0];
        }
        &mut (*this).stack[usize::from((*this).depth) - 1]
    }
}

pub unsafe fn analysis_state_has_supertype(this: *mut AnalysisState, symbol: TSSymbol) -> bool {
    unsafe {
        for i in 0..usize::from((*this).depth) {
            if (*this).stack[i].parent_symbol == symbol {
                return true;
            }
        }
        false
    }
}

// ---- AnalysisStateSet ----

/// Obtains an `AnalysisState` instance, either by consuming one from this set's object pool, or by
/// cloning one from scratch.
pub unsafe fn analysis_state_pool_clone_or_reuse(
    this: *mut AnalysisStateSet,
    borrowed_item: *mut AnalysisState,
) -> *mut AnalysisState {
    unsafe {
        let new_item = if (*this).size > 0 {
            (*this).pop()
        } else {
            ts_malloc(size_of::<AnalysisState>()) as *mut AnalysisState
        };
        ptr::write(new_item, *borrowed_item);
        new_item
    }
}

/// Inserts a clone of the passed-in item at the appropriate position to maintain ordering in this
/// set. The set does not contain duplicates.
pub unsafe fn analysis_state_set_insert_sorted(
    this: *mut AnalysisStateSet,
    pool: *mut AnalysisStateSet,
    mut borrowed_item: *mut AnalysisState,
) {
    unsafe {
        let needle: *const *mut AnalysisState = &mut borrowed_item;
        let (index, exists) = (*this).search_sorted_with(
            |existing, n: *const *mut AnalysisState| analysis_state_compare(existing, n),
            needle,
        );
        if !exists {
            let new_item = analysis_state_pool_clone_or_reuse(pool, borrowed_item);
            (*this).insert(index, new_item);
        }
    }
}

/// Inserts a clone of the passed-in item at the end position of this list. The caller must ensure
/// that this item is larger than the largest item already in the set.
pub unsafe fn analysis_state_set_push(
    this: *mut AnalysisStateSet,
    pool: *mut AnalysisStateSet,
    borrowed_item: *mut AnalysisState,
) {
    unsafe {
        let new_item = analysis_state_pool_clone_or_reuse(pool, borrowed_item);
        (*this).push(new_item);
    }
}

/// Removes all items from this set, returning it to an empty state.
pub unsafe fn analysis_state_set_clear(this: *mut AnalysisStateSet, pool: *mut AnalysisStateSet) {
    unsafe {
        (*pool).push_all(&*this);
        (*this).clear();
    }
}

/// Releases all memory that is managed with this state set, including any items currently present.
pub unsafe fn analysis_state_set_delete(this: *mut AnalysisStateSet) {
    unsafe {
        for i in 0..(*this).size {
            ts_free(*(*this).get(i) as *mut core::ffi::c_void);
        }
        (*this).delete();
    }
}

// ---- QueryAnalyzer ----

pub fn query_analysis_new() -> QueryAnalysis {
    QueryAnalysis {
        states: Array::new(),
        next_states: Array::new(),
        deeper_states: Array::new(),
        state_pool: Array::new(),
        final_step_indices: Array::new(),
        finished_parent_symbols: Array::new(),
        did_abort: false,
    }
}

pub unsafe fn query_analysis_delete(this: *mut QueryAnalysis) {
    unsafe {
        analysis_state_set_delete(&mut (*this).states);
        analysis_state_set_delete(&mut (*this).next_states);
        analysis_state_set_delete(&mut (*this).deeper_states);
        analysis_state_set_delete(&mut (*this).state_pool);
        (*this).final_step_indices.delete();
        (*this).finished_parent_symbols.delete();
    }
}

// ---- AnalysisSubgraphNode ----

pub fn analysis_subgraph_node_compare(
    this: &AnalysisSubgraphNode,
    other: &AnalysisSubgraphNode,
) -> i32 {
    if this.state < other.state {
        return -1;
    }
    if this.state > other.state {
        return 1;
    }
    if this.child_index < other.child_index {
        return -1;
    }
    if this.child_index > other.child_index {
        return 1;
    }
    if !this.done && other.done {
        return -1;
    }
    if this.done && !other.done {
        return 1;
    }
    if this.production_id < other.production_id {
        return -1;
    }
    if this.production_id > other.production_id {
        return 1;
    }
    0
}

// ---- Query: pattern map ----

/// Looks up `needle` in the pattern map (sorted by root symbol, binary search). Returns whether the
/// symbol is present; `*result` is the index of the entry, or where it should be inserted.
pub unsafe fn query_pattern_map_search(
    this: *const TSQuery,
    needle: TSSymbol,
    result: *mut u32,
) -> bool {
    unsafe {
        let mut base_index = u32::from((*this).wildcard_root_pattern_count);
        let mut size = (*this).pattern_map.size.wrapping_sub(base_index);
        if size == 0 {
            *result = base_index;
            return false;
        }
        while size > 1 {
            let half_size = size / 2;
            let mid_index = base_index + half_size;
            let mid_symbol = (*(*this)
                .steps
                .get(u32::from((*(*this).pattern_map.get(mid_index)).step_index)))
            .symbol;
            if needle > mid_symbol {
                base_index = mid_index;
            }
            size -= half_size;
        }

        let mut symbol = (*(*this)
            .steps
            .get(u32::from((*(*this).pattern_map.get(base_index)).step_index)))
        .symbol;

        if needle > symbol {
            base_index += 1;
            if base_index < (*this).pattern_map.size {
                symbol = (*(*this)
                    .steps
                    .get(u32::from((*(*this).pattern_map.get(base_index)).step_index)))
                .symbol;
            }
        }

        *result = base_index;
        needle == symbol
    }
}

/// Insert a new pattern's start index into the pattern map, maintaining its ordering invariant.
pub unsafe fn query_pattern_map_insert(
    this: *mut TSQuery,
    symbol: TSSymbol,
    new_entry: PatternEntry,
) {
    unsafe {
        let mut index = 0u32;
        query_pattern_map_search(this, symbol, &mut index);

        // Ensure that the entries are sorted not only by symbol, but also
        // by pattern_index. This way, states for earlier patterns will be
        // initiated first, which allows the ordering of the states array
        // to be maintained more efficiently.
        while index < (*this).pattern_map.size {
            let entry = (*this).pattern_map.get(index);
            if (*(*this).steps.get(u32::from((*entry).step_index))).symbol == symbol
                && (*entry).pattern_index < new_entry.pattern_index
            {
                index += 1;
            } else {
                break;
            }
        }

        (*this).pattern_map.insert(index, new_entry);
    }
}
