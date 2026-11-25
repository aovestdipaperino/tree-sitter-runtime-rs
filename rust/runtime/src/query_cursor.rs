//! Part of the `query.c` port (see `query.rs`): `TSQueryCursor`, C lines 3426-4881.
//!
//! Function names follow the C (`ts_query_cursor__x` becomes `query_cursor_x` for the static
//! ones); control flow, including the `j--`/`continue` index games of the state loops, is kept
//! as is so that matches come out identical.
#![allow(clippy::missing_safety_doc, non_upper_case_globals)]
use crate::alloc::{ts_free, ts_malloc};
use crate::api::*;
use crate::array::Array;
use crate::node::*;
use crate::parser_abi::ts_builtin_sym_error;
use crate::point::*;
use crate::query::*;
use crate::query::{TSQuery, TSQueryCursor};
use crate::subtree::*;
use crate::tree_cursor::*;
use core::ptr;

const POINT_MAX: TSPoint = TSPoint {
    row: u32::MAX,
    column: u32::MAX,
};

use crate::query_parse::query_step_is_fallible;

// ---- QueryCursor ----

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_cursor_new() -> *mut TSQueryCursor {
    unsafe {
        let this = ts_malloc(size_of::<TSQueryCursor>()) as *mut TSQueryCursor;
        ptr::write(
            this,
            TSQueryCursor {
                query: ptr::null(),
                cursor: core::mem::zeroed(),
                states: Array::new(),
                finished_states: Array::new(),
                finished_states_heap_size: 0,
                capture_list_pool: capture_list_pool_new(),
                depth: 0,
                max_start_depth: u32::MAX,
                included_range: TSRange {
                    start_point: TSPoint { row: 0, column: 0 },
                    end_point: POINT_MAX,
                    start_byte: 0,
                    end_byte: u32::MAX,
                },
                containing_range: TSRange {
                    start_point: TSPoint { row: 0, column: 0 },
                    end_point: POINT_MAX,
                    start_byte: 0,
                    end_byte: u32::MAX,
                },
                next_state_id: 0,
                next_finished_state_id: 0,
                query_options: ptr::null(),
                query_state: core::mem::zeroed(),
                work_count: 0,
                on_visible_node: false,
                ascending: false,
                halted: false,
                did_exceed_match_limit: false,
            },
        );
        (*this).states.reserve(8);
        (*this).finished_states.reserve(8);
        this
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_cursor_delete(this: *mut TSQueryCursor) {
    unsafe {
        (*this).states.delete();
        (*this).finished_states.delete();
        ts_tree_cursor_delete(&mut (*this).cursor);
        capture_list_pool_delete(&mut (*this).capture_list_pool);
        ts_free(this as *mut core::ffi::c_void);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_cursor_did_exceed_match_limit(
    this: *const TSQueryCursor,
) -> bool {
    unsafe { (*this).did_exceed_match_limit }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_cursor_match_limit(this: *const TSQueryCursor) -> u32 {
    unsafe { (*this).capture_list_pool.max_capture_list_count }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_cursor_set_match_limit(this: *mut TSQueryCursor, limit: u32) {
    unsafe { (*this).capture_list_pool.max_capture_list_count = limit }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_cursor_exec(
    this: *mut TSQueryCursor,
    query: *const TSQuery,
    node: TSNode,
) {
    unsafe {
        (*this).states.clear();
        (*this).finished_states.clear();
        (*this).finished_states_heap_size = 0;
        ts_tree_cursor_reset(&mut (*this).cursor, node);
        capture_list_pool_reset(&mut (*this).capture_list_pool);
        (*this).on_visible_node = true;
        (*this).next_state_id = 0;
        (*this).next_finished_state_id = 0;
        (*this).depth = 0;
        (*this).ascending = false;
        (*this).halted = false;
        (*this).query = query;
        (*this).did_exceed_match_limit = false;
        (*this).work_count = 0;
        (*this).query_options = ptr::null();
        (*this).query_state = core::mem::zeroed();
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_cursor_exec_with_options(
    this: *mut TSQueryCursor,
    query: *const TSQuery,
    node: TSNode,
    query_options: *const TSQueryCursorOptions,
) {
    unsafe {
        ts_query_cursor_exec(this, query, node);
        if !query_options.is_null() {
            (*this).query_options = query_options;
            (*this).query_state = TSQueryCursorState {
                payload: (*query_options).payload,
                current_byte_offset: 0,
            };
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_cursor_set_byte_range(
    this: *mut TSQueryCursor,
    start_byte: u32,
    mut end_byte: u32,
) -> bool {
    unsafe {
        if end_byte == 0 {
            end_byte = u32::MAX;
        }
        if start_byte > end_byte {
            return false;
        }
        (*this).included_range.start_byte = start_byte;
        (*this).included_range.end_byte = end_byte;
        true
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_cursor_set_point_range(
    this: *mut TSQueryCursor,
    start_point: TSPoint,
    mut end_point: TSPoint,
) -> bool {
    unsafe {
        if end_point.row == 0 && end_point.column == 0 {
            end_point = POINT_MAX;
        }
        if point_gt(start_point, end_point) {
            return false;
        }
        (*this).included_range.start_point = start_point;
        (*this).included_range.end_point = end_point;
        true
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_cursor_set_containing_byte_range(
    this: *mut TSQueryCursor,
    start_byte: u32,
    mut end_byte: u32,
) -> bool {
    unsafe {
        if end_byte == 0 {
            end_byte = u32::MAX;
        }
        if start_byte > end_byte {
            return false;
        }
        (*this).containing_range.start_byte = start_byte;
        (*this).containing_range.end_byte = end_byte;
        true
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_cursor_set_containing_point_range(
    this: *mut TSQueryCursor,
    start_point: TSPoint,
    mut end_point: TSPoint,
) -> bool {
    unsafe {
        if end_point.row == 0 && end_point.column == 0 {
            end_point = POINT_MAX;
        }
        if point_gt(start_point, end_point) {
            return false;
        }
        (*this).containing_range.start_point = start_point;
        (*this).containing_range.end_point = end_point;
        true
    }
}

/// Search through all of the in-progress states, and find the captured node that occurs earliest
/// in the document.
unsafe fn query_cursor_first_in_progress_capture(
    this: *mut TSQueryCursor,
    state_index: *mut u32,
    byte_offset: *mut u32,
    pattern_index: *mut u32,
    is_definite: *mut bool,
) -> bool {
    unsafe {
        let mut result = false;
        *state_index = u32::MAX;
        *byte_offset = u32::MAX;
        *pattern_index = u32::MAX;
        let mut i = 0u32;
        while i < (*this).states.size {
            let state = (*this).states.get(i);
            if (*state).dead {
                i = i.wrapping_add(1);
                continue;
            }

            let captures =
                capture_list_pool_get(&(*this).capture_list_pool, (*state).capture_list_id);
            if u32::from((*state).consumed_capture_count) >= (*captures).size {
                i = i.wrapping_add(1);
                continue;
            }

            let node = (*(*captures).get(u32::from((*state).consumed_capture_count))).node;
            if ts_node_end_byte(node) <= (*this).included_range.start_byte
                || point_lte(ts_node_end_point(node), (*this).included_range.start_point)
            {
                (*state).consumed_capture_count =
                    ((*state).consumed_capture_count + 1) & CONSUMED_CAPTURE_COUNT_MASK;
                // C: `i--; continue;` followed by the loop's `i++`, so `i` is unchanged.
                continue;
            }

            let node_start_byte = ts_node_start_byte(node);
            if !result
                || node_start_byte < *byte_offset
                || (node_start_byte == *byte_offset
                    && u32::from((*state).pattern_index) < *pattern_index)
            {
                let step = (*(*this).query).steps.get(u32::from((*state).step_index));
                if !is_definite.is_null() {
                    // We're being a bit conservative here by asserting that the following step
                    // is not immediate, because this capture might end up being discarded if the
                    // following symbol in the tree isn't the required symbol for this step.
                    *is_definite = (*step).root_pattern_guaranteed && !(*step).is_immediate;
                } else if (*step).root_pattern_guaranteed {
                    i = i.wrapping_add(1);
                    continue;
                }

                result = true;
                *state_index = i;
                *byte_offset = node_start_byte;
                *pattern_index = u32::from((*state).pattern_index);
            }
            i = i.wrapping_add(1);
        }
        result
    }
}

/// Determine which node is first in a depth-first traversal.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_cursor__compare_nodes(left: TSNode, right: TSNode) -> i32 {
    unsafe {
        if left.id != right.id {
            let left_start = ts_node_start_byte(left);
            let right_start = ts_node_start_byte(right);
            if left_start < right_start {
                return -1;
            }
            if left_start > right_start {
                return 1;
            }
            let left_node_count = ts_node_end_byte(left);
            let right_node_count = ts_node_end_byte(right);
            if left_node_count > right_node_count {
                return -1;
            }
            if left_node_count < right_node_count {
                return 1;
            }
        }
        0
    }
}

/// Determine if either state contains a superset of the other state's captures.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_cursor__compare_captures(
    this: *mut TSQueryCursor,
    left_state: *mut QueryState,
    right_state: *mut QueryState,
    left_contains_right: *mut bool,
    right_contains_left: *mut bool,
) {
    unsafe {
        let left_captures =
            capture_list_pool_get(&(*this).capture_list_pool, (*left_state).capture_list_id);
        let right_captures =
            capture_list_pool_get(&(*this).capture_list_pool, (*right_state).capture_list_id);
        *left_contains_right = true;
        *right_contains_left = true;
        let mut i = 0u32;
        let mut j = 0u32;
        loop {
            if i < (*left_captures).size {
                if j < (*right_captures).size {
                    let left = (*left_captures).get(i);
                    let right = (*right_captures).get(j);
                    if (*left).node.id == (*right).node.id && (*left).index == (*right).index {
                        i += 1;
                        j += 1;
                    } else {
                        match ts_query_cursor__compare_nodes((*left).node, (*right).node) {
                            -1 => {
                                *right_contains_left = false;
                                i += 1;
                            }
                            1 => {
                                *left_contains_right = false;
                                j += 1;
                            }
                            _ => {
                                *right_contains_left = false;
                                *left_contains_right = false;
                                i += 1;
                                j += 1;
                            }
                        }
                    }
                } else {
                    *right_contains_left = false;
                    break;
                }
            } else {
                if j < (*right_captures).size {
                    *left_contains_right = false;
                }
                break;
            }
        }
    }
}

/// Order two in-progress states for the longest-match dedup pass. Within a
/// (start_depth, pattern_index) group, states with no captures sort first, as they are a subset of
/// every other state (so the dedup pass must always compare them). The rest sort by the start byte
/// of their first capture.
unsafe fn query_cursor_state_precedes(
    this: *const TSQueryCursor,
    a: *const QueryState,
    b: *const QueryState,
) -> bool {
    unsafe {
        if (*a).start_depth != (*b).start_depth {
            return (*a).start_depth < (*b).start_depth;
        }
        if (*a).pattern_index != (*b).pattern_index {
            return (*a).pattern_index < (*b).pattern_index;
        }
        let a_caps = capture_list_pool_get(&(*this).capture_list_pool, (*a).capture_list_id);
        let b_caps = capture_list_pool_get(&(*this).capture_list_pool, (*b).capture_list_id);
        if ((*a_caps).size == 0) != ((*b_caps).size == 0) {
            return (*a_caps).size == 0;
        }
        if (*a_caps).size == 0 {
            return false;
        }
        ts_node_start_byte((*(*a_caps).get(0)).node) < ts_node_start_byte((*(*b_caps).get(0)).node)
    }
}

/// Stable-sort the in-progress states with the order dictated by `query_cursor_state_precedes`.
/// This runs once per node, right before the dedup pass.
unsafe fn query_cursor_sort_states_by_capture(this: *mut TSQueryCursor) {
    unsafe {
        let states = &mut (*this).states as *mut QueryStateList;
        let mut i = 1u32;
        while i < (*states).size {
            // Fast+common path: this state is already ordered after its predecessor, so it does not
            // need to move.
            if !query_cursor_state_precedes(this, (*states).get(i), (*states).get(i - 1)) {
                i += 1;
                continue;
            }
            let key = *(*states).get(i);
            let mut j = i;
            loop {
                *(*states).get(j) = *(*states).get(j - 1);
                j -= 1;
                if !(j > 0 && query_cursor_state_precedes(this, &key, (*states).get(j - 1))) {
                    break;
                }
            }
            *(*states).get(j) = key;
            i += 1;
        }
    }
}

unsafe fn query_cursor_add_state(this: *mut TSQueryCursor, pattern: *const PatternEntry) {
    unsafe {
        let step = (*(*this).query).steps.get(u32::from((*pattern).step_index));
        let start_depth = (*this).depth.wrapping_sub(u32::from((*step).depth));

        // Keep the states array in ascending order of start_depth and pattern_index, so that it can
        // be processed more efficiently elsewhere. Usually, there is no work to do here because of
        // two facts:
        // * States with lower start_depth are naturally added first due to the order in which nodes
        //   are visited.
        // * Earlier patterns are naturally added first because of the ordering of the pattern_map
        //   data structure that's used to initiate matches.
        //
        // This loop is only needed in cases where two conditions hold:
        // * A pattern consists of more than one sibling node, so that its states remain in progress
        //   after exiting the node that started the match.
        // * The first node in the pattern matches against multiple nodes at the same depth.
        //
        // An example of this is the pattern '((comment)* (function))'. If multiple `comment` nodes
        // appear in a row, then we may initiate a new state for this pattern while another state for
        // the same pattern is already in progress. If there are multiple patterns like this in a
        // query, then this loop will need to execute in order to keep the states ordered by
        // pattern_index.
        let mut index = (*this).states.size;
        while index > 0 {
            let prev_state = (*this).states.get(index - 1);
            if u32::from((*prev_state).start_depth) < start_depth {
                break;
            }
            if u32::from((*prev_state).start_depth) == start_depth {
                // Avoid inserting an unnecessary duplicate state, which would be immediately pruned
                // by the longest-match criteria.
                if (*prev_state).pattern_index == (*pattern).pattern_index
                    && (*prev_state).step_index == (*pattern).step_index
                {
                    return;
                }
                if (*prev_state).pattern_index <= (*pattern).pattern_index {
                    break;
                }
            }
            index -= 1;
        }

        (*this).states.insert(
            index,
            QueryState {
                id: u32::MAX,
                capture_list_id: CAPTURE_LIST_NONE,
                heap_insert_order: u32::MAX,
                step_index: (*pattern).step_index,
                pattern_index: (*pattern).pattern_index,
                start_depth: start_depth as u16,
                consumed_capture_count: 0,
                seeking_immediate_match: true,
                has_in_progress_alternatives: false,
                needs_parent: (*step).depth == 1,
                dead: false,
                skipped_quantifier: false,
            },
        );
    }
}

/// Acquire a capture list for this state. If there are no capture lists left in the pool, this will
/// steal the capture list from another existing state, and mark that other state as 'dead'.
unsafe fn query_cursor_prepare_to_capture(
    this: *mut TSQueryCursor,
    state: *mut QueryState,
    state_index_to_preserve: u32,
) -> *mut CaptureList {
    unsafe {
        if (*state).capture_list_id == CAPTURE_LIST_NONE {
            (*state).capture_list_id = capture_list_pool_acquire(&mut (*this).capture_list_pool);

            // If there are no capture lists left in the pool, then terminate whichever state has
            // captured the earliest node in the document, and steal its capture list.
            if (*state).capture_list_id == CAPTURE_LIST_NONE {
                (*this).did_exceed_match_limit = true;
                let mut state_index = 0u32;
                let mut byte_offset = 0u32;
                let mut pattern_index = 0u32;
                if query_cursor_first_in_progress_capture(
                    this,
                    &mut state_index,
                    &mut byte_offset,
                    &mut pattern_index,
                    ptr::null_mut(),
                ) && state_index != state_index_to_preserve
                {
                    let other_state = (*this).states.get(state_index);
                    (*state).capture_list_id = (*other_state).capture_list_id;
                    (*other_state).capture_list_id = CAPTURE_LIST_NONE;
                    (*other_state).dead = true;
                    let list = capture_list_pool_get_mut(
                        &mut (*this).capture_list_pool,
                        (*state).capture_list_id,
                    );
                    (*list).clear();
                    return list;
                } else {
                    return ptr::null_mut();
                }
            }
        }
        capture_list_pool_get_mut(&mut (*this).capture_list_pool, (*state).capture_list_id)
    }
}

unsafe fn query_cursor_capture(
    this: *mut TSQueryCursor,
    state: *mut QueryState,
    step: *mut QueryStep,
    node: TSNode,
) {
    unsafe {
        if (*state).dead {
            return;
        }
        let capture_list = query_cursor_prepare_to_capture(this, state, u32::MAX);
        if capture_list.is_null() {
            (*state).dead = true;
            return;
        }

        for j in 0..MAX_STEP_CAPTURE_COUNT {
            let capture_id = (*step).capture_ids[j];
            if capture_id == NONE {
                break;
            }
            (*capture_list).push(TSQueryCapture {
                node,
                index: u32::from(capture_id),
            });
        }
    }
}

/// Duplicate the given state and insert the newly-created state immediately after the given state
/// in the `states` array. Ensures that the given state reference is still valid, even if the
/// states array is reallocated.
unsafe fn query_cursor_copy_state(
    this: *mut TSQueryCursor,
    state_ref: *mut *mut QueryState,
) -> *mut QueryState {
    unsafe {
        let state = *state_ref as *const QueryState;
        let state_index = state.offset_from((*this).states.contents) as u32;
        let mut copy = *state;
        copy.capture_list_id = CAPTURE_LIST_NONE;

        // If the state has captures, copy its capture list.
        if (*state).capture_list_id != CAPTURE_LIST_NONE {
            let new_captures = query_cursor_prepare_to_capture(this, &mut copy, state_index);
            if new_captures.is_null() {
                return ptr::null_mut();
            }
            let old_captures =
                capture_list_pool_get(&(*this).capture_list_pool, (*state).capture_list_id);
            (*new_captures).push_all(&*old_captures);
        }

        (*this).states.insert(state_index + 1, copy);
        *state_ref = (*this).states.get(state_index);
        (*this).states.get(state_index + 1)
    }
}

unsafe fn query_cursor_should_descend(
    this: *mut TSQueryCursor,
    node_intersects_range: bool,
) -> bool {
    unsafe {
        if node_intersects_range && (*this).depth < (*this).max_start_depth {
            return true;
        }

        // If there are in-progress matches whose remaining steps occur deeper in the tree, then
        // descend.
        for i in 0..(*this).states.size {
            let state = (*this).states.get(i);
            let next_step = (*(*this).query).steps.get(u32::from((*state).step_index));
            if (*next_step).depth != PATTERN_DONE_MARKER
                && u32::from((*state).start_depth) + u32::from((*next_step).depth) > (*this).depth
            {
                return true;
            }
        }

        if (*this).depth >= (*this).max_start_depth {
            return false;
        }

        // If the current node is hidden, then a non-rooted pattern might match one if its roots
        // inside of this node, and match another of its roots as part of a sibling node, so we may
        // need to descend.
        if !(*this).on_visible_node {
            // Descending into a repetition node outside of the range can be expensive, because
            // these nodes can have many visible children. Avoid descending into repetition nodes
            // unless we have already determined that this query can match rootless patterns inside
            // of this type of repetition node.
            let subtree = tree_cursor_current_subtree(&(*this).cursor);
            if ts_subtree_is_repetition(subtree) != 0 {
                let (_index, exists) = (*(*this).query)
                    .repeat_symbols_with_rootless_patterns
                    .search_sorted_by(|s| u32::from(*s), u32::from(ts_subtree_symbol(subtree)));
                return exists;
            }

            return true;
        }

        false
    }
}

/// `ts_tree_cursor_current_subtree` (an inline function of `tree_cursor.h`).
unsafe fn tree_cursor_current_subtree(this: *const TSTreeCursor) -> Subtree {
    unsafe {
        let this = this as *const TreeCursor;
        let last_entry = (*this).stack.back();
        *(*last_entry).subtree
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn range_intersects(a: *const TSRange, b: *const TSRange) -> bool {
    unsafe {
        let is_empty = (*a).start_byte == (*a).end_byte;
        ((*a).end_byte > (*b).start_byte || (is_empty && (*a).end_byte == (*b).start_byte))
            && (point_gt((*a).end_point, (*b).start_point)
                || (is_empty && point_eq((*a).end_point, (*b).start_point)))
            && (*a).start_byte < (*b).end_byte
            && point_lt((*a).start_point, (*b).end_point)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn range_within(a: *const TSRange, b: *const TSRange) -> bool {
    unsafe {
        (*a).start_byte >= (*b).start_byte
            && point_gte((*a).start_point, (*b).start_point)
            && (*a).end_byte <= (*b).end_byte
            && point_lte((*a).end_point, (*b).end_point)
    }
}

/// Walk the tree, processing patterns until at least one pattern finishes. If one or more
/// patterns finish, return `true` and store their states in the `finished_states` array. Multiple
/// patterns can finish on the same node. If there are no more matches, return `false`.
unsafe fn query_cursor_advance(this: *mut TSQueryCursor, stop_on_definite_step: bool) -> bool {
    unsafe {
        let mut did_match = false;
        loop {
            if (*this).halted {
                while (*this).states.size > 0 {
                    let state = (*this).states.pop();
                    capture_list_pool_release(
                        &mut (*this).capture_list_pool,
                        state.capture_list_id,
                    );
                }
            }

            if did_match || (*this).halted {
                return did_match;
            }

            // Consult the progress callback after a bounded amount of work. Only iterations that do
            // work are charged.
            (*this).work_count = (*this)
                .work_count
                .wrapping_add(1)
                .wrapping_add((*this).states.size);
            if (*this).work_count >= WORK_PER_QUERY_CALLBACK_CHECK {
                (*this).work_count = 0;
                if !(*this).query_options.is_null()
                    && let Some(callback) = (*(*this).query_options).progress_callback
                {
                    (*this).query_state.current_byte_offset =
                        ts_node_start_byte(ts_tree_cursor_current_node(&(*this).cursor));
                    if callback(&mut (*this).query_state) {
                        // Halt the same way reaching the end of the tree does. The next iteration
                        // discards the in-progress states, so only finished matches are returned.
                        (*this).halted = true;
                        continue;
                    }
                }
            }

            // Exit the current node.
            if (*this).ascending {
                if (*this).on_visible_node {
                    // After leaving a node, remove any states that cannot make further progress.
                    let mut deleted_count = 0u32;
                    let n = (*this).states.size;
                    for i in 0..n {
                        let state = (*this).states.get(i);
                        let step = (*(*this).query).steps.get(u32::from((*state).step_index));

                        // If a state completed its pattern inside of this node, but was deferred
                        // from finishing in order to search for longer matches, mark it as finished.
                        if (*step).depth == PATTERN_DONE_MARKER
                            && (u32::from((*state).start_depth) > (*this).depth
                                || (*this).depth == 0)
                        {
                            query_cursor_push_finished_state(this, state);
                            did_match = true;
                            deleted_count += 1;
                        }
                        // If a state needed to match something within this node, then remove that
                        // state as it has failed to match.
                        else if (*step).depth != PATTERN_DONE_MARKER
                            && u32::from((*state).start_depth) + u32::from((*step).depth)
                                > (*this).depth
                        {
                            capture_list_pool_release(
                                &mut (*this).capture_list_pool,
                                (*state).capture_list_id,
                            );
                            deleted_count += 1;
                        } else if deleted_count > 0 {
                            *(*this).states.get(i - deleted_count) = *state;
                        }
                    }
                    (*this).states.size -= deleted_count;
                }

                // Leave this node by stepping to its next sibling or to its parent.
                match ts_tree_cursor_goto_next_sibling_internal(&mut (*this).cursor) {
                    TREE_CURSOR_STEP_VISIBLE => {
                        if !(*this).on_visible_node {
                            (*this).depth = (*this).depth.wrapping_add(1);
                            (*this).on_visible_node = true;
                        }
                        (*this).ascending = false;
                    }
                    TREE_CURSOR_STEP_HIDDEN => {
                        if (*this).on_visible_node {
                            (*this).depth = (*this).depth.wrapping_sub(1);
                            (*this).on_visible_node = false;
                        }
                        (*this).ascending = false;
                    }
                    _ => {
                        if ts_tree_cursor_goto_parent(&mut (*this).cursor) {
                            (*this).depth = (*this).depth.wrapping_sub(1);
                        } else {
                            (*this).halted = true;
                        }
                    }
                }
            }
            // Enter a new node.
            else {
                let node = ts_tree_cursor_current_node(&(*this).cursor);
                let parent_node = ts_tree_cursor_parent_node(&(*this).cursor);

                let parent_intersects_range = ts_node_is_null(parent_node) || {
                    let parent_range = TSRange {
                        start_point: ts_node_start_point(parent_node),
                        end_point: ts_node_end_point(parent_node),
                        start_byte: ts_node_start_byte(parent_node),
                        end_byte: ts_node_end_byte(parent_node),
                    };
                    range_intersects(&parent_range, &(*this).included_range)
                };
                let node_range = TSRange {
                    start_point: ts_node_start_point(node),
                    end_point: ts_node_end_point(node),
                    start_byte: ts_node_start_byte(node),
                    end_byte: ts_node_end_byte(node),
                };
                let node_intersects_range = parent_intersects_range
                    && range_intersects(&node_range, &(*this).included_range);
                let node_intersects_containing_range =
                    range_intersects(&node_range, &(*this).containing_range);
                let node_within_containing_range =
                    range_within(&node_range, &(*this).containing_range);

                if node_within_containing_range && (*this).on_visible_node {
                    let symbol = ts_node_symbol(node);
                    let is_named = ts_node_is_named(node);
                    let is_missing = ts_node_is_missing(node);
                    let mut has_later_siblings = false;
                    let mut has_later_named_siblings = false;
                    let mut can_have_later_siblings_with_this_field = false;
                    let mut field_id: TSFieldId = 0;
                    let mut supertypes: [TSSymbol; 8] = [0; 8];
                    let mut supertype_count = 8u32;
                    ts_tree_cursor_current_status(
                        &(*this).cursor,
                        &mut field_id,
                        &mut has_later_siblings,
                        &mut has_later_named_siblings,
                        &mut can_have_later_siblings_with_this_field,
                        supertypes.as_mut_ptr(),
                        &mut supertype_count,
                    );

                    let node_is_error = symbol == ts_builtin_sym_error;
                    let parent_is_error = !ts_node_is_null(parent_node)
                        && ts_node_symbol(parent_node) == ts_builtin_sym_error;

                    // Add new states for any patterns whose root node is a wildcard.
                    if !node_is_error {
                        for i in 0..u32::from((*(*this).query).wildcard_root_pattern_count) {
                            let pattern = (*(*this).query).pattern_map.get(i);

                            // If this node matches the first step of the pattern, then add a new
                            // state at the start of this pattern.
                            let step = (*(*this).query).steps.get(u32::from((*pattern).step_index));
                            let start_depth = (*this).depth.wrapping_sub(u32::from((*step).depth));
                            if (if (*pattern).is_rooted {
                                node_intersects_range
                            } else {
                                parent_intersects_range && !parent_is_error
                            }) && ((*step).field == 0 || field_id == (*step).field)
                                && ((*step).supertype_symbol == 0 || supertype_count > 0)
                                && (start_depth <= (*this).max_start_depth)
                            {
                                query_cursor_add_state(this, pattern);
                            }
                        }
                    }

                    // Add new states for any patterns whose root node matches this node.
                    let mut i = 0u32;
                    if query_pattern_map_search((*this).query, symbol, &mut i) {
                        let mut pattern = (*(*this).query).pattern_map.get(i);

                        let mut step = (*(*this).query).steps.get(u32::from((*pattern).step_index));
                        let start_depth = (*this).depth.wrapping_sub(u32::from((*step).depth));
                        loop {
                            // If this node matches the first step of the pattern, then add a new
                            // state at the start of this pattern.
                            if (if (*pattern).is_rooted {
                                node_intersects_range
                            } else {
                                parent_intersects_range && !parent_is_error
                            }) && ((*step).field == 0 || field_id == (*step).field)
                                && (start_depth <= (*this).max_start_depth)
                            {
                                query_cursor_add_state(this, pattern);
                            }

                            // Advance to the next pattern whose root node matches this node.
                            i += 1;
                            if i == (*(*this).query).pattern_map.size {
                                break;
                            }
                            pattern = (*(*this).query).pattern_map.get(i);
                            step = (*(*this).query).steps.get(u32::from((*pattern).step_index));
                            if (*step).symbol != symbol {
                                break;
                            }
                        }
                    }

                    // Update all of the in-progress states with current node.
                    let mut j = 0u32;
                    let mut copy_count;
                    while j < (*this).states.size {
                        let mut state = (*this).states.get(j);
                        let step = (*(*this).query).steps.get(u32::from((*state).step_index));
                        (*state).has_in_progress_alternatives = false;
                        copy_count = 0u32;

                        'body: {
                            // Check that the node matches all of the criteria for the next step of
                            // the pattern.
                            if u32::from((*state).start_depth) + u32::from((*step).depth)
                                != (*this).depth
                            {
                                break 'body;
                            }

                            // Determine if this node matches this step of the pattern, and also if
                            // this node can have later siblings that match this step of the
                            // pattern.
                            let mut node_does_match;
                            if (*step).symbol == WILDCARD_SYMBOL {
                                if (*step).is_missing {
                                    node_does_match = is_missing;
                                } else {
                                    node_does_match =
                                        !node_is_error && (is_named || !(*step).is_named);
                                }
                            } else {
                                node_does_match =
                                    symbol == (*step).symbol && (!(*step).is_missing || is_missing);
                            }
                            let mut later_sibling_can_match = has_later_siblings;
                            if ((*step).is_immediate && is_named && !(*state).skipped_quantifier)
                                || (*state).seeking_immediate_match
                            {
                                later_sibling_can_match = false;
                            }
                            if (*step).is_last_child && has_later_named_siblings {
                                node_does_match = false;
                            }
                            if (*step).supertype_symbol != 0 {
                                let has_supertype = supertypes
                                    .iter()
                                    .take(supertype_count as usize)
                                    .any(|&t| t == (*step).supertype_symbol);
                                if !has_supertype {
                                    node_does_match = false;
                                }
                            }
                            if (*step).field != 0 {
                                if (*step).field == field_id {
                                    if !can_have_later_siblings_with_this_field {
                                        later_sibling_can_match = false;
                                    }
                                } else {
                                    node_does_match = false;
                                }
                            }

                            if (*step).negated_field_list_id != 0 {
                                let mut negated_field_ids = (*(*this).query)
                                    .negated_fields
                                    .get(u32::from((*step).negated_field_list_id));
                                loop {
                                    let negated_field_id = *negated_field_ids;
                                    if negated_field_id != 0 {
                                        negated_field_ids = negated_field_ids.add(1);
                                        if !ts_node_child_by_field_id(node, negated_field_id)
                                            .id
                                            .is_null()
                                        {
                                            node_does_match = false;
                                            break;
                                        }
                                    } else {
                                        break;
                                    }
                                }
                            }

                            // Remove states immediately if it is ever clear that they cannot match.
                            if !node_does_match {
                                if !later_sibling_can_match {
                                    capture_list_pool_release(
                                        &mut (*this).capture_list_pool,
                                        (*state).capture_list_id,
                                    );
                                    (*this).states.erase(j);
                                    j = j.wrapping_sub(1);
                                }
                                break 'body;
                            }

                            // Some patterns can match their root node in multiple ways, capturing
                            // different children. If this pattern step could match later children
                            // within the same parent, then this query state cannot simply be
                            // updated in place. It must be split into two states: one that matches
                            // this node, and one which skips over this node, to preserve the
                            // possibility of matching later siblings.
                            if later_sibling_can_match
                                && ((*step).contains_captures
                                    || query_step_is_fallible((*this).query, (*state).step_index))
                                && !query_cursor_copy_state(this, &mut state).is_null()
                            {
                                copy_count += 1;
                            }

                            // If this pattern started with a wildcard, such that the pattern map
                            // actually points to the *second* step of the pattern, then check that
                            // the node has a parent, and capture the parent node if necessary.
                            if (*state).needs_parent {
                                let parent = ts_tree_cursor_parent_node(&(*this).cursor);
                                if ts_node_is_null(parent) {
                                    (*state).dead = true;
                                } else {
                                    (*state).needs_parent = false;
                                    let mut skipped_wildcard_step = step;
                                    loop {
                                        skipped_wildcard_step = skipped_wildcard_step.sub(1);
                                        if !((*skipped_wildcard_step).is_dead_end
                                            || (*skipped_wildcard_step).is_pass_through
                                            || (*skipped_wildcard_step).depth > 0)
                                        {
                                            break;
                                        }
                                    }
                                    if (*skipped_wildcard_step).capture_ids[0] != NONE {
                                        query_cursor_capture(
                                            this,
                                            state,
                                            skipped_wildcard_step,
                                            parent,
                                        );
                                    }
                                }
                            }

                            // If the current node is captured in this pattern, add it to the
                            // capture list.
                            if (*step).capture_ids[0] != NONE {
                                query_cursor_capture(this, state, step, node);
                            }

                            if (*state).dead {
                                (*this).states.erase(j);
                                j = j.wrapping_sub(1);
                                break 'body;
                            }

                            // Advance this state to the next step of its pattern.
                            (*state).step_index += 1;

                            let next_step =
                                (*(*this).query).steps.get(u32::from((*state).step_index));

                            // For a given step, if the current symbol is the wildcard symbol, `_`,
                            // and it is **not** named, meaning it should capture anonymous nodes,
                            // **and** the next step is immediate, we reuse the
                            // `seeking_immediate_match` flag to indicate that we are looking for an
                            // immediate match due to an unnamed wildcard symbol.
                            //
                            // The reason for this is that typically, anchors will not consider
                            // anonymous nodes, but we're special casing the wildcard symbol to
                            // allow for any immediate matches, regardless of whether they are named
                            // or not.
                            (*state).seeking_immediate_match = (*step).symbol == WILDCARD_SYMBOL
                                && !(*step).is_named
                                && (*next_step).is_immediate;
                            // The zero-skip's vacuous-anchor exemption only covers the immediate
                            // step it lands on. Once the state advances, a later anchor is normal.
                            (*state).skipped_quantifier = false;

                            if stop_on_definite_step && (*next_step).root_pattern_guaranteed {
                                did_match = true;
                            }

                            // If this state's next step has an alternative step, then copy the
                            // state in order to pursue both alternatives. The alternative step
                            // itself may have an alternative, so this is an interactive process.
                            let mut end_index = j.wrapping_add(1);
                            let mut k = j;
                            while k < end_index {
                                let mut child_state = (*this).states.get(k);
                                let child_step = (*(*this).query)
                                    .steps
                                    .get(u32::from((*child_state).step_index));
                                'child: {
                                    if (*child_step).alternative_index != NONE {
                                        // A "dead-end" step exists only to add a non-sequential
                                        // jump into the step sequence, via its alternative index.
                                        // When a state reaches a dead-end step, it jumps straight to
                                        // the step's alternative.
                                        if (*child_step).is_dead_end {
                                            (*child_state).step_index =
                                                (*child_step).alternative_index;
                                            k = k.wrapping_sub(1);
                                            break 'child;
                                        }

                                        // A "pass-through" step exists only to add a branch into
                                        // the step sequence, via its alternative_index. When a state
                                        // reaches a pass-through step, it splits in order to process
                                        // the alternative step, and then it advances to the next
                                        // step.
                                        if (*child_step).is_pass_through {
                                            (*child_state).step_index += 1;
                                            k = k.wrapping_sub(1);
                                        }

                                        // A `?`/`*` zero-skip past a step that carries a trailing
                                        // last-child anchor transfers that requirement to the last
                                        // matched node. The skip is only valid if that node really
                                        // is the last named child.
                                        if (*child_step).alternative_is_skip
                                            && (*child_step).is_last_child
                                            && has_later_named_siblings
                                        {
                                            break 'child;
                                        }

                                        let copy = query_cursor_copy_state(this, &mut child_state);
                                        if !copy.is_null() {
                                            end_index = end_index.wrapping_add(1);
                                            copy_count += 1;
                                            (*copy).step_index = (*child_step).alternative_index;
                                            if (*child_step).is_pass_through {
                                                (*copy).seeking_immediate_match = true;
                                            }
                                            // Taking a `?`/`*` zero-skip means the quantified
                                            // subpattern matched nothing. How an adjacent anchor
                                            // behaves then depends on where it sat:
                                            if (*child_step).alternative_is_skip {
                                                if !(*child_step).is_immediate {
                                                    let skip_target = (*(*this).query).steps.get(
                                                        u32::from((*child_step).alternative_index),
                                                    );
                                                    // No leading anchor on the skipped step, so an
                                                    // immediately-following anchor on the skip
                                                    // target is vacuous (`Q* . B` with zero `Q` lets
                                                    // `B` match anywhere).
                                                    (*copy).skipped_quantifier =
                                                        (*skip_target).depth == (*child_step).depth;
                                                } else if (*(*(*this).query).steps.get(u32::from(
                                                    (*child_state).step_index.wrapping_sub(1),
                                                )))
                                                .depth
                                                    < (*child_step).depth
                                                {
                                                    // The skipped step was the parent's first child
                                                    // pattern and carried a leading *boundary*
                                                    // anchor (`(P . Q* Y)`). Transfer the
                                                    // first-child requirement to the skip target so
                                                    // it survives the empty run: `Y` must still be
                                                    // the parent's first named child.
                                                    (*copy).seeking_immediate_match = true;
                                                }
                                                // Otherwise the skipped step carried a leading
                                                // *between* anchor (`A . Q* ...`): with zero `Q`
                                                // that adjacency vanishes, while the skip target's
                                                // own anchor, if any, still applies (`A . Q* . B`
                                                // stays adjacent).
                                            }
                                        }
                                    }
                                }
                                k = k.wrapping_add(1);
                            }
                        }

                        j = j.wrapping_add(1 + copy_count);
                    }

                    // Order states by capture position so the dedup pass below can stop scanning a
                    // group once the remaining states are disjoint from the current one.
                    query_cursor_sort_states_by_capture(this);

                    let mut j = 0u32;
                    while j < (*this).states.size {
                        let state = (*this).states.get(j);
                        if (*state).dead {
                            (*this).states.erase(j);
                            // C: `j--; continue;` then the loop's `j++`: `j` is unchanged.
                            continue;
                        }

                        // Enforce the longest-match criteria. When a query pattern contains
                        // optional or repeated nodes, this is necessary to avoid multiple redundant
                        // states, where one state has a strict subset of another state's captures.
                        let mut did_remove = false;
                        let mut k = j + 1;
                        while k < (*this).states.size {
                            let other_state = (*this).states.get(k);

                            // Query states are kept in ascending order of start_depth and
                            // pattern_index, and (via the above call to
                            // `query_cursor_sort_states_by_capture`) in ascending order of
                            // first-capture position within each such group.
                            //
                            // Since the longest-match criteria is only used for deduping matches of
                            // the same pattern and root node, we only need to perform pairwise
                            // comparisons within a small slice of the states array.
                            if (*other_state).start_depth != (*state).start_depth
                                || (*other_state).pattern_index != (*state).pattern_index
                            {
                                break;
                            }

                            // States in a group acquire their first capture in tree-traversal
                            // order, so the group is ordered by first-capture position. Once
                            // `other_state`'s captures begin at or after where `state`'s captures
                            // end, `other_state` (and every state after it in the group) is
                            // disjoint from `state`: neither can be a capture-subset of the other,
                            // so there is nothing to drop and no longest-match alternative to
                            // record. Stop scanning `state` against the rest of the group.
                            let state_captures = capture_list_pool_get(
                                &(*this).capture_list_pool,
                                (*state).capture_list_id,
                            );
                            let other_captures = capture_list_pool_get(
                                &(*this).capture_list_pool,
                                (*other_state).capture_list_id,
                            );
                            if (*state_captures).size > 0
                                && (*other_captures).size > 0
                                && ts_node_start_byte((*(*other_captures).get(0)).node)
                                    >= ts_node_end_byte(
                                        (*(*state_captures).get((*state_captures).size - 1)).node,
                                    )
                            {
                                break;
                            }

                            let mut left_contains_right = false;
                            let mut right_contains_left = false;
                            ts_query_cursor__compare_captures(
                                this,
                                state,
                                other_state,
                                &mut left_contains_right,
                                &mut right_contains_left,
                            );
                            if left_contains_right {
                                if (*state).step_index == (*other_state).step_index
                                    && ((*other_state).seeking_immediate_match
                                        || !(*state).seeking_immediate_match)
                                {
                                    capture_list_pool_release(
                                        &mut (*this).capture_list_pool,
                                        (*other_state).capture_list_id,
                                    );
                                    (*this).states.erase(k);
                                    // C: `k--; continue;` then `k++`: `k` is unchanged.
                                    continue;
                                }
                                (*other_state).has_in_progress_alternatives = true;
                            }
                            if right_contains_left {
                                if (*state).step_index == (*other_state).step_index
                                    && ((*state).seeking_immediate_match
                                        || !(*other_state).seeking_immediate_match)
                                {
                                    capture_list_pool_release(
                                        &mut (*this).capture_list_pool,
                                        (*state).capture_list_id,
                                    );
                                    (*this).states.erase(j);
                                    j = j.wrapping_sub(1);
                                    did_remove = true;
                                    break;
                                }
                                (*state).has_in_progress_alternatives = true;
                            }
                            k += 1;
                        }

                        // If the state is at the end of its pattern, remove it from the list of
                        // in-progress states and add it to the list of finished states.
                        if !did_remove {
                            let next_step =
                                (*(*this).query).steps.get(u32::from((*state).step_index));
                            if (*next_step).depth == PATTERN_DONE_MARKER
                                && !(*state).has_in_progress_alternatives
                            {
                                query_cursor_push_finished_state(this, state);
                                let index = state.offset_from((*this).states.contents) as u32;
                                (*this).states.erase(index);
                                did_match = true;
                                j = j.wrapping_sub(1);
                            }
                        }
                        j = j.wrapping_add(1);
                    }
                }

                if node_intersects_containing_range
                    && query_cursor_should_descend(this, node_intersects_range)
                {
                    match ts_tree_cursor_goto_first_child_internal(&mut (*this).cursor) {
                        TREE_CURSOR_STEP_VISIBLE => {
                            (*this).depth = (*this).depth.wrapping_add(1);
                            (*this).on_visible_node = true;
                            continue;
                        }
                        TREE_CURSOR_STEP_HIDDEN => {
                            (*this).on_visible_node = false;
                            continue;
                        }
                        _ => {}
                    }
                }

                (*this).ascending = true;
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_cursor_next_match(
    this: *mut TSQueryCursor,
    match_: *mut TSQueryMatch,
) -> bool {
    unsafe {
        if (*this).finished_states.size == 0 && !query_cursor_advance(this, false) {
            return false;
        }
        if (*this).finished_states_heap_size > 0 {
            query_cursor_heapify_finished_states(this);
        }

        let mut state_index = 0u32;
        if (*this).finished_states_heap_size > 0 {
            for i in 1..(*this).finished_states.size {
                let state = (*this).finished_states.get(i);
                let earliest_state = (*this).finished_states.get(state_index);
                if (*state).heap_insert_order < (*earliest_state).heap_insert_order {
                    state_index = i;
                }
            }
        }

        let state = (*this).finished_states.get(state_index);
        if (*state).id == u32::MAX {
            (*state).id = (*this).next_state_id;
            (*this).next_state_id = (*this).next_state_id.wrapping_add(1);
        }
        (*match_).id = (*state).id;
        (*match_).pattern_index = (*state).pattern_index;
        let captures = capture_list_pool_get(&(*this).capture_list_pool, (*state).capture_list_id);
        (*match_).captures = (*captures).contents;
        (*match_).capture_count = (*captures).size as u16;
        capture_list_pool_release(&mut (*this).capture_list_pool, (*state).capture_list_id);
        if (*this).finished_states_heap_size > 0 {
            finished_state_erase(
                &mut (*this).finished_states,
                state_index,
                &(*this).capture_list_pool,
            );
            (*this).finished_states_heap_size = (*this).finished_states.size;
        } else {
            (*this).finished_states.erase(state_index);
        }
        true
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_cursor_remove_match(this: *mut TSQueryCursor, match_id: u32) {
    unsafe {
        if (*this).finished_states_heap_size > 0 {
            query_cursor_heapify_finished_states(this);
        }

        for i in 0..(*this).finished_states.size {
            let state = (*this).finished_states.get(i);
            if (*state).id == match_id {
                capture_list_pool_release(&mut (*this).capture_list_pool, (*state).capture_list_id);
                if (*this).finished_states_heap_size > 0 {
                    finished_state_erase(
                        &mut (*this).finished_states,
                        i,
                        &(*this).capture_list_pool,
                    );
                    (*this).finished_states_heap_size = (*this).finished_states.size;
                } else {
                    (*this).finished_states.erase(i);
                }
                return;
            }
        }

        // Remove unfinished query states as well to prevent future captures for a match being
        // removed.
        for i in 0..(*this).states.size {
            let state = (*this).states.get(i);
            if (*state).id == match_id {
                capture_list_pool_release(&mut (*this).capture_list_pool, (*state).capture_list_id);
                (*this).states.erase(i);
                return;
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_cursor_next_capture(
    this: *mut TSQueryCursor,
    match_: *mut TSQueryMatch,
    capture_index: *mut u32,
) -> bool {
    unsafe {
        // The goal here is to return captures in order, even though they may not be discovered in
        // order, because patterns can overlap. Search for matches until there is a finished capture
        // that is before any unfinished capture.
        loop {
            // Sift any newly pushed finished states into the heap.
            query_cursor_heapify_finished_states(this);

            // First, find the earliest capture in an unfinished match.
            let mut first_unfinished_capture_byte = 0u32;
            let mut first_unfinished_pattern_index = 0u32;
            let mut first_unfinished_state_index = 0u32;
            let mut first_unfinished_state_is_definite = false;
            let found_unfinished_state = query_cursor_first_in_progress_capture(
                this,
                &mut first_unfinished_state_index,
                &mut first_unfinished_capture_byte,
                &mut first_unfinished_pattern_index,
                &mut first_unfinished_state_is_definite,
            );

            // Then find the earliest capture in a finished match. The finished_states array is
            // maintained as a min-heap, so the earliest is always at index 0. Clean up
            // fully-consumed and out-of-range states from the heap root first.
            let mut first_finished_state: *mut QueryState = ptr::null_mut();
            let first_finished_capture_byte = first_unfinished_capture_byte;
            let first_finished_pattern_index = first_unfinished_pattern_index;
            while (*this).finished_states.size > 0 {
                let state = (*this).finished_states.get(0);
                let captures =
                    capture_list_pool_get(&(*this).capture_list_pool, (*state).capture_list_id);

                // Remove states whose captures are all consumed.
                if u32::from((*state).consumed_capture_count) >= (*captures).size {
                    capture_list_pool_release(
                        &mut (*this).capture_list_pool,
                        (*state).capture_list_id,
                    );
                    finished_state_pop(&mut (*this).finished_states, &(*this).capture_list_pool);
                    (*this).finished_states_heap_size = (*this).finished_states.size;
                    continue;
                }

                let node = (*(*captures).get(u32::from((*state).consumed_capture_count))).node;

                let node_precedes_range = ts_node_end_byte(node)
                    <= (*this).included_range.start_byte
                    || point_lte(ts_node_end_point(node), (*this).included_range.start_point);
                let node_follows_range = ts_node_start_byte(node)
                    >= (*this).included_range.end_byte
                    || point_gte(ts_node_start_point(node), (*this).included_range.end_point);
                let node_outside_of_range = node_precedes_range || node_follows_range;

                // Skip captures that are outside of the cursor's range.
                if node_outside_of_range {
                    (*state).consumed_capture_count =
                        ((*state).consumed_capture_count + 1) & CONSUMED_CAPTURE_COUNT_MASK;
                    finished_state_sift_down(
                        &mut (*this).finished_states,
                        0,
                        &(*this).capture_list_pool,
                    );
                    continue;
                }

                let node_start_byte = ts_node_start_byte(node);
                if node_start_byte < first_finished_capture_byte
                    || (node_start_byte == first_finished_capture_byte
                        && u32::from((*state).pattern_index) < first_finished_pattern_index)
                {
                    // (C also updates the two `first_finished_*` values here, but breaks right after.)
                    first_finished_state = state;
                }
                break;
            }

            // If there is finished capture that is clearly before any unfinished capture, then
            // return its match, and its capture index. Internally record the fact that the capture
            // has been 'consumed'.
            let state: *mut QueryState = if !first_finished_state.is_null() {
                first_finished_state
            } else if first_unfinished_state_is_definite {
                (*this).states.get(first_unfinished_state_index)
            } else {
                ptr::null_mut()
            };

            if !state.is_null() {
                if (*state).id == u32::MAX {
                    (*state).id = (*this).next_state_id;
                    (*this).next_state_id = (*this).next_state_id.wrapping_add(1);
                }
                (*match_).id = (*state).id;
                (*match_).pattern_index = (*state).pattern_index;
                let captures =
                    capture_list_pool_get(&(*this).capture_list_pool, (*state).capture_list_id);
                (*match_).captures = (*captures).contents;
                (*match_).capture_count = (*captures).size as u16;
                *capture_index = u32::from((*state).consumed_capture_count);
                (*state).consumed_capture_count =
                    ((*state).consumed_capture_count + 1) & CONSUMED_CAPTURE_COUNT_MASK;
                // If this state is in the finished_states heap, its sort key has changed (next
                // capture is now later in the document). Restore heap order.
                if state == first_finished_state {
                    finished_state_sift_down(
                        &mut (*this).finished_states,
                        0,
                        &(*this).capture_list_pool,
                    );
                }
                return true;
            }

            if capture_list_pool_is_empty(&(*this).capture_list_pool) && found_unfinished_state {
                capture_list_pool_release(
                    &mut (*this).capture_list_pool,
                    (*(*this).states.get(first_unfinished_state_index)).capture_list_id,
                );
                (*this).states.erase(first_unfinished_state_index);
            }

            // If there are no finished matches that are ready to be returned, then continue finding
            // more matches.
            if !query_cursor_advance(this, true) && (*this).finished_states.size == 0 {
                return false;
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_query_cursor_set_max_start_depth(
    this: *mut TSQueryCursor,
    max_start_depth: u32,
) {
    unsafe { (*this).max_start_depth = max_start_depth }
}
