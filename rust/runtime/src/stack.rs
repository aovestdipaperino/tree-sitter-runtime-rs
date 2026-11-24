//! Port of `src/stack.c` / `src/stack.h`: the parser's graph-structured stack.
//!
//! The `Stack` type is opaque to the rest of the C runtime (`Stack *`), so its internal layout is
//! free; it is still `repr(C)` and mirrors the C structs one to one. The slices and arrays the
//! functions return by value use the C layout, since the C parser reads them.
#![allow(clippy::missing_safety_doc)]
use crate::alloc::{ts_calloc, ts_free, ts_malloc};
use crate::api::*;
use crate::array::Array;
use crate::consts::*;
use crate::language::*;
use crate::length::*;
use crate::parser_abi::*;
use crate::subtree::*;
use crate::ts_assert;
use core::ffi::{c_char, c_int, c_void};
use core::ptr;

const MAX_LINK_COUNT: usize = 8;
const MAX_NODE_POOL_SIZE: u32 = 50;
const MAX_ITERATOR_COUNT: u32 = 64;

pub type StackVersion = u32;
pub const STACK_VERSION_NONE: StackVersion = u32::MAX;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct StackSlice {
    pub subtrees: SubtreeArray,
    pub version: StackVersion,
}
pub type StackSliceArray = Array<StackSlice>;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct StackSummaryEntry {
    pub position: Length,
    pub depth: u32,
    pub state: TSStateId,
}
pub type StackSummary = Array<StackSummaryEntry>;

#[repr(C)]
#[derive(Clone, Copy)]
struct StackLink {
    node: *mut StackNode,
    subtree: Subtree,
    is_pending: bool,
}

#[repr(C)]
struct StackNode {
    state: TSStateId,
    position: Length,
    links: [StackLink; MAX_LINK_COUNT],
    link_count: u16,
    ref_count: u32,
    error_cost: u32,
    node_count: u32,
    dynamic_precedence: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct StackIterator {
    node: *mut StackNode,
    subtrees: SubtreeArray,
    subtree_count: u32,
    is_pending: bool,
}

type StackNodeArray = Array<*mut StackNode>;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum StackStatus {
    Active,
    Paused,
    Halted,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct StackHead {
    node: *mut StackNode,
    summary: *mut StackSummary,
    node_count_at_last_error: u32,
    last_external_token: Subtree,
    lookahead_when_paused: Subtree,
    status: StackStatus,
}

#[repr(C)]
pub struct Stack {
    heads: Array<StackHead>,
    slices: StackSliceArray,
    iterators: Array<StackIterator>,
    node_pool: StackNodeArray,
    base_node: *mut StackNode,
    subtree_pool: *mut SubtreePool,
}

type StackAction = u32;
const STACK_ACTION_NONE: StackAction = 0;
const STACK_ACTION_STOP: StackAction = 1;
const STACK_ACTION_POP: StackAction = 2;

unsafe fn stack_node_retain(this: *mut StackNode) {
    unsafe {
        if this.is_null() {
            return;
        }
        ts_assert!((*this).ref_count > 0);
        (*this).ref_count = (*this).ref_count.wrapping_add(1);
        ts_assert!((*this).ref_count != 0);
    }
}

unsafe fn stack_node_release(
    mut this: *mut StackNode,
    pool: *mut StackNodeArray,
    subtree_pool: *mut SubtreePool,
) {
    unsafe {
        loop {
            ts_assert!((*this).ref_count != 0);
            (*this).ref_count -= 1;
            if (*this).ref_count > 0 {
                return;
            }

            let mut first_predecessor: *mut StackNode = ptr::null_mut();
            if (*this).link_count > 0 {
                let mut i = (*this).link_count as usize - 1;
                while i > 0 {
                    let link = (*this).links[i];
                    if !link.subtree.ptr.is_null() {
                        ts_subtree_release(subtree_pool, link.subtree);
                    }
                    stack_node_release(link.node, pool, subtree_pool);
                    i -= 1;
                }
                let link = (*this).links[0];
                if !link.subtree.ptr.is_null() {
                    ts_subtree_release(subtree_pool, link.subtree);
                }
                first_predecessor = (*this).links[0].node;
            }

            if (*pool).size < MAX_NODE_POOL_SIZE {
                (*pool).push(this);
            } else {
                ts_free(this as *mut c_void);
            }

            if first_predecessor.is_null() {
                return;
            }
            this = first_predecessor;
        }
    }
}

/// The number of nodes in the subtree, for measuring a stack version's progress.
unsafe fn stack_subtree_node_count(subtree: Subtree) -> u32 {
    let mut count = ts_subtree_visible_descendant_count(subtree);
    if ts_subtree_visible(subtree) {
        count = count.wrapping_add(1);
    }
    // Intermediate error nodes count too, though they are not visible.
    if ts_subtree_symbol(subtree) == ts_builtin_sym_error_repeat {
        count = count.wrapping_add(1);
    }
    count
}

unsafe fn stack_node_new(
    previous_node: *mut StackNode,
    subtree: Subtree,
    is_pending: bool,
    state: TSStateId,
    pool: *mut StackNodeArray,
) -> *mut StackNode {
    unsafe {
        let node = if (*pool).size > 0 {
            (*pool).pop()
        } else {
            ts_malloc(size_of::<StackNode>()) as *mut StackNode
        };
        // The C compound literal zeroes every field it does not name.
        ptr::write_bytes(node, 0, 1);
        (*node).ref_count = 1;
        (*node).link_count = 0;
        (*node).state = state;

        if !previous_node.is_null() {
            (*node).link_count = 1;
            (*node).links[0] = StackLink {
                node: previous_node,
                subtree,
                is_pending,
            };

            (*node).position = (*previous_node).position;
            (*node).error_cost = (*previous_node).error_cost;
            (*node).dynamic_precedence = (*previous_node).dynamic_precedence;
            (*node).node_count = (*previous_node).node_count;

            if !subtree.ptr.is_null() {
                (*node).error_cost = (*node)
                    .error_cost
                    .wrapping_add(ts_subtree_error_cost(subtree));
                (*node).position = length_add((*node).position, ts_subtree_total_size(subtree));
                (*node).node_count = (*node)
                    .node_count
                    .wrapping_add(stack_subtree_node_count(subtree));
                (*node).dynamic_precedence = (*node)
                    .dynamic_precedence
                    .wrapping_add(ts_subtree_dynamic_precedence(subtree));
            }
        } else {
            (*node).position = length_zero();
            (*node).error_cost = 0;
        }
        node
    }
}

unsafe fn stack_subtree_is_equivalent(left: Subtree, right: Subtree) -> bool {
    unsafe {
        if left.ptr == right.ptr {
            return true;
        }
        if left.ptr.is_null() || right.ptr.is_null() {
            return false;
        }

        // Symbols must match.
        if ts_subtree_symbol(left) != ts_subtree_symbol(right) {
            return false;
        }

        // If both have errors, don't bother keeping both.
        if ts_subtree_error_cost(left) > 0 && ts_subtree_error_cost(right) > 0 {
            return true;
        }

        ts_subtree_padding(left).bytes == ts_subtree_padding(right).bytes
            && ts_subtree_size(left).bytes == ts_subtree_size(right).bytes
            && ts_subtree_child_count(left) == ts_subtree_child_count(right)
            && ts_subtree_extra(left) == ts_subtree_extra(right)
            && ts_subtree_external_scanner_state_eq(left, right)
    }
}

unsafe fn stack_node_add_link(
    this: *mut StackNode,
    link: StackLink,
    subtree_pool: *mut SubtreePool,
) {
    unsafe {
        if link.node == this {
            return;
        }

        let mut i = 0;
        while i < (*this).link_count as usize {
            let existing_link = ptr::addr_of_mut!((*this).links[i]);
            if stack_subtree_is_equivalent((*existing_link).subtree, link.subtree) {
                // Ambiguities are normally kept until a pop reaches the same node by several paths,
                // but two links joining the same pair of nodes can be resolved right away.
                if (*existing_link).node == link.node {
                    if ts_subtree_dynamic_precedence(link.subtree)
                        > ts_subtree_dynamic_precedence((*existing_link).subtree)
                    {
                        ts_subtree_retain(link.subtree);
                        ts_subtree_release(subtree_pool, (*existing_link).subtree);
                        (*existing_link).subtree = link.subtree;
                        (*this).dynamic_precedence = (*link.node)
                            .dynamic_precedence
                            .wrapping_add(ts_subtree_dynamic_precedence(link.subtree));
                    }
                    return;
                }

                // If the previous nodes are mergeable, merge them recursively.
                if (*(*existing_link).node).state == (*link.node).state
                    && (*(*existing_link).node).position.bytes == (*link.node).position.bytes
                    && (*(*existing_link).node).error_cost == (*link.node).error_cost
                {
                    let mut j = 0;
                    while j < (*link.node).link_count as usize {
                        stack_node_add_link(
                            (*existing_link).node,
                            (*link.node).links[j],
                            subtree_pool,
                        );
                        j += 1;
                    }
                    let mut dynamic_precedence = (*link.node).dynamic_precedence;
                    if !link.subtree.ptr.is_null() {
                        dynamic_precedence = dynamic_precedence
                            .wrapping_add(ts_subtree_dynamic_precedence(link.subtree));
                    }
                    if dynamic_precedence > (*this).dynamic_precedence {
                        (*this).dynamic_precedence = dynamic_precedence;
                    }
                    return;
                }
            }
            i += 1;
        }

        if (*this).link_count as usize == MAX_LINK_COUNT {
            return;
        }

        stack_node_retain(link.node);
        let mut node_count = (*link.node).node_count;
        let mut dynamic_precedence = (*link.node).dynamic_precedence;
        let n = (*this).link_count as usize;
        (*this).links[n] = link;
        (*this).link_count += 1;

        if !link.subtree.ptr.is_null() {
            ts_subtree_retain(link.subtree);
            node_count = node_count.wrapping_add(stack_subtree_node_count(link.subtree));
            dynamic_precedence =
                dynamic_precedence.wrapping_add(ts_subtree_dynamic_precedence(link.subtree));
        }

        if node_count > (*this).node_count {
            (*this).node_count = node_count;
        }
        if dynamic_precedence > (*this).dynamic_precedence {
            (*this).dynamic_precedence = dynamic_precedence;
        }
    }
}

unsafe fn stack_head_delete(
    this: *mut StackHead,
    pool: *mut StackNodeArray,
    subtree_pool: *mut SubtreePool,
) {
    unsafe {
        if !(*this).node.is_null() {
            if !(*this).last_external_token.ptr.is_null() {
                ts_subtree_release(subtree_pool, (*this).last_external_token);
            }
            if !(*this).lookahead_when_paused.ptr.is_null() {
                ts_subtree_release(subtree_pool, (*this).lookahead_when_paused);
            }
            if !(*this).summary.is_null() {
                (*(*this).summary).delete();
                ts_free((*this).summary as *mut c_void);
            }
            stack_node_release((*this).node, pool, subtree_pool);
        }
    }
}

unsafe fn stack_add_version(
    this: *mut Stack,
    original_version: StackVersion,
    node: *mut StackNode,
) -> StackVersion {
    unsafe {
        let original = *(*this).heads.get(original_version);
        let head = StackHead {
            node,
            summary: ptr::null_mut(),
            node_count_at_last_error: original.node_count_at_last_error,
            last_external_token: original.last_external_token,
            status: StackStatus::Active,
            lookahead_when_paused: NULL_SUBTREE,
        };
        (*this).heads.push(head);
        stack_node_retain(node);
        if !head.last_external_token.ptr.is_null() {
            ts_subtree_retain(head.last_external_token);
        }
        (*this).heads.size - 1
    }
}

unsafe fn stack_add_slice(
    this: *mut Stack,
    original_version: StackVersion,
    node: *mut StackNode,
    subtrees: *mut SubtreeArray,
) {
    unsafe {
        let mut i = (*this).slices.size.wrapping_sub(1);
        while i.wrapping_add(1) > 0 {
            let version = (*(*this).slices.get(i)).version;
            if (*(*this).heads.get(version)).node == node {
                let slice = StackSlice {
                    subtrees: *subtrees,
                    version,
                };
                (*this).slices.insert(i + 1, slice);
                return;
            }
            i = i.wrapping_sub(1);
        }

        let version = stack_add_version(this, original_version, node);
        let slice = StackSlice {
            subtrees: *subtrees,
            version,
        };
        (*this).slices.push(slice);
    }
}

unsafe fn stack_iter(
    this: *mut Stack,
    version: StackVersion,
    callback: &mut dyn FnMut(*const StackIterator) -> StackAction,
    goal_subtree_count: i32,
) -> StackSliceArray {
    unsafe {
        (*this).slices.clear();
        (*this).iterators.clear();

        let head = (*this).heads.get(version);
        let mut new_iterator = StackIterator {
            node: (*head).node,
            subtrees: Array::new(),
            subtree_count: 0,
            is_pending: true,
        };

        let mut include_subtrees = false;
        if goal_subtree_count >= 0 {
            include_subtrees = true;
            new_iterator.subtrees.reserve(
                (ts_subtree_alloc_size(goal_subtree_count as u32) as u32)
                    / size_of::<Subtree>() as u32,
            );
        }

        (*this).iterators.push(new_iterator);

        while (*this).iterators.size > 0 {
            let mut i = 0u32;
            let mut size = (*this).iterators.size;
            while i < size {
                let iterator = (*this).iterators.get(i);
                let node = (*iterator).node;

                let action = callback(iterator);
                let should_pop = action & STACK_ACTION_POP != 0;
                let should_stop = action & STACK_ACTION_STOP != 0 || (*node).link_count == 0;

                if should_pop {
                    let mut subtrees = (*iterator).subtrees;
                    if !should_stop {
                        ts_subtree_array_copy(subtrees, &mut subtrees);
                    }
                    ts_subtree_array_reverse(&mut subtrees);
                    stack_add_slice(this, version, node, &mut subtrees);
                }

                if should_stop {
                    if !should_pop {
                        ts_subtree_array_delete(
                            (*this).subtree_pool,
                            ptr::addr_of_mut!((*iterator).subtrees),
                        );
                    }
                    (*this).iterators.erase(i);
                    // C: `i--, size--; continue;` on unsigned values.
                    i = i.wrapping_sub(1);
                    size = size.wrapping_sub(1);
                    i = i.wrapping_add(1);
                    continue;
                }

                let mut j = 1u32;
                while j <= (*node).link_count as u32 {
                    let next_iterator: *mut StackIterator;
                    let link: StackLink;
                    if j == (*node).link_count as u32 {
                        link = (*node).links[0];
                        next_iterator = (*this).iterators.get(i);
                    } else {
                        if (*this).iterators.size >= MAX_ITERATOR_COUNT {
                            j += 1;
                            continue;
                        }
                        link = (*node).links[j as usize];
                        let current_iterator = *(*this).iterators.get(i);
                        (*this).iterators.push(current_iterator);
                        next_iterator = (*this).iterators.back();
                        ts_subtree_array_copy(
                            (*next_iterator).subtrees,
                            ptr::addr_of_mut!((*next_iterator).subtrees),
                        );
                    }

                    (*next_iterator).node = link.node;
                    if !link.subtree.ptr.is_null() {
                        if include_subtrees {
                            (*next_iterator).subtrees.push(link.subtree);
                            ts_subtree_retain(link.subtree);
                        }

                        if !ts_subtree_extra(link.subtree) {
                            (*next_iterator).subtree_count += 1;
                            if !link.is_pending {
                                (*next_iterator).is_pending = false;
                            }
                        }
                    } else {
                        (*next_iterator).subtree_count += 1;
                        (*next_iterator).is_pending = false;
                    }
                    j += 1;
                }
                i += 1;
            }
        }

        (*this).slices
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_new(subtree_pool: *mut SubtreePool) -> *mut Stack {
    unsafe {
        let this = ts_calloc(1, size_of::<Stack>()) as *mut Stack;

        (*this).heads = Array::new();
        (*this).slices = Array::new();
        (*this).iterators = Array::new();
        (*this).node_pool = Array::new();
        (*this).heads.reserve(4);
        (*this).slices.reserve(4);
        (*this).iterators.reserve(4);
        (*this).node_pool.reserve(MAX_NODE_POOL_SIZE);

        (*this).subtree_pool = subtree_pool;
        (*this).base_node = stack_node_new(
            ptr::null_mut(),
            NULL_SUBTREE,
            false,
            1,
            &mut (*this).node_pool,
        );
        ts_stack_clear(this);

        this
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_delete(this: *mut Stack) {
    unsafe {
        if !(*this).slices.contents.is_null() {
            (*this).slices.delete();
        }
        if !(*this).iterators.contents.is_null() {
            (*this).iterators.delete();
        }
        stack_node_release(
            (*this).base_node,
            &mut (*this).node_pool,
            (*this).subtree_pool,
        );
        for i in 0..(*this).heads.size {
            stack_head_delete(
                (*this).heads.get(i),
                &mut (*this).node_pool,
                (*this).subtree_pool,
            );
        }
        (*this).heads.clear();
        if !(*this).node_pool.contents.is_null() {
            for i in 0..(*this).node_pool.size {
                ts_free(*(*this).node_pool.get(i) as *mut c_void);
            }
            (*this).node_pool.delete();
        }
        (*this).heads.delete();
        ts_free(this as *mut c_void);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_version_count(this: *const Stack) -> u32 {
    unsafe { (*this).heads.size }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_halted_version_count(this: *mut Stack) -> u32 {
    unsafe {
        let mut count = 0;
        for i in 0..(*this).heads.size {
            if (*(*this).heads.get(i)).status == StackStatus::Halted {
                count += 1;
            }
        }
        count
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_state(this: *const Stack, version: StackVersion) -> TSStateId {
    unsafe { (*(*(*this).heads.get(version)).node).state }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_position(this: *const Stack, version: StackVersion) -> Length {
    unsafe { (*(*(*this).heads.get(version)).node).position }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_last_external_token(
    this: *const Stack,
    version: StackVersion,
) -> Subtree {
    unsafe { (*(*this).heads.get(version)).last_external_token }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_set_last_external_token(
    this: *mut Stack,
    version: StackVersion,
    token: Subtree,
) {
    unsafe {
        let head = (*this).heads.get(version);
        if !token.ptr.is_null() {
            ts_subtree_retain(token);
        }
        if !(*head).last_external_token.ptr.is_null() {
            ts_subtree_release((*this).subtree_pool, (*head).last_external_token);
        }
        (*head).last_external_token = token;
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_error_cost(this: *const Stack, version: StackVersion) -> u32 {
    unsafe {
        let head = (*this).heads.get(version);
        let mut result = (*(*head).node).error_cost;
        if (*head).status == StackStatus::Paused
            || ((*(*head).node).state == ERROR_STATE
                && (*(*head).node).links[0].subtree.ptr.is_null())
        {
            result = result.wrapping_add(ERROR_COST_PER_RECOVERY);
        }
        result
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_node_count_since_error(
    this: *const Stack,
    version: StackVersion,
) -> u32 {
    unsafe {
        let head = (*this).heads.get(version);
        if (*(*head).node).node_count < (*head).node_count_at_last_error {
            (*head).node_count_at_last_error = (*(*head).node).node_count;
        }
        (*(*head).node)
            .node_count
            .wrapping_sub((*head).node_count_at_last_error)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_push(
    this: *mut Stack,
    version: StackVersion,
    subtree: Subtree,
    pending: bool,
    state: TSStateId,
) {
    unsafe {
        let head = (*this).heads.get(version);
        let new_node = stack_node_new(
            (*head).node,
            subtree,
            pending,
            state,
            &mut (*this).node_pool,
        );
        if subtree.ptr.is_null() {
            (*head).node_count_at_last_error = (*new_node).node_count;
        }
        (*head).node = new_node;
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_pop_count(
    this: *mut Stack,
    version: StackVersion,
    count: u32,
) -> StackSliceArray {
    unsafe {
        let mut callback = |iterator: *const StackIterator| {
            if (*iterator).subtree_count == count {
                STACK_ACTION_POP | STACK_ACTION_STOP
            } else {
                STACK_ACTION_NONE
            }
        };
        stack_iter(this, version, &mut callback, count as i32)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_pop_pending(
    this: *mut Stack,
    version: StackVersion,
) -> StackSliceArray {
    unsafe {
        let mut callback = |iterator: *const StackIterator| {
            if (*iterator).subtree_count >= 1 {
                if (*iterator).is_pending {
                    STACK_ACTION_POP | STACK_ACTION_STOP
                } else {
                    STACK_ACTION_STOP
                }
            } else {
                STACK_ACTION_NONE
            }
        };
        let pop = stack_iter(this, version, &mut callback, 0);
        if pop.size > 0 {
            ts_stack_renumber_version(this, (*pop.get(0)).version, version);
            (*pop.get(0)).version = version;
        }
        pop
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_pop_error(
    this: *mut Stack,
    version: StackVersion,
) -> SubtreeArray {
    unsafe {
        let node = (*(*this).heads.get(version)).node;
        for i in 0..(*node).link_count as usize {
            if !(*node).links[i].subtree.ptr.is_null()
                && ts_subtree_is_error((*node).links[i].subtree)
            {
                let mut found_error = false;
                let mut callback = |iterator: *const StackIterator| {
                    if (*iterator).subtrees.size > 0 {
                        if !found_error && ts_subtree_is_error(*(*iterator).subtrees.get(0)) {
                            found_error = true;
                            STACK_ACTION_POP | STACK_ACTION_STOP
                        } else {
                            STACK_ACTION_STOP
                        }
                    } else {
                        STACK_ACTION_NONE
                    }
                };
                let pop = stack_iter(this, version, &mut callback, 1);
                if pop.size > 0 {
                    ts_assert!(pop.size == 1);
                    ts_stack_renumber_version(this, (*pop.get(0)).version, version);
                    return (*pop.get(0)).subtrees;
                }
                break;
            }
        }
        // `(SubtreeArray) {.size = 0}`: everything else zero.
        Array::new()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_pop_all(
    this: *mut Stack,
    version: StackVersion,
) -> StackSliceArray {
    unsafe {
        let mut callback = |iterator: *const StackIterator| {
            if (*(*iterator).node).link_count == 0 {
                STACK_ACTION_POP
            } else {
                STACK_ACTION_NONE
            }
        };
        stack_iter(this, version, &mut callback, 0)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_record_summary(
    this: *mut Stack,
    version: StackVersion,
    max_depth: u32,
) {
    unsafe {
        let summary = ts_malloc(size_of::<StackSummary>()) as *mut StackSummary;
        ptr::write(summary, Array::new());
        let mut callback = |iterator: *const StackIterator| {
            let state = (*(*iterator).node).state;
            let depth = (*iterator).subtree_count;
            if depth > max_depth {
                return STACK_ACTION_STOP;
            }
            let mut i = (*summary).size.wrapping_sub(1);
            while i.wrapping_add(1) > 0 {
                let entry = *(*summary).get(i);
                if entry.depth < depth {
                    break;
                }
                if entry.depth == depth && entry.state == state {
                    return STACK_ACTION_NONE;
                }
                i = i.wrapping_sub(1);
            }
            (*summary).push(StackSummaryEntry {
                position: (*(*iterator).node).position,
                depth,
                state,
            });
            STACK_ACTION_NONE
        };
        stack_iter(this, version, &mut callback, -1);
        let head = (*this).heads.get(version);
        if !(*head).summary.is_null() {
            (*(*head).summary).delete();
            ts_free((*head).summary as *mut c_void);
        }
        (*head).summary = summary;
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_get_summary(
    this: *mut Stack,
    version: StackVersion,
) -> *mut StackSummary {
    unsafe { (*(*this).heads.get(version)).summary }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_dynamic_precedence(
    this: *mut Stack,
    version: StackVersion,
) -> c_int {
    unsafe { (*(*(*this).heads.get(version)).node).dynamic_precedence }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_has_advanced_since_error(
    this: *const Stack,
    version: StackVersion,
) -> bool {
    unsafe {
        let head = (*this).heads.get(version);
        let mut node = (*head).node as *const StackNode;
        if (*node).error_cost == 0 {
            return true;
        }
        while !node.is_null() {
            if (*node).link_count > 0 {
                let subtree = (*node).links[0].subtree;
                if !subtree.ptr.is_null() {
                    if ts_subtree_total_bytes(subtree) > 0 {
                        return true;
                    } else if (*node).node_count > (*head).node_count_at_last_error
                        && ts_subtree_error_cost(subtree) == 0
                    {
                        node = (*node).links[0].node;
                        continue;
                    }
                }
            }
            break;
        }
        false
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_remove_version(this: *mut Stack, version: StackVersion) {
    unsafe {
        stack_head_delete(
            (*this).heads.get(version),
            &mut (*this).node_pool,
            (*this).subtree_pool,
        );
        (*this).heads.erase(version);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_renumber_version(
    this: *mut Stack,
    v1: StackVersion,
    v2: StackVersion,
) {
    unsafe {
        if v1 == v2 {
            return;
        }
        ts_assert!(v2 < v1);
        ts_assert!(v1 < (*this).heads.size);
        let source_head = (*this).heads.get(v1);
        let target_head = (*this).heads.get(v2);
        if !(*target_head).summary.is_null() && (*source_head).summary.is_null() {
            (*source_head).summary = (*target_head).summary;
            (*target_head).summary = ptr::null_mut();
        }
        stack_head_delete(target_head, &mut (*this).node_pool, (*this).subtree_pool);
        *target_head = *source_head;
        (*this).heads.erase(v1);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_swap_versions(
    this: *mut Stack,
    v1: StackVersion,
    v2: StackVersion,
) {
    unsafe {
        let temporary_head = *(*this).heads.get(v1);
        *(*this).heads.get(v1) = *(*this).heads.get(v2);
        *(*this).heads.get(v2) = temporary_head;
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_copy_version(
    this: *mut Stack,
    version: StackVersion,
) -> StackVersion {
    unsafe {
        ts_assert!(version < (*this).heads.size);
        let version_head = *(*this).heads.get(version);
        (*this).heads.push(version_head);
        let head = (*this).heads.back();
        stack_node_retain((*head).node);
        if !(*head).last_external_token.ptr.is_null() {
            ts_subtree_retain((*head).last_external_token);
        }
        (*head).summary = ptr::null_mut();
        (*this).heads.size - 1
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_merge(
    this: *mut Stack,
    version1: StackVersion,
    version2: StackVersion,
) -> bool {
    unsafe {
        if !ts_stack_can_merge(this, version1, version2) {
            return false;
        }
        let head1 = (*this).heads.get(version1);
        let head2 = (*this).heads.get(version2);
        for i in 0..(*(*head2).node).link_count as usize {
            stack_node_add_link(
                (*head1).node,
                (*(*head2).node).links[i],
                (*this).subtree_pool,
            );
        }
        if (*(*head1).node).state == ERROR_STATE {
            (*head1).node_count_at_last_error = (*(*head1).node).node_count;
        }
        ts_stack_remove_version(this, version2);
        true
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_can_merge(
    this: *mut Stack,
    version1: StackVersion,
    version2: StackVersion,
) -> bool {
    unsafe {
        let head1 = (*this).heads.get(version1);
        let head2 = (*this).heads.get(version2);
        (*head1).status == StackStatus::Active
            && (*head2).status == StackStatus::Active
            && (*(*head1).node).state == (*(*head2).node).state
            && (*(*head1).node).position.bytes == (*(*head2).node).position.bytes
            && (*(*head1).node).error_cost == (*(*head2).node).error_cost
            && ts_subtree_external_scanner_state_eq(
                (*head1).last_external_token,
                (*head2).last_external_token,
            )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_halt(this: *mut Stack, version: StackVersion) {
    unsafe { (*(*this).heads.get(version)).status = StackStatus::Halted }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_pause(
    this: *mut Stack,
    version: StackVersion,
    lookahead: Subtree,
) {
    unsafe {
        let head = (*this).heads.get(version);
        (*head).status = StackStatus::Paused;
        (*head).lookahead_when_paused = lookahead;
        (*head).node_count_at_last_error = (*(*head).node).node_count;
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_is_active(this: *const Stack, version: StackVersion) -> bool {
    unsafe { (*(*this).heads.get(version)).status == StackStatus::Active }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_is_halted(this: *const Stack, version: StackVersion) -> bool {
    unsafe { (*(*this).heads.get(version)).status == StackStatus::Halted }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_is_paused(this: *const Stack, version: StackVersion) -> bool {
    unsafe { (*(*this).heads.get(version)).status == StackStatus::Paused }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_resume(this: *mut Stack, version: StackVersion) -> Subtree {
    unsafe {
        let head = (*this).heads.get(version);
        ts_assert!((*head).status == StackStatus::Paused);
        let result = (*head).lookahead_when_paused;
        (*head).status = StackStatus::Active;
        (*head).lookahead_when_paused = NULL_SUBTREE;
        result
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_clear(this: *mut Stack) {
    unsafe {
        stack_node_retain((*this).base_node);
        for i in 0..(*this).heads.size {
            stack_head_delete(
                (*this).heads.get(i),
                &mut (*this).node_pool,
                (*this).subtree_pool,
            );
        }
        (*this).heads.clear();
        (*this).heads.push(StackHead {
            node: (*this).base_node,
            summary: ptr::null_mut(),
            node_count_at_last_error: 0,
            status: StackStatus::Active,
            last_external_token: NULL_SUBTREE,
            lookahead_when_paused: NULL_SUBTREE,
        });
    }
}

unsafe extern "C" {
    fn fprintf(f: *mut c_void, fmt: *const c_char, ...) -> c_int;
    fn fputc(c: c_int, f: *mut c_void) -> c_int;
    fn fputs(s: *const c_char, f: *mut c_void) -> c_int;
    #[cfg_attr(target_vendor = "apple", link_name = "__stderrp")]
    static mut stderr: *mut c_void;
}

unsafe fn write_symbol_as_dot_string(
    language: *const TSLanguage,
    f: *mut c_void,
    symbol: TSSymbol,
) {
    unsafe {
        let name = ts_language_symbol_name(language, symbol);
        let mut chr = name;
        while *chr != 0 {
            match *chr as u8 {
                b'"' | b'\\' => {
                    fputc(i32::from(b'\\'), f);
                    fputc(i32::from(*chr as u8), f);
                }
                b'\n' => {
                    fputs(c"\\n".as_ptr(), f);
                }
                b'\t' => {
                    fputs(c"\\t".as_ptr(), f);
                }
                c => {
                    fputc(i32::from(c), f);
                }
            }
            chr = chr.add(1);
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_stack_print_dot_graph(
    this: *mut Stack,
    language: *const TSLanguage,
    mut f: *mut c_void,
) -> bool {
    unsafe {
        (*this).iterators.reserve(32);
        if f.is_null() {
            f = stderr;
        }

        fprintf(f, c"digraph stack {\n".as_ptr());
        fprintf(f, c"rankdir=\"RL\";\n".as_ptr());
        fprintf(f, c"edge [arrowhead=none]\n".as_ptr());

        let mut visited_nodes: Array<*mut StackNode> = Array::new();

        (*this).iterators.clear();
        for i in 0..(*this).heads.size {
            let head = (*this).heads.get(i);
            if (*head).status == StackStatus::Halted {
                continue;
            }

            fprintf(f, c"node_head_%u [shape=none, label=\"\"]\n".as_ptr(), i);
            fprintf(
                f,
                c"node_head_%u -> node_%p [".as_ptr(),
                i,
                (*head).node as *const c_void,
            );

            if (*head).status == StackStatus::Paused {
                fprintf(f, c"color=red ".as_ptr());
            }
            fprintf(
                f,
                c"label=%u, fontcolor=blue, weight=10000, labeltooltip=\"node_count: %u\nerror_cost: %u".as_ptr(),
                i,
                ts_stack_node_count_since_error(this, i),
                ts_stack_error_cost(this, i),
            );

            if !(*head).summary.is_null() {
                fprintf(f, c"\nsummary:".as_ptr());
                for j in 0..(*(*head).summary).size {
                    fprintf(
                        f,
                        c" %u".as_ptr(),
                        u32::from((*(*(*head).summary).get(j)).state),
                    );
                }
            }

            if !(*head).last_external_token.ptr.is_null() {
                let state =
                    ptr::addr_of!((*(*head).last_external_token.ptr).u.external_scanner_state);
                let data = ts_external_scanner_state_data(state);
                fprintf(f, c"\nexternal_scanner_state:".as_ptr());
                for j in 0..(*state).length {
                    fprintf(f, c" %2X".as_ptr(), c_int::from(*data.add(j as usize)));
                }
            }

            fprintf(f, c"\"]\n".as_ptr());
            (*this).iterators.push(StackIterator {
                node: (*head).node,
                subtrees: Array::new(),
                subtree_count: 0,
                is_pending: false,
            });
        }

        let mut all_iterators_done = false;
        while !all_iterators_done {
            all_iterators_done = true;

            let mut i = 0;
            while i < (*this).iterators.size {
                let iterator = *(*this).iterators.get(i);
                let mut node = iterator.node;

                for j in 0..visited_nodes.size {
                    if *visited_nodes.get(j) == node {
                        node = ptr::null_mut();
                        break;
                    }
                }

                if node.is_null() {
                    i += 1;
                    continue;
                }
                all_iterators_done = false;

                fprintf(f, c"node_%p [".as_ptr(), node as *const c_void);
                if (*node).state == ERROR_STATE {
                    fprintf(f, c"label=\"?\"".as_ptr());
                } else if (*node).link_count == 1
                    && !(*node).links[0].subtree.ptr.is_null()
                    && ts_subtree_extra((*node).links[0].subtree)
                {
                    fprintf(f, c"shape=point margin=0 label=\"\"".as_ptr());
                } else {
                    fprintf(f, c"label=\"%d\"".as_ptr(), c_int::from((*node).state));
                }

                fprintf(
                    f,
                    c" tooltip=\"position: %u,%u\nnode_count:%u\nerror_cost: %u\ndynamic_precedence: %d\"];\n".as_ptr(),
                    (*node).position.extent.row.wrapping_add(1),
                    (*node).position.extent.column,
                    (*node).node_count,
                    (*node).error_cost,
                    (*node).dynamic_precedence,
                );

                for j in 0..(*node).link_count as usize {
                    let link = (*node).links[j];
                    fprintf(
                        f,
                        c"node_%p -> node_%p [".as_ptr(),
                        node as *const c_void,
                        link.node as *const c_void,
                    );
                    if link.is_pending {
                        fprintf(f, c"style=dashed ".as_ptr());
                    }
                    if !link.subtree.ptr.is_null() && ts_subtree_extra(link.subtree) {
                        fprintf(f, c"fontcolor=gray ".as_ptr());
                    }

                    if link.subtree.ptr.is_null() {
                        fprintf(f, c"color=red".as_ptr());
                    } else {
                        fprintf(f, c"label=\"".as_ptr());
                        let quoted =
                            ts_subtree_visible(link.subtree) && !ts_subtree_named(link.subtree);
                        if quoted {
                            fprintf(f, c"'".as_ptr());
                        }
                        write_symbol_as_dot_string(language, f, ts_subtree_symbol(link.subtree));
                        if quoted {
                            fprintf(f, c"'".as_ptr());
                        }
                        fprintf(f, c"\"".as_ptr());
                        fprintf(
                            f,
                            c"labeltooltip=\"error_cost: %u\ndynamic_precedence: %d\"".as_ptr(),
                            ts_subtree_error_cost(link.subtree),
                            ts_subtree_dynamic_precedence(link.subtree),
                        );
                    }

                    fprintf(f, c"];\n".as_ptr());

                    let next_iterator: *mut StackIterator = if j == 0 {
                        (*this).iterators.get(i)
                    } else {
                        (*this).iterators.push(iterator);
                        (*this).iterators.back()
                    };
                    (*next_iterator).node = link.node;
                }

                visited_nodes.push(node);
                i += 1;
            }
        }

        fprintf(f, c"}\n".as_ptr());

        visited_nodes.delete();
        true
    }
}
