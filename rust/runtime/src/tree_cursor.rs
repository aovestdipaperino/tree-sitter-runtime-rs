//! Port of `src/tree_cursor.c` / `src/tree_cursor.h`.
//!
//! `TSTreeCursor` (public) is an opaque 32-byte blob that holds a `TreeCursor`; the stack of
//! `TreeCursorEntry` points into the tree's own subtrees, so entries stay valid as long as the tree.
#![allow(clippy::missing_safety_doc)]
use crate::api::*;
use crate::array::Array;
use crate::language::*;
use crate::length::*;
use crate::parser_abi::TSFieldMapEntry;
use crate::point::point_gt;
use crate::subtree::*;
use crate::tree::TSTree;
use core::ffi::c_char;
use core::ptr;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TreeCursorEntry {
    pub subtree: *const Subtree,
    pub position: Length,
    pub child_index: u32,
    pub structural_child_index: u32,
    pub descendant_index: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TreeCursor {
    pub tree: *const TSTree,
    pub stack: Array<TreeCursorEntry>,
    pub root_alias_symbol: TSSymbol,
}

pub type TreeCursorStep = u32;
pub const TREE_CURSOR_STEP_NONE: TreeCursorStep = 0;
pub const TREE_CURSOR_STEP_HIDDEN: TreeCursorStep = 1;
pub const TREE_CURSOR_STEP_VISIBLE: TreeCursorStep = 2;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CursorChildIterator {
    parent: Subtree,
    tree: *const TSTree,
    position: Length,
    child_index: u32,
    structural_child_index: u32,
    descendant_index: u32,
    alias_sequence: *const TSSymbol,
}

unsafe extern "C" {
    // node.c
    fn ts_node_new(
        tree: *const TSTree,
        subtree: *const Subtree,
        position: Length,
        alias: TSSymbol,
    ) -> TSNode;
    fn ts_node_start_byte(node: TSNode) -> u32;
    fn ts_node_start_point(node: TSNode) -> TSPoint;
}

unsafe fn is_entry_visible(this: *const TreeCursor, index: u32) -> bool {
    unsafe {
        let entry = (*this).stack.get(index);
        if index == 0 || ts_subtree_visible(*(*entry).subtree) {
            true
        } else if !ts_subtree_extra(*(*entry).subtree) {
            let parent_entry = (*this).stack.get(index - 1);
            ts_language_alias_at(
                (*(*this).tree).language,
                u32::from((*(*(*parent_entry).subtree).ptr).u.node.production_id),
                (*entry).structural_child_index,
            ) != 0
        } else {
            false
        }
    }
}

unsafe fn iterate_children(this: *const TreeCursor) -> CursorChildIterator {
    unsafe {
        let last_entry = (*this).stack.back();
        if ts_subtree_child_count(*(*last_entry).subtree) == 0 {
            return CursorChildIterator {
                parent: NULL_SUBTREE,
                tree: (*this).tree,
                position: length_zero(),
                child_index: 0,
                structural_child_index: 0,
                descendant_index: 0,
                alias_sequence: ptr::null(),
            };
        }
        let alias_sequence = ts_language_alias_sequence(
            (*(*this).tree).language,
            u32::from((*(*(*last_entry).subtree).ptr).u.node.production_id),
        );

        let mut descendant_index = (*last_entry).descendant_index;
        if is_entry_visible(this, (*this).stack.size - 1) {
            descendant_index += 1;
        }

        CursorChildIterator {
            tree: (*this).tree,
            parent: *(*last_entry).subtree,
            position: (*last_entry).position,
            child_index: 0,
            structural_child_index: 0,
            descendant_index,
            alias_sequence,
        }
    }
}

unsafe extern "C" fn child_iterator_next(
    this: *mut CursorChildIterator,
    result: *mut TreeCursorEntry,
    visible: *mut bool,
) -> bool {
    unsafe {
        let s = &mut *this;
        if s.parent.ptr.is_null() || s.child_index == (*s.parent.ptr).child_count {
            return false;
        }
        let child = ts_subtree_children(s.parent).add(s.child_index as usize);
        *result = TreeCursorEntry {
            subtree: child,
            position: s.position,
            child_index: s.child_index,
            structural_child_index: s.structural_child_index,
            descendant_index: s.descendant_index,
        };
        *visible = ts_subtree_visible(*child);
        let extra = ts_subtree_extra(*child);
        if !extra {
            if !s.alias_sequence.is_null() {
                *visible |= *s.alias_sequence.add(s.structural_child_index as usize) != 0;
            }
            s.structural_child_index += 1;
        }

        s.descendant_index = s
            .descendant_index
            .wrapping_add(ts_subtree_visible_descendant_count(*child));
        if *visible {
            s.descendant_index = s.descendant_index.wrapping_add(1);
        }

        s.position = length_add(s.position, ts_subtree_size(*child));
        s.child_index += 1;

        if s.child_index < (*s.parent.ptr).child_count {
            let next_child = *ts_subtree_children(s.parent).add(s.child_index as usize);
            s.position = length_add(s.position, ts_subtree_padding(next_child));
        }

        true
    }
}

/// A position that, when `b` is added to it, yields `a`; only possible if `b` has zero rows.
fn length_backtrack(a: Length, b: Length) -> Length {
    if length_is_undefined(a) || b.extent.row != 0 {
        return LENGTH_UNDEFINED;
    }
    Length {
        bytes: a.bytes.wrapping_sub(b.bytes),
        extent: TSPoint {
            row: a.extent.row,
            column: a.extent.column.wrapping_sub(b.extent.column),
        },
    }
}

/// A reverse `child_iterator_next`, taking unsigned underflow into account.
///
/// Two upstream 0.26.13 bugs are fixed here, as upstream did later (ADR-013): the underflow test
/// compared `(int8_t)child_index` with -1, ending the walk at child 255, 511, ... (upstream
/// 9a15b6afa), and the returned entries lacked `descendant_index`, which was never rewound
/// (upstream 2cca12dac).
unsafe extern "C" fn child_iterator_previous(
    this: *mut CursorChildIterator,
    result: *mut TreeCursorEntry,
    visible: *mut bool,
) -> bool {
    unsafe {
        let s = &mut *this;
        if s.parent.ptr.is_null() || s.child_index == u32::MAX {
            return false;
        }
        let child = ts_subtree_children(s.parent).add(s.child_index as usize);
        *result = TreeCursorEntry {
            subtree: child,
            position: s.position,
            child_index: s.child_index,
            structural_child_index: s.structural_child_index,
            descendant_index: s.descendant_index,
        };
        *visible = ts_subtree_visible(*child);
        let extra = ts_subtree_extra(*child);

        s.position = length_backtrack(s.position, ts_subtree_padding(*child));
        s.child_index = s.child_index.wrapping_sub(1);

        if !extra && !s.alias_sequence.is_null() {
            *visible |= *s.alias_sequence.add(s.structural_child_index as usize) != 0;
            if s.structural_child_index > 0 {
                s.structural_child_index -= 1;
            }
        }

        // Unsigned can underflow, so compare with child_count.
        if s.child_index < (*s.parent.ptr).child_count {
            let previous_child = *ts_subtree_children(s.parent).add(s.child_index as usize);
            let mut previous_visible = ts_subtree_visible(previous_child);
            if !ts_subtree_extra(previous_child) && !s.alias_sequence.is_null() {
                previous_visible |= *s.alias_sequence.add(s.structural_child_index as usize) != 0;
            }
            s.descendant_index = s
                .descendant_index
                .wrapping_sub(ts_subtree_visible_descendant_count(previous_child));
            if previous_visible {
                s.descendant_index = s.descendant_index.wrapping_sub(1);
            }
            let size = ts_subtree_size(previous_child);
            s.position = length_backtrack(s.position, size);
        }

        true
    }
}

// ---- lifecycle ----

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_cursor_new(node: TSNode) -> TSTreeCursor {
    unsafe {
        let mut cursor: TreeCursor = core::mem::zeroed();
        ts_tree_cursor_init(&mut cursor, node);
        let mut this: TSTreeCursor = core::mem::zeroed();
        const { assert!(size_of::<TreeCursor>() <= size_of::<TSTreeCursor>()) };
        ptr::copy_nonoverlapping(
            &cursor as *const TreeCursor as *const u8,
            &mut this as *mut TSTreeCursor as *mut u8,
            size_of::<TreeCursor>(),
        );
        this
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_cursor_reset(this: *mut TSTreeCursor, node: TSNode) {
    unsafe { ts_tree_cursor_init(this as *mut TreeCursor, node) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_cursor_init(this: *mut TreeCursor, node: TSNode) {
    unsafe {
        (*this).tree = node.tree;
        (*this).root_alias_symbol = node.context[3] as TSSymbol;
        (*this).stack.clear();
        (*this).stack.push(TreeCursorEntry {
            subtree: node.id as *const Subtree,
            position: Length {
                bytes: ts_node_start_byte(node),
                extent: ts_node_start_point(node),
            },
            child_index: 0,
            structural_child_index: 0,
            descendant_index: 0,
        });
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_cursor_delete(this: *mut TSTreeCursor) {
    unsafe { (*(this as *mut TreeCursor)).stack.delete() }
}

// ---- walking ----

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_cursor_goto_first_child_internal(
    this: *mut TSTreeCursor,
) -> TreeCursorStep {
    unsafe {
        let this = this as *mut TreeCursor;
        let mut visible = false;
        let mut entry: TreeCursorEntry = core::mem::zeroed();
        let mut iterator = iterate_children(this);
        while child_iterator_next(&mut iterator, &mut entry, &mut visible) {
            if visible {
                (*this).stack.push(entry);
                return TREE_CURSOR_STEP_VISIBLE;
            }
            if ts_subtree_visible_child_count(*entry.subtree) > 0 {
                (*this).stack.push(entry);
                return TREE_CURSOR_STEP_HIDDEN;
            }
        }
        TREE_CURSOR_STEP_NONE
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_cursor_goto_first_child(this: *mut TSTreeCursor) -> bool {
    unsafe {
        loop {
            match ts_tree_cursor_goto_first_child_internal(this) {
                TREE_CURSOR_STEP_HIDDEN => continue,
                TREE_CURSOR_STEP_VISIBLE => return true,
                _ => return false,
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_cursor_goto_last_child_internal(
    this: *mut TSTreeCursor,
) -> TreeCursorStep {
    unsafe {
        let this = this as *mut TreeCursor;
        let mut visible = false;
        let mut entry: TreeCursorEntry = core::mem::zeroed();
        let mut iterator = iterate_children(this);
        if iterator.parent.ptr.is_null() || (*iterator.parent.ptr).child_count == 0 {
            return TREE_CURSOR_STEP_NONE;
        }

        let mut last_entry: TreeCursorEntry = core::mem::zeroed();
        let mut last_step = TREE_CURSOR_STEP_NONE;
        while child_iterator_next(&mut iterator, &mut entry, &mut visible) {
            if visible {
                last_entry = entry;
                last_step = TREE_CURSOR_STEP_VISIBLE;
            } else if ts_subtree_visible_child_count(*entry.subtree) > 0 {
                last_entry = entry;
                last_step = TREE_CURSOR_STEP_HIDDEN;
            }
        }
        if !last_entry.subtree.is_null() {
            (*this).stack.push(last_entry);
            return last_step;
        }

        TREE_CURSOR_STEP_NONE
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_cursor_goto_last_child(this: *mut TSTreeCursor) -> bool {
    unsafe {
        loop {
            match ts_tree_cursor_goto_last_child_internal(this) {
                TREE_CURSOR_STEP_HIDDEN => continue,
                TREE_CURSOR_STEP_VISIBLE => return true,
                _ => return false,
            }
        }
    }
}

unsafe fn goto_first_child_for_byte_and_point(
    this: *mut TSTreeCursor,
    goal_byte: u32,
    goal_point: TSPoint,
) -> i64 {
    unsafe {
        let this = this as *mut TreeCursor;
        let initial_size = (*this).stack.size;
        let mut visible_child_index = 0u32;

        loop {
            let mut did_descend = false;

            let mut visible = false;
            let mut entry: TreeCursorEntry = core::mem::zeroed();
            let mut iterator = iterate_children(this);
            while child_iterator_next(&mut iterator, &mut entry, &mut visible) {
                let entry_end = length_add(entry.position, ts_subtree_size(*entry.subtree));
                let at_goal = entry_end.bytes > goal_byte && point_gt(entry_end.extent, goal_point);
                let visible_child_count = ts_subtree_visible_child_count(*entry.subtree);
                if at_goal {
                    if visible {
                        (*this).stack.push(entry);
                        return i64::from(visible_child_index);
                    }
                    if visible_child_count > 0 {
                        (*this).stack.push(entry);
                        did_descend = true;
                        break;
                    }
                } else if visible {
                    visible_child_index = visible_child_index.wrapping_add(1);
                } else {
                    visible_child_index = visible_child_index.wrapping_add(visible_child_count);
                }
            }
            if !did_descend {
                break;
            }
        }

        (*this).stack.size = initial_size;
        -1
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_cursor_goto_first_child_for_byte(
    this: *mut TSTreeCursor,
    goal_byte: u32,
) -> i64 {
    unsafe { goto_first_child_for_byte_and_point(this, goal_byte, TSPoint { row: 0, column: 0 }) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_cursor_goto_first_child_for_point(
    this: *mut TSTreeCursor,
    goal_point: TSPoint,
) -> i64 {
    unsafe { goto_first_child_for_byte_and_point(this, 0, goal_point) }
}

type AdvanceFn =
    unsafe extern "C" fn(*mut CursorChildIterator, *mut TreeCursorEntry, *mut bool) -> bool;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_cursor_goto_sibling_internal(
    this: *mut TSTreeCursor,
    advance: AdvanceFn,
) -> TreeCursorStep {
    unsafe {
        let this = this as *mut TreeCursor;
        let initial_size = (*this).stack.size;

        while (*this).stack.size > 1 {
            let mut entry = (*this).stack.pop();
            let mut iterator = iterate_children(this);
            iterator.child_index = entry.child_index;
            iterator.structural_child_index = entry.structural_child_index;
            iterator.position = entry.position;
            iterator.descendant_index = entry.descendant_index;

            let mut visible = false;
            advance(&mut iterator, &mut entry, &mut visible);
            if visible && (*this).stack.size + 1 < initial_size {
                break;
            }

            while advance(&mut iterator, &mut entry, &mut visible) {
                if visible {
                    (*this).stack.push(entry);
                    return TREE_CURSOR_STEP_VISIBLE;
                }

                if ts_subtree_visible_child_count(*entry.subtree) != 0 {
                    (*this).stack.push(entry);
                    return TREE_CURSOR_STEP_HIDDEN;
                }
            }
        }

        (*this).stack.size = initial_size;
        TREE_CURSOR_STEP_NONE
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_cursor_goto_next_sibling_internal(
    this: *mut TSTreeCursor,
) -> TreeCursorStep {
    unsafe { ts_tree_cursor_goto_sibling_internal(this, child_iterator_next) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_cursor_goto_next_sibling(this: *mut TSTreeCursor) -> bool {
    unsafe {
        match ts_tree_cursor_goto_next_sibling_internal(this) {
            TREE_CURSOR_STEP_HIDDEN => {
                ts_tree_cursor_goto_first_child(this);
                true
            }
            TREE_CURSOR_STEP_VISIBLE => true,
            _ => false,
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_cursor_goto_previous_sibling_internal(
    this: *mut TSTreeCursor,
) -> TreeCursorStep {
    unsafe {
        // Subtracting across a row loses column information, so the position may have to be restored.
        let cursor = this as *mut TreeCursor;

        let step = ts_tree_cursor_goto_sibling_internal(this, child_iterator_previous);
        if step == TREE_CURSOR_STEP_NONE {
            return step;
        }

        // If the length is already valid there is no need to recompute it.
        if !length_is_undefined((*(*cursor).stack.back()).position) {
            return step;
        }

        // Restore the position from the parent node.
        let parent = (*cursor).stack.get((*cursor).stack.size - 2);
        let mut position = (*parent).position;
        let child_index = (*(*cursor).stack.back()).child_index;
        let children = ts_subtree_children(*(*parent).subtree);

        if child_index > 0 {
            // Skip the first child's padding: its position matches the parent's.
            position = length_add(position, ts_subtree_size(*children));
            for i in 1..child_index {
                position = length_add(position, ts_subtree_total_size(*children.add(i as usize)));
            }
            position = length_add(
                position,
                ts_subtree_padding(*children.add(child_index as usize)),
            );
        }

        (*(*cursor).stack.back()).position = position;

        step
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_cursor_goto_previous_sibling(this: *mut TSTreeCursor) -> bool {
    unsafe {
        match ts_tree_cursor_goto_previous_sibling_internal(this) {
            TREE_CURSOR_STEP_HIDDEN => {
                ts_tree_cursor_goto_last_child(this);
                true
            }
            TREE_CURSOR_STEP_VISIBLE => true,
            _ => false,
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_cursor_goto_parent(this: *mut TSTreeCursor) -> bool {
    unsafe {
        let this = this as *mut TreeCursor;
        let mut i = (*this).stack.size.wrapping_sub(2);
        while i.wrapping_add(1) > 0 {
            if is_entry_visible(this, i) {
                (*this).stack.size = i + 1;
                return true;
            }
            i = i.wrapping_sub(1);
        }
        false
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_cursor_goto_descendant(
    this: *mut TSTreeCursor,
    goal_descendant_index: u32,
) {
    unsafe {
        let this = this as *mut TreeCursor;

        // Ascend to the lowest ancestor that contains the goal node.
        loop {
            let i = (*this).stack.size - 1;
            let entry = (*this).stack.get(i);
            let next_descendant_index = (*entry)
                .descendant_index
                .wrapping_add(u32::from(is_entry_visible(this, i)))
                .wrapping_add(ts_subtree_visible_descendant_count(*(*entry).subtree));
            if (*entry).descendant_index <= goal_descendant_index
                && next_descendant_index > goal_descendant_index
            {
                break;
            } else if (*this).stack.size <= 1 {
                return;
            } else {
                (*this).stack.size -= 1;
            }
        }

        // Descend to the goal node.
        loop {
            let mut did_descend = false;
            let mut visible = false;
            let mut entry: TreeCursorEntry = core::mem::zeroed();
            let mut iterator = iterate_children(this);
            if iterator.descendant_index > goal_descendant_index {
                return;
            }

            while child_iterator_next(&mut iterator, &mut entry, &mut visible) {
                if iterator.descendant_index > goal_descendant_index {
                    (*this).stack.push(entry);
                    if visible && entry.descendant_index == goal_descendant_index {
                        return;
                    } else {
                        did_descend = true;
                        break;
                    }
                }
            }
            if !did_descend {
                break;
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_cursor_current_descendant_index(this: *const TSTreeCursor) -> u32 {
    unsafe { (*(*(this as *const TreeCursor)).stack.back()).descendant_index }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_cursor_current_node(this: *const TSTreeCursor) -> TSNode {
    unsafe {
        let this = this as *const TreeCursor;
        let last_entry = (*this).stack.back();
        let is_extra = ts_subtree_extra(*(*last_entry).subtree);
        let mut alias_symbol = if is_extra {
            0
        } else {
            (*this).root_alias_symbol
        };
        if (*this).stack.size > 1 && !is_extra {
            let parent_entry = (*this).stack.get((*this).stack.size - 2);
            alias_symbol = ts_language_alias_at(
                (*(*this).tree).language,
                u32::from((*(*(*parent_entry).subtree).ptr).u.node.production_id),
                (*last_entry).structural_child_index,
            );
        }
        ts_node_new(
            (*this).tree,
            (*last_entry).subtree,
            (*last_entry).position,
            alias_symbol,
        )
    }
}

/// Facts about the current node needed when executing queries.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_cursor_current_status(
    this: *const TSTreeCursor,
    field_id: *mut TSFieldId,
    has_later_siblings: *mut bool,
    has_later_named_siblings: *mut bool,
    can_have_later_siblings_with_this_field: *mut bool,
    supertypes: *mut TSSymbol,
    supertype_count: *mut u32,
) {
    unsafe {
        let this = this as *const TreeCursor;
        let max_supertypes = *supertype_count;
        *field_id = 0;
        *supertype_count = 0;
        *has_later_siblings = false;
        *has_later_named_siblings = false;
        *can_have_later_siblings_with_this_field = false;

        // Walk up through the current node and its invisible ancestors: fields can refer to nodes
        // through invisible wrapper nodes.
        let mut i = (*this).stack.size - 1;
        while i > 0 {
            let entry = (*this).stack.get(i);
            let parent_entry = (*this).stack.get(i - 1);

            let alias_sequence = ts_language_alias_sequence(
                (*(*this).tree).language,
                u32::from((*(*(*parent_entry).subtree).ptr).u.node.production_id),
            );

            let subtree_symbol = |subtree: Subtree, structural_child_index: u32| -> TSSymbol {
                if !ts_subtree_extra(subtree)
                    && !alias_sequence.is_null()
                    && *alias_sequence.add(structural_child_index as usize) != 0
                {
                    *alias_sequence.add(structural_child_index as usize)
                } else {
                    ts_subtree_symbol(subtree)
                }
            };

            // Stop walking up when a visible ancestor is found.
            let entry_symbol = subtree_symbol(*(*entry).subtree, (*entry).structural_child_index);
            let entry_metadata =
                ts_language_symbol_metadata((*(*this).tree).language, entry_symbol);
            if i != (*this).stack.size - 1 && entry_metadata.visible {
                break;
            }

            // Record any supertypes.
            if entry_metadata.supertype && *supertype_count < max_supertypes {
                *supertypes.add(*supertype_count as usize) = entry_symbol;
                *supertype_count += 1;
            }

            // Does the current node have later siblings? A later anonymous sibling settles
            // `has_later_siblings` but says nothing about later named siblings.
            if !*has_later_named_siblings {
                let sibling_count = (*(*(*parent_entry).subtree).ptr).child_count;
                let mut structural_child_index = (*entry).structural_child_index;
                if !ts_subtree_extra(*(*entry).subtree) {
                    structural_child_index += 1;
                }
                let mut j = (*entry).child_index + 1;
                while j < sibling_count {
                    let sibling = *ts_subtree_children(*(*parent_entry).subtree).add(j as usize);
                    let sibling_metadata = ts_language_symbol_metadata(
                        (*(*this).tree).language,
                        subtree_symbol(sibling, structural_child_index),
                    );
                    if sibling_metadata.visible {
                        *has_later_siblings = true;
                        if sibling_metadata.named {
                            *has_later_named_siblings = true;
                            break;
                        }
                    } else if ts_subtree_visible_child_count(sibling) > 0 {
                        *has_later_siblings = true;
                        if (*sibling.ptr).u.node.named_child_count > 0 {
                            *has_later_named_siblings = true;
                            break;
                        }
                    }
                    if !ts_subtree_extra(sibling) {
                        structural_child_index += 1;
                    }
                    j += 1;
                }
            }

            if !ts_subtree_extra(*(*entry).subtree) {
                let mut field_map: *const TSFieldMapEntry = ptr::null();
                let mut field_map_end: *const TSFieldMapEntry = ptr::null();
                ts_language_field_map(
                    (*(*this).tree).language,
                    u32::from((*(*(*parent_entry).subtree).ptr).u.node.production_id),
                    &mut field_map,
                    &mut field_map_end,
                );

                // Look for a field name associated with the current node.
                if *field_id == 0 {
                    let mut map = field_map;
                    while map < field_map_end {
                        if !(*map).inherited
                            && u32::from((*map).child_index) == (*entry).structural_child_index
                        {
                            *field_id = (*map).field_id;
                            break;
                        }
                        map = map.add(1);
                    }
                }

                // Can the current node have later siblings with the same field name?
                if *field_id != 0 {
                    let mut map = field_map;
                    while map < field_map_end {
                        if (*map).field_id == *field_id
                            && u32::from((*map).child_index) > (*entry).structural_child_index
                        {
                            *can_have_later_siblings_with_this_field = true;
                            break;
                        }
                        map = map.add(1);
                    }
                }
            }
            i -= 1;
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_cursor_current_depth(this: *const TSTreeCursor) -> u32 {
    unsafe {
        let this = this as *const TreeCursor;
        let mut depth = 0;
        for i in 1..(*this).stack.size {
            if is_entry_visible(this, i) {
                depth += 1;
            }
        }
        depth
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_cursor_parent_node(this: *const TSTreeCursor) -> TSNode {
    unsafe {
        let this = this as *const TreeCursor;
        let mut i = (*this).stack.size as i32 - 2;
        while i >= 0 {
            let entry = (*this).stack.get(i as u32);
            let mut is_visible = true;
            let mut alias_symbol = 0;
            if i > 0 {
                let parent_entry = (*this).stack.get(i as u32 - 1);
                alias_symbol = ts_language_alias_at(
                    (*(*this).tree).language,
                    u32::from((*(*(*parent_entry).subtree).ptr).u.node.production_id),
                    (*entry).structural_child_index,
                );
                is_visible = alias_symbol != 0 || ts_subtree_visible(*(*entry).subtree);
            }
            if is_visible {
                return ts_node_new(
                    (*this).tree,
                    (*entry).subtree,
                    (*entry).position,
                    alias_symbol,
                );
            }
            i -= 1;
        }
        ts_node_new(ptr::null(), ptr::null(), length_zero(), 0)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_cursor_current_field_id(this: *const TSTreeCursor) -> TSFieldId {
    unsafe {
        let this = this as *const TreeCursor;

        // Walk up through the current node and its invisible ancestors.
        let mut i = (*this).stack.size - 1;
        while i > 0 {
            let entry = (*this).stack.get(i);
            let parent_entry = (*this).stack.get(i - 1);

            // Stop walking up when another visible node is found.
            if i != (*this).stack.size - 1 && is_entry_visible(this, i) {
                break;
            }

            if ts_subtree_extra(*(*entry).subtree) {
                break;
            }

            let mut field_map: *const TSFieldMapEntry = ptr::null();
            let mut field_map_end: *const TSFieldMapEntry = ptr::null();
            ts_language_field_map(
                (*(*this).tree).language,
                u32::from((*(*(*parent_entry).subtree).ptr).u.node.production_id),
                &mut field_map,
                &mut field_map_end,
            );
            let mut map = field_map;
            while map < field_map_end {
                if !(*map).inherited
                    && u32::from((*map).child_index) == (*entry).structural_child_index
                {
                    return (*map).field_id;
                }
                map = map.add(1);
            }
            i -= 1;
        }
        0
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_cursor_current_field_name(
    this: *const TSTreeCursor,
) -> *const c_char {
    unsafe {
        let id = ts_tree_cursor_current_field_id(this);
        if id != 0 {
            let this = this as *const TreeCursor;
            *(*(*(*this).tree).language).field_names.add(usize::from(id))
        } else {
            ptr::null()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_cursor_copy(cursor: *const TSTreeCursor) -> TSTreeCursor {
    unsafe {
        let cursor = cursor as *const TreeCursor;
        let mut copy: TreeCursor = core::mem::zeroed();
        copy.tree = (*cursor).tree;
        copy.root_alias_symbol = (*cursor).root_alias_symbol;
        copy.stack = Array::new();
        copy.stack.push_all(&(*cursor).stack);
        let mut res: TSTreeCursor = core::mem::zeroed();
        ptr::copy_nonoverlapping(
            &copy as *const TreeCursor as *const u8,
            &mut res as *mut TSTreeCursor as *mut u8,
            size_of::<TreeCursor>(),
        );
        res
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_cursor_reset_to(dst: *mut TSTreeCursor, src: *const TSTreeCursor) {
    unsafe {
        let cursor = src as *const TreeCursor;
        let copy = dst as *mut TreeCursor;
        (*copy).tree = (*cursor).tree;
        (*copy).root_alias_symbol = (*cursor).root_alias_symbol;
        (*copy).stack.clear();
        (*copy).stack.push_all(&(*cursor).stack);
    }
}
