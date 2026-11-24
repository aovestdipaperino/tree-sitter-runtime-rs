//! Port of `src/node.c`: the `TSNode` accessors and navigation.
//!
//! A `TSNode` is `{context: [start_byte, start_row, start_column, alias], id: *const Subtree,
//! tree}`. Function names follow the C (`ts_node__x` becomes `node_x`) and the control flow is kept
//! as is, including the `goto` in `first_child_for_byte`, so results are identical.
#![allow(clippy::missing_safety_doc)]
use crate::api::*;
use crate::language::*;
use crate::length::*;
use crate::parser_abi::*;
use crate::point::*;
use crate::subtree::*;
use crate::tree::{TSTree, ts_tree_language, ts_tree_root_node};
use core::ffi::c_char;
use core::ptr;

#[derive(Clone, Copy)]
struct NodeChildIterator {
    parent: Subtree,
    tree: *const TSTree,
    position: Length,
    child_index: u32,
    structural_child_index: u32,
    alias_sequence: *const TSSymbol,
}

// ---- TSNode - constructors ----

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_new(
    tree: *const TSTree,
    subtree: *const Subtree,
    position: Length,
    alias: TSSymbol,
) -> TSNode {
    TSNode {
        context: [
            position.bytes,
            position.extent.row,
            position.extent.column,
            u32::from(alias),
        ],
        id: subtree as *const core::ffi::c_void,
        tree,
    }
}

fn node_null() -> TSNode {
    // SAFETY: `ts_node_new` only builds a value.
    unsafe { ts_node_new(ptr::null(), ptr::null(), length_zero(), 0) }
}

// ---- TSNode - accessors ----

#[unsafe(no_mangle)]
pub extern "C" fn ts_node_start_byte(this: TSNode) -> u32 {
    this.context[0]
}

#[unsafe(no_mangle)]
pub extern "C" fn ts_node_start_point(this: TSNode) -> TSPoint {
    TSPoint {
        row: this.context[1],
        column: this.context[2],
    }
}

#[inline]
fn node_alias(this: &TSNode) -> u32 {
    this.context[3]
}

#[inline]
unsafe fn node_subtree(this: TSNode) -> Subtree {
    // SAFETY: `id` of a non-null node points at a live `Subtree` of its tree.
    unsafe { *(this.id as *const Subtree) }
}

#[inline]
unsafe fn tree_language(this: &TSNode) -> *const TSLanguage {
    unsafe { (*this.tree).language }
}

// ---- NodeChildIterator ----

unsafe fn node_iterate_children(node: *const TSNode) -> NodeChildIterator {
    unsafe {
        let subtree = node_subtree(*node);
        if ts_subtree_child_count(subtree) == 0 {
            return NodeChildIterator {
                parent: NULL_SUBTREE,
                tree: (*node).tree,
                position: length_zero(),
                child_index: 0,
                structural_child_index: 0,
                alias_sequence: ptr::null(),
            };
        }
        let alias_sequence = ts_language_alias_sequence(
            tree_language(&*node),
            u32::from((*subtree.ptr).u.node.production_id),
        );
        NodeChildIterator {
            tree: (*node).tree,
            parent: subtree,
            position: Length {
                bytes: ts_node_start_byte(*node),
                extent: ts_node_start_point(*node),
            },
            child_index: 0,
            structural_child_index: 0,
            alias_sequence,
        }
    }
}

unsafe fn node_child_iterator_done(this: &NodeChildIterator) -> bool {
    unsafe { this.child_index == (*this.parent.ptr).child_count }
}

unsafe fn node_child_iterator_next(this: &mut NodeChildIterator, result: &mut TSNode) -> bool {
    unsafe {
        if this.parent.ptr.is_null() || node_child_iterator_done(this) {
            return false;
        }
        let child = ts_subtree_children(this.parent).add(this.child_index as usize);
        let mut alias_symbol: TSSymbol = 0;
        if !ts_subtree_extra(*child) {
            if !this.alias_sequence.is_null() {
                alias_symbol = *this
                    .alias_sequence
                    .add(this.structural_child_index as usize);
            }
            this.structural_child_index += 1;
        }
        if this.child_index > 0 {
            this.position = length_add(this.position, ts_subtree_padding(*child));
        }
        *result = ts_node_new(this.tree, child, this.position, alias_symbol);
        this.position = length_add(this.position, ts_subtree_size(*child));
        this.child_index += 1;
        true
    }
}

// ---- TSNode - private ----

unsafe fn node_is_relevant(this: TSNode, include_anonymous: bool) -> bool {
    unsafe {
        let tree = node_subtree(this);
        if include_anonymous {
            ts_subtree_visible(tree) || node_alias(&this) != 0
        } else {
            let alias = node_alias(&this);
            if alias != 0 {
                ts_language_symbol_metadata(tree_language(&this), alias as TSSymbol).named
            } else {
                ts_subtree_visible(tree) && ts_subtree_named(tree)
            }
        }
    }
}

unsafe fn node_relevant_child_count(this: TSNode, include_anonymous: bool) -> u32 {
    unsafe {
        let tree = node_subtree(this);
        if ts_subtree_child_count(tree) > 0 {
            if include_anonymous {
                (*tree.ptr).u.node.visible_child_count
            } else {
                (*tree.ptr).u.node.named_child_count
            }
        } else {
            0
        }
    }
}

unsafe fn node_child(this: TSNode, mut child_index: u32, include_anonymous: bool) -> TSNode {
    unsafe {
        let mut result = this;
        let mut did_descend = true;

        while did_descend {
            did_descend = false;

            let mut child = node_null();
            let mut index = 0u32;
            let mut iterator = node_iterate_children(&result);
            while node_child_iterator_next(&mut iterator, &mut child) {
                if node_is_relevant(child, include_anonymous) {
                    if index == child_index {
                        return child;
                    }
                    index += 1;
                } else {
                    let grandchild_index = child_index.wrapping_sub(index);
                    let grandchild_count = node_relevant_child_count(child, include_anonymous);
                    if grandchild_index < grandchild_count {
                        did_descend = true;
                        result = child;
                        child_index = grandchild_index;
                        break;
                    }
                    index = index.wrapping_add(grandchild_count);
                }
            }
        }

        node_null()
    }
}

unsafe fn subtree_has_trailing_empty_descendant(this: Subtree, other: Subtree) -> bool {
    unsafe {
        let mut i = ts_subtree_child_count(this).wrapping_sub(1);
        while i.wrapping_add(1) > 0 {
            let child = *ts_subtree_children(this).add(i as usize);
            if ts_subtree_total_bytes(child) > 0 {
                break;
            }
            if child.ptr == other.ptr || subtree_has_trailing_empty_descendant(child, other) {
                return true;
            }
            i = i.wrapping_sub(1);
        }
        false
    }
}

unsafe fn node_prev_sibling(this: TSNode, include_anonymous: bool) -> TSNode {
    unsafe {
        let self_subtree = node_subtree(this);
        let self_is_empty = ts_subtree_total_bytes(self_subtree) == 0;
        let target_end_byte = ts_node_end_byte(this);

        let mut node = ts_node_parent(this);
        let mut earlier_node = node_null();
        let mut earlier_node_is_relevant = false;

        while !ts_node_is_null(node) {
            let mut earlier_child = node_null();
            let mut earlier_child_is_relevant = false;
            let mut found_child_containing_target = false;

            let mut child = node_null();
            let mut iterator = node_iterate_children(&node);
            while node_child_iterator_next(&mut iterator, &mut child) {
                if child.id == this.id {
                    break;
                }
                if iterator.position.bytes > target_end_byte {
                    found_child_containing_target = true;
                    break;
                }

                if iterator.position.bytes == target_end_byte
                    && (!self_is_empty
                        || subtree_has_trailing_empty_descendant(node_subtree(child), self_subtree))
                {
                    found_child_containing_target = true;
                    break;
                }

                if node_is_relevant(child, include_anonymous) {
                    earlier_child = child;
                    earlier_child_is_relevant = true;
                } else if node_relevant_child_count(child, include_anonymous) > 0 {
                    earlier_child = child;
                    earlier_child_is_relevant = false;
                }
            }

            if found_child_containing_target {
                if !ts_node_is_null(earlier_child) {
                    earlier_node = earlier_child;
                    earlier_node_is_relevant = earlier_child_is_relevant;
                }
                node = child;
            } else if earlier_child_is_relevant {
                return earlier_child;
            } else if !ts_node_is_null(earlier_child) {
                node = earlier_child;
            } else if earlier_node_is_relevant {
                return earlier_node;
            } else {
                node = earlier_node;
                earlier_node = node_null();
                earlier_node_is_relevant = false;
            }
        }

        node_null()
    }
}

unsafe fn node_next_sibling(this: TSNode, include_anonymous: bool) -> TSNode {
    unsafe {
        let target_end_byte = ts_node_end_byte(this);

        let mut node = ts_node_parent(this);
        let mut later_node = node_null();
        let mut later_node_is_relevant = false;

        while !ts_node_is_null(node) {
            let mut later_child = node_null();
            let mut later_child_is_relevant = false;
            let mut child_containing_target = node_null();

            let mut child = node_null();
            let mut iterator = node_iterate_children(&node);
            while node_child_iterator_next(&mut iterator, &mut child) {
                if iterator.position.bytes <= target_end_byte {
                    continue;
                }
                let start_byte = ts_node_start_byte(this);
                let child_start_byte = ts_node_start_byte(child);

                let is_empty = start_byte == target_end_byte;
                let contains_target = if is_empty {
                    child_start_byte < start_byte
                } else {
                    child_start_byte <= start_byte
                };

                if contains_target {
                    if node_subtree(child).ptr != node_subtree(this).ptr {
                        child_containing_target = child;
                    }
                } else if node_is_relevant(child, include_anonymous) {
                    later_child = child;
                    later_child_is_relevant = true;
                    break;
                } else if node_relevant_child_count(child, include_anonymous) > 0 {
                    later_child = child;
                    later_child_is_relevant = false;
                    break;
                }
            }

            if !ts_node_is_null(child_containing_target) {
                if !ts_node_is_null(later_child) {
                    later_node = later_child;
                    later_node_is_relevant = later_child_is_relevant;
                }
                node = child_containing_target;
            } else if later_child_is_relevant {
                return later_child;
            } else if !ts_node_is_null(later_child) {
                node = later_child;
            } else if later_node_is_relevant {
                return later_node;
            } else {
                node = later_node;
            }
        }

        node_null()
    }
}

unsafe fn node_first_child_for_byte(this: TSNode, goal: u32, include_anonymous: bool) -> TSNode {
    unsafe {
        let mut node = this;
        let mut did_descend = true;

        let mut last_iterator = node_iterate_children(&node);
        let mut has_last_iterator = false;

        while did_descend {
            did_descend = false;

            let mut child = node_null();
            let mut iterator = node_iterate_children(&node);
            loop {
                while node_child_iterator_next(&mut iterator, &mut child) {
                    if ts_node_end_byte(child) > goal {
                        if node_is_relevant(child, include_anonymous) {
                            return child;
                        } else if ts_node_child_count(child) > 0 {
                            if !node_child_iterator_done(&iterator) {
                                last_iterator = iterator;
                                has_last_iterator = true;
                            }
                            did_descend = true;
                            node = child;
                            break;
                        }
                    }
                }

                if !did_descend && has_last_iterator {
                    iterator = last_iterator;
                    has_last_iterator = false;
                    continue;
                }
                break;
            }
        }

        node_null()
    }
}

unsafe fn node_descendant_for_byte_range(
    this: TSNode,
    range_start: u32,
    range_end: u32,
    include_anonymous: bool,
) -> TSNode {
    unsafe {
        if range_start > range_end {
            return node_null();
        }
        let mut node = this;
        let mut last_visible_node = this;

        let mut did_descend = true;
        while did_descend {
            did_descend = false;

            let mut child = node_null();
            let mut iterator = node_iterate_children(&node);
            while node_child_iterator_next(&mut iterator, &mut child) {
                let node_end = iterator.position.bytes;

                // The end of this node must extend far enough forward to touch
                // the end of the range
                if node_end < range_end {
                    continue;
                }

                // ...and exceed the start of the range, unless the node itself is
                // empty, in which case it must at least be equal to the start of the range.
                let is_empty = ts_node_start_byte(child) == node_end;
                if if is_empty {
                    node_end < range_start
                } else {
                    node_end <= range_start
                } {
                    continue;
                }

                // The start of this node must extend far enough backward to
                // touch the start of the range.
                if range_start < ts_node_start_byte(child) {
                    break;
                }

                node = child;
                if node_is_relevant(node, include_anonymous) {
                    last_visible_node = node;
                }
                did_descend = true;
                break;
            }
        }

        last_visible_node
    }
}

unsafe fn node_descendant_for_point_range(
    this: TSNode,
    range_start: TSPoint,
    range_end: TSPoint,
    include_anonymous: bool,
) -> TSNode {
    unsafe {
        if point_gt(range_start, range_end) {
            return node_null();
        }
        let mut node = this;
        let mut last_visible_node = this;

        let mut did_descend = true;
        while did_descend {
            did_descend = false;

            let mut child = node_null();
            let mut iterator = node_iterate_children(&node);
            while node_child_iterator_next(&mut iterator, &mut child) {
                let node_end = iterator.position.extent;

                // The end of this node must extend far enough forward to touch
                // the end of the range
                if point_lt(node_end, range_end) {
                    continue;
                }

                // ...and exceed the start of the range, unless the node itself is
                // empty, in which case it must at least be equal to the start of the range.
                let is_empty = point_eq(ts_node_start_point(child), node_end);
                if if is_empty {
                    point_lt(node_end, range_start)
                } else {
                    point_lte(node_end, range_start)
                } {
                    continue;
                }

                // The start of this node must extend far enough backward to
                // touch the start of the range.
                if point_lt(range_start, ts_node_start_point(child)) {
                    break;
                }

                node = child;
                if node_is_relevant(node, include_anonymous) {
                    last_visible_node = node;
                }
                did_descend = true;
                break;
            }
        }

        last_visible_node
    }
}

// ---- TSNode - public ----

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_end_byte(this: TSNode) -> u32 {
    unsafe { ts_node_start_byte(this).wrapping_add(ts_subtree_size(node_subtree(this)).bytes) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_end_point(this: TSNode) -> TSPoint {
    unsafe {
        point_add(
            ts_node_start_point(this),
            ts_subtree_size(node_subtree(this)).extent,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_symbol(this: TSNode) -> TSSymbol {
    unsafe {
        let mut symbol = node_alias(&this) as TSSymbol;
        if symbol == 0 {
            symbol = ts_subtree_symbol(node_subtree(this));
        }
        ts_language_public_symbol(tree_language(&this), symbol)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_type(this: TSNode) -> *const c_char {
    unsafe {
        let mut symbol = node_alias(&this) as TSSymbol;
        if symbol == 0 {
            symbol = ts_subtree_symbol(node_subtree(this));
        }
        ts_language_symbol_name(tree_language(&this), symbol)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_language(this: TSNode) -> *const TSLanguage {
    unsafe { ts_tree_language(this.tree) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_grammar_symbol(this: TSNode) -> TSSymbol {
    unsafe { ts_subtree_symbol(node_subtree(this)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_grammar_type(this: TSNode) -> *const c_char {
    unsafe {
        let symbol = ts_subtree_symbol(node_subtree(this));
        ts_language_symbol_name(tree_language(&this), symbol)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_string(this: TSNode) -> *mut c_char {
    unsafe {
        let alias_symbol = node_alias(&this) as TSSymbol;
        ts_subtree_string(
            node_subtree(this),
            alias_symbol,
            ts_language_symbol_metadata(tree_language(&this), alias_symbol).visible,
            tree_language(&this),
            false,
        )
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ts_node_eq(this: TSNode, other: TSNode) -> bool {
    this.tree == other.tree && this.id == other.id
}

#[unsafe(no_mangle)]
pub extern "C" fn ts_node_is_null(this: TSNode) -> bool {
    this.id.is_null()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_is_extra(this: TSNode) -> bool {
    unsafe { ts_subtree_extra(node_subtree(this)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_is_named(this: TSNode) -> bool {
    unsafe {
        let alias = node_alias(&this);
        if alias != 0 {
            ts_language_symbol_metadata(tree_language(&this), alias as TSSymbol).named
        } else {
            ts_subtree_named(node_subtree(this))
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_is_missing(this: TSNode) -> bool {
    unsafe { ts_subtree_missing(node_subtree(this)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_has_changes(this: TSNode) -> bool {
    unsafe { ts_subtree_has_changes(node_subtree(this)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_has_error(this: TSNode) -> bool {
    unsafe { ts_node_is_error(this) || ts_subtree_error_cost(node_subtree(this)) > 0 }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_is_error(this: TSNode) -> bool {
    unsafe { ts_node_symbol(this) == ts_builtin_sym_error }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_descendant_count(this: TSNode) -> u32 {
    unsafe { ts_subtree_visible_descendant_count(node_subtree(this)).wrapping_add(1) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_parse_state(this: TSNode) -> TSStateId {
    unsafe { ts_subtree_parse_state(node_subtree(this)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_next_parse_state(this: TSNode) -> TSStateId {
    unsafe {
        let language = tree_language(&this);
        let state = ts_node_parse_state(this);
        if state == TS_TREE_STATE_NONE {
            return TS_TREE_STATE_NONE;
        }
        let symbol = ts_node_grammar_symbol(this);
        ts_language_next_state(language, state, symbol)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_parent(this: TSNode) -> TSNode {
    unsafe {
        let mut node = ts_tree_root_node(this.tree);
        if node.id == this.id {
            return node_null();
        }

        loop {
            let next_node = ts_node_child_with_descendant(node, this);
            if next_node.id == this.id || ts_node_is_null(next_node) {
                break;
            }
            node = next_node;
        }

        node
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_child_with_descendant(
    mut this: TSNode,
    descendant: TSNode,
) -> TSNode {
    unsafe {
        let start_byte = ts_node_start_byte(descendant);
        let end_byte = ts_node_end_byte(descendant);
        let is_empty = start_byte == end_byte;

        loop {
            let mut iter = node_iterate_children(&this);
            loop {
                if !node_child_iterator_next(&mut iter, &mut this)
                    || ts_node_start_byte(this) > start_byte
                {
                    return node_null();
                }
                if this.id == descendant.id {
                    return this;
                }

                // If the descendant is empty, and the end byte is within `self`,
                // we check whether `self` contains it or not.
                if is_empty && iter.position.bytes >= end_byte && ts_node_child_count(this) > 0 {
                    let child = ts_node_child_with_descendant(this, descendant);
                    // If the child is not null, return self if it's relevant, else return the child
                    if !ts_node_is_null(child) {
                        return if node_is_relevant(this, true) {
                            this
                        } else {
                            child
                        };
                    }
                }
                let again = (if is_empty {
                    iter.position.bytes <= end_byte
                } else {
                    iter.position.bytes < end_byte
                }) || ts_node_child_count(this) == 0;
                if !again {
                    break;
                }
            }
            if node_is_relevant(this, true) {
                break;
            }
        }

        this
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_child(this: TSNode, child_index: u32) -> TSNode {
    unsafe { node_child(this, child_index, true) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_named_child(this: TSNode, child_index: u32) -> TSNode {
    unsafe { node_child(this, child_index, false) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_child_by_field_id(
    mut this: TSNode,
    field_id: TSFieldId,
) -> TSNode {
    unsafe {
        'recur: loop {
            if field_id == 0 || ts_node_child_count(this) == 0 {
                return node_null();
            }

            let mut field_map: *const TSFieldMapEntry = ptr::null();
            let mut field_map_end: *const TSFieldMapEntry = ptr::null();
            ts_language_field_map(
                tree_language(&this),
                u32::from((*node_subtree(this).ptr).u.node.production_id),
                &mut field_map,
                &mut field_map_end,
            );
            if field_map == field_map_end {
                return node_null();
            }

            // The field mappings are sorted by their field id. Scan all
            // the mappings to find the ones for the given field id.
            while (*field_map).field_id < field_id {
                field_map = field_map.add(1);
                if field_map == field_map_end {
                    return node_null();
                }
            }
            while (*field_map_end.sub(1)).field_id > field_id {
                field_map_end = field_map_end.sub(1);
                if field_map == field_map_end {
                    return node_null();
                }
            }

            let mut child = node_null();
            let mut iterator = node_iterate_children(&this);
            while node_child_iterator_next(&mut iterator, &mut child) {
                if !ts_subtree_extra(node_subtree(child)) {
                    let index = iterator.structural_child_index.wrapping_sub(1);
                    if index < u32::from((*field_map).child_index) {
                        continue;
                    }

                    // Hidden nodes' fields are "inherited" by their visible parent.
                    if (*field_map).inherited {
                        // If this is the *last* possible child node for this field,
                        // then perform a tail call to avoid recursion.
                        if field_map.add(1) == field_map_end {
                            this = child;
                            continue 'recur;
                        }
                        // Otherwise, descend into this child, but if it doesn't contain
                        // the field, continue searching subsequent children.
                        else {
                            let result = ts_node_child_by_field_id(child, field_id);
                            if !result.id.is_null() {
                                return result;
                            }
                            field_map = field_map.add(1);
                            if field_map == field_map_end {
                                return node_null();
                            }
                        }
                    } else if node_is_relevant(child, true) {
                        return child;
                    }
                    // If the field refers to a hidden node with visible children,
                    // return the first visible child.
                    else if ts_node_child_count(child) > 0 {
                        return ts_node_child(child, 0);
                    }
                    // Otherwise, continue searching subsequent children.
                    else {
                        field_map = field_map.add(1);
                        if field_map == field_map_end {
                            return node_null();
                        }
                    }
                }
            }

            return node_null();
        }
    }
}

unsafe fn node_field_name_from_language(
    this: TSNode,
    structural_child_index: u32,
) -> *const c_char {
    unsafe {
        let mut field_map: *const TSFieldMapEntry = ptr::null();
        let mut field_map_end: *const TSFieldMapEntry = ptr::null();
        ts_language_field_map(
            tree_language(&this),
            u32::from((*node_subtree(this).ptr).u.node.production_id),
            &mut field_map,
            &mut field_map_end,
        );
        while field_map != field_map_end {
            if !(*field_map).inherited
                && u32::from((*field_map).child_index) == structural_child_index
            {
                return *(*tree_language(&this))
                    .field_names
                    .add(usize::from((*field_map).field_id));
            }
            field_map = field_map.add(1);
        }
        ptr::null()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_field_name_for_child(
    this: TSNode,
    mut child_index: u32,
) -> *const c_char {
    unsafe {
        let mut result = this;
        let mut did_descend = true;
        let mut inherited_field_name: *const c_char = ptr::null();

        while did_descend {
            did_descend = false;

            let mut child = node_null();
            let mut index = 0u32;
            let mut iterator = node_iterate_children(&result);
            while node_child_iterator_next(&mut iterator, &mut child) {
                if node_is_relevant(child, true) {
                    if index == child_index {
                        if ts_node_is_extra(child) {
                            return ptr::null();
                        }
                        let field_name = node_field_name_from_language(
                            result,
                            iterator.structural_child_index.wrapping_sub(1),
                        );
                        if !field_name.is_null() {
                            return field_name;
                        }
                        return inherited_field_name;
                    }
                    index += 1;
                } else {
                    let grandchild_index = child_index.wrapping_sub(index);
                    let grandchild_count = node_relevant_child_count(child, true);
                    if grandchild_index < grandchild_count {
                        let field_name = node_field_name_from_language(
                            result,
                            iterator.structural_child_index.wrapping_sub(1),
                        );
                        if !field_name.is_null() {
                            inherited_field_name = field_name;
                        }

                        did_descend = true;
                        result = child;
                        child_index = grandchild_index;
                        break;
                    }
                    index = index.wrapping_add(grandchild_count);
                }
            }
        }

        ptr::null()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_field_name_for_named_child(
    this: TSNode,
    mut named_child_index: u32,
) -> *const c_char {
    unsafe {
        let mut result = this;
        let mut did_descend = true;
        let mut inherited_field_name: *const c_char = ptr::null();

        while did_descend {
            did_descend = false;

            let mut child = node_null();
            let mut index = 0u32;
            let mut iterator = node_iterate_children(&result);
            while node_child_iterator_next(&mut iterator, &mut child) {
                if node_is_relevant(child, false) {
                    if index == named_child_index {
                        if ts_node_is_extra(child) {
                            return ptr::null();
                        }
                        let field_name = node_field_name_from_language(
                            result,
                            iterator.structural_child_index.wrapping_sub(1),
                        );
                        if !field_name.is_null() {
                            return field_name;
                        }
                        return inherited_field_name;
                    }
                    index += 1;
                } else {
                    let named_grandchild_index = named_child_index.wrapping_sub(index);
                    let grandchild_count = node_relevant_child_count(child, false);
                    if named_grandchild_index < grandchild_count {
                        let field_name = node_field_name_from_language(
                            result,
                            iterator.structural_child_index.wrapping_sub(1),
                        );
                        if !field_name.is_null() {
                            inherited_field_name = field_name;
                        }

                        did_descend = true;
                        result = child;
                        named_child_index = named_grandchild_index;
                        break;
                    }
                    index = index.wrapping_add(grandchild_count);
                }
            }
        }

        ptr::null()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_child_by_field_name(
    this: TSNode,
    name: *const c_char,
    name_length: u32,
) -> TSNode {
    unsafe {
        let field_id = ts_language_field_id_for_name(tree_language(&this), name, name_length);
        ts_node_child_by_field_id(this, field_id)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_child_count(this: TSNode) -> u32 {
    unsafe {
        let tree = node_subtree(this);
        if ts_subtree_child_count(tree) > 0 {
            (*tree.ptr).u.node.visible_child_count
        } else {
            0
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_named_child_count(this: TSNode) -> u32 {
    unsafe {
        let tree = node_subtree(this);
        if ts_subtree_child_count(tree) > 0 {
            (*tree.ptr).u.node.named_child_count
        } else {
            0
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_next_sibling(this: TSNode) -> TSNode {
    unsafe { node_next_sibling(this, true) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_next_named_sibling(this: TSNode) -> TSNode {
    unsafe { node_next_sibling(this, false) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_prev_sibling(this: TSNode) -> TSNode {
    unsafe { node_prev_sibling(this, true) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_prev_named_sibling(this: TSNode) -> TSNode {
    unsafe { node_prev_sibling(this, false) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_first_child_for_byte(this: TSNode, byte: u32) -> TSNode {
    unsafe { node_first_child_for_byte(this, byte, true) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_first_named_child_for_byte(this: TSNode, byte: u32) -> TSNode {
    unsafe { node_first_child_for_byte(this, byte, false) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_descendant_for_byte_range(
    this: TSNode,
    start: u32,
    end: u32,
) -> TSNode {
    unsafe { node_descendant_for_byte_range(this, start, end, true) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_named_descendant_for_byte_range(
    this: TSNode,
    start: u32,
    end: u32,
) -> TSNode {
    unsafe { node_descendant_for_byte_range(this, start, end, false) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_descendant_for_point_range(
    this: TSNode,
    start: TSPoint,
    end: TSPoint,
) -> TSNode {
    unsafe { node_descendant_for_point_range(this, start, end, true) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_named_descendant_for_point_range(
    this: TSNode,
    start: TSPoint,
    end: TSPoint,
) -> TSNode {
    unsafe { node_descendant_for_point_range(this, start, end, false) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_node_edit(this: *mut TSNode, edit: *const TSInputEdit) {
    unsafe {
        let mut start_byte = ts_node_start_byte(*this);
        let mut start_point = ts_node_start_point(*this);

        crate::point::ts_point_edit(&mut start_point, &mut start_byte, edit);

        (*this).context[0] = start_byte;
        (*this).context[1] = start_point.row;
        (*this).context[2] = start_point.column;
    }
}
