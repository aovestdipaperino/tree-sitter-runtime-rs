//! Port of `src/get_changed_ranges.c`: computing the ranges whose syntax changed between two trees.
#![allow(clippy::missing_safety_doc)]
use crate::api::*;
use crate::array::Array;
use crate::consts::*;
use crate::language::*;
use crate::length::*;
use crate::parser_abi::*;
use crate::point::*;
use crate::subtree::*;
use crate::tree_cursor::{TreeCursor, TreeCursorEntry};
use core::ptr;

pub type TSRangeArray = Array<TSRange>;

const POINT_MAX: TSPoint = TSPoint {
    row: u32::MAX,
    column: u32::MAX,
};

unsafe fn range_array_add(this: *mut TSRangeArray, start: Length, end: Length) {
    unsafe {
        if (*this).size > 0 {
            let last_range = (*this).back();
            if start.bytes <= (*last_range).end_byte {
                (*last_range).end_byte = end.bytes;
                (*last_range).end_point = end.extent;
                return;
            }
        }

        if start.bytes < end.bytes {
            let range = TSRange {
                start_point: start.extent,
                end_point: end.extent,
                start_byte: start.bytes,
                end_byte: end.bytes,
            };
            (*this).push(range);
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_range_array_intersects(
    this: *const TSRangeArray,
    start_index: u32,
    start_byte: u32,
    end_byte: u32,
) -> bool {
    unsafe {
        let mut i = start_index;
        while i < (*this).size {
            let range = (*this).get(i);
            if (*range).end_byte > start_byte {
                if (*range).start_byte >= end_byte {
                    break;
                }
                return true;
            }
            i += 1;
        }
        false
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_range_array_get_changed_ranges(
    old_ranges: *const TSRange,
    old_range_count: u32,
    new_ranges: *const TSRange,
    new_range_count: u32,
    differences: *mut TSRangeArray,
) {
    unsafe {
        let mut new_index = 0u32;
        let mut old_index = 0u32;
        let mut current_position = length_zero();
        let mut in_old_range = false;
        let mut in_new_range = false;

        while old_index < old_range_count || new_index < new_range_count {
            // The C forms these pointers even past the end of the arrays but only reads them when in range.
            let old_range = old_ranges.wrapping_add(old_index as usize);
            let new_range = new_ranges.wrapping_add(new_index as usize);

            let next_old_position = if in_old_range {
                Length {
                    bytes: (*old_range).end_byte,
                    extent: (*old_range).end_point,
                }
            } else if old_index < old_range_count {
                Length {
                    bytes: (*old_range).start_byte,
                    extent: (*old_range).start_point,
                }
            } else {
                LENGTH_MAX
            };

            let next_new_position = if in_new_range {
                Length {
                    bytes: (*new_range).end_byte,
                    extent: (*new_range).end_point,
                }
            } else if new_index < new_range_count {
                Length {
                    bytes: (*new_range).start_byte,
                    extent: (*new_range).start_point,
                }
            } else {
                LENGTH_MAX
            };

            if next_old_position.bytes < next_new_position.bytes {
                if in_old_range != in_new_range {
                    range_array_add(differences, current_position, next_old_position);
                }
                if in_old_range {
                    old_index += 1;
                }
                current_position = next_old_position;
                in_old_range = !in_old_range;
            } else if next_new_position.bytes < next_old_position.bytes {
                if in_old_range != in_new_range {
                    range_array_add(differences, current_position, next_new_position);
                }
                if in_new_range {
                    new_index += 1;
                }
                current_position = next_new_position;
                in_new_range = !in_new_range;
            } else {
                if in_old_range != in_new_range {
                    range_array_add(differences, current_position, next_new_position);
                }
                if in_old_range {
                    old_index += 1;
                }
                if in_new_range {
                    new_index += 1;
                }
                in_old_range = !in_old_range;
                in_new_range = !in_new_range;
                current_position = next_new_position;
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_range_edit(range: *mut TSRange, edit: *const TSInputEdit) {
    unsafe {
        let r = &mut *range;
        let e = &*edit;
        if r.end_byte >= e.old_end_byte {
            if r.end_byte != u32::MAX {
                r.end_byte = e
                    .new_end_byte
                    .wrapping_add(r.end_byte.wrapping_sub(e.old_end_byte));
                r.end_point = point_add(e.new_end_point, point_sub(r.end_point, e.old_end_point));
                if r.end_byte < e.new_end_byte {
                    r.end_byte = u32::MAX;
                    r.end_point = POINT_MAX;
                }
            }
        } else if r.end_byte > e.start_byte {
            r.end_byte = e.start_byte;
            r.end_point = e.start_point;
        }

        if r.start_byte >= e.old_end_byte {
            r.start_byte = e
                .new_end_byte
                .wrapping_add(r.start_byte.wrapping_sub(e.old_end_byte));
            r.start_point = point_add(e.new_end_point, point_sub(r.start_point, e.old_end_point));
            if r.start_byte < e.new_end_byte {
                r.start_byte = u32::MAX;
                r.start_point = POINT_MAX;
            }
        } else if r.start_byte > e.start_byte {
            r.start_byte = e.start_byte;
            r.start_point = e.start_point;
        }
    }
}

#[derive(Clone, Copy)]
struct Iterator {
    cursor: TreeCursor,
    language: *const TSLanguage,
    visible_depth: u32,
    in_padding: bool,
    prev_external_token: Subtree,
}

unsafe fn iterator_new(
    cursor: *mut TreeCursor,
    tree: *const Subtree,
    language: *const TSLanguage,
) -> Iterator {
    unsafe {
        (*cursor).stack.clear();
        (*cursor).stack.push(TreeCursorEntry {
            subtree: tree,
            position: length_zero(),
            child_index: 0,
            structural_child_index: 0,
            descendant_index: 0,
        });
        Iterator {
            cursor: ptr::read(cursor),
            language,
            visible_depth: 1,
            in_padding: false,
            prev_external_token: NULL_SUBTREE,
        }
    }
}

fn iterator_done(this: &Iterator) -> bool {
    this.cursor.stack.size == 0
}

unsafe fn iterator_start_position(this: &Iterator) -> Length {
    unsafe {
        let entry = *this.cursor.stack.back();
        if this.in_padding {
            entry.position
        } else {
            length_add(entry.position, ts_subtree_padding(*entry.subtree))
        }
    }
}

unsafe fn iterator_end_position(this: &Iterator) -> Length {
    unsafe {
        let entry = *this.cursor.stack.back();
        let result = length_add(entry.position, ts_subtree_padding(*entry.subtree));
        if this.in_padding {
            result
        } else {
            length_add(result, ts_subtree_size(*entry.subtree))
        }
    }
}

unsafe fn iterator_tree_is_visible(this: &Iterator) -> bool {
    unsafe {
        let entry = *this.cursor.stack.back();
        if ts_subtree_visible(*entry.subtree) {
            return true;
        }
        if this.cursor.stack.size > 1 {
            let parent = *(*this.cursor.stack.get(this.cursor.stack.size - 2)).subtree;
            return ts_language_alias_at(
                this.language,
                u32::from((*parent.ptr).u.node.production_id),
                entry.structural_child_index,
            ) != 0;
        }
        false
    }
}

unsafe fn iterator_get_visible_state(
    this: &Iterator,
    tree: &mut Subtree,
    alias_symbol: &mut TSSymbol,
    start_byte: &mut u32,
) {
    unsafe {
        let mut i = this.cursor.stack.size.wrapping_sub(1);

        if this.in_padding {
            if i == 0 {
                return;
            }
            i -= 1;
        }

        while i.wrapping_add(1) > 0 {
            let entry = *this.cursor.stack.get(i);

            if i > 0 {
                let parent = (*this.cursor.stack.get(i - 1)).subtree;
                *alias_symbol = ts_language_alias_at(
                    this.language,
                    u32::from((*(*parent).ptr).u.node.production_id),
                    entry.structural_child_index,
                );
            }

            if ts_subtree_visible(*entry.subtree) || *alias_symbol != 0 {
                *tree = *entry.subtree;
                *start_byte = entry.position.bytes;
                break;
            }
            i = i.wrapping_sub(1);
        }
    }
}

unsafe fn iterator_ascend(this: &mut Iterator) {
    unsafe {
        if iterator_done(this) {
            return;
        }
        if iterator_tree_is_visible(this) && !this.in_padding {
            this.visible_depth -= 1;
        }
        if (*this.cursor.stack.back()).child_index > 0 {
            this.in_padding = false;
        }
        this.cursor.stack.size -= 1;
    }
}

unsafe fn iterator_descend(this: &mut Iterator, goal_position: u32) -> bool {
    unsafe {
        if this.in_padding {
            return false;
        }

        loop {
            let mut did_descend = false;
            let entry = *this.cursor.stack.back();
            let mut position = entry.position;
            let mut structural_child_index = 0u32;
            let n = ts_subtree_child_count(*entry.subtree);
            for i in 0..n {
                let child = ts_subtree_children(*entry.subtree).add(i as usize);
                let child_left = length_add(position, ts_subtree_padding(*child));
                let child_right = length_add(child_left, ts_subtree_size(*child));

                if child_right.bytes > goal_position {
                    this.cursor.stack.push(TreeCursorEntry {
                        subtree: child,
                        position,
                        child_index: i,
                        structural_child_index,
                        descendant_index: 0,
                    });

                    if iterator_tree_is_visible(this) {
                        if child_left.bytes > goal_position {
                            this.in_padding = true;
                        } else {
                            this.visible_depth += 1;
                        }
                        return true;
                    }

                    did_descend = true;
                    break;
                }

                position = child_right;
                if !ts_subtree_extra(*child) {
                    structural_child_index += 1;
                }
                let last_external_token = ts_subtree_last_external_token(*child);
                if !last_external_token.ptr.is_null() {
                    this.prev_external_token = last_external_token;
                }
            }
            if !did_descend {
                break;
            }
        }

        false
    }
}

unsafe fn iterator_advance(this: &mut Iterator) {
    unsafe {
        if this.in_padding {
            this.in_padding = false;
            if iterator_tree_is_visible(this) {
                this.visible_depth += 1;
            } else {
                iterator_descend(this, 0);
            }
            return;
        }

        loop {
            if iterator_tree_is_visible(this) {
                this.visible_depth -= 1;
            }
            let entry = this.cursor.stack.pop();
            if iterator_done(this) {
                return;
            }

            let parent = (*this.cursor.stack.back()).subtree;
            let child_index = entry.child_index + 1;
            let last_external_token = ts_subtree_last_external_token(*entry.subtree);
            if !last_external_token.ptr.is_null() {
                this.prev_external_token = last_external_token;
            }
            if ts_subtree_child_count(*parent) > child_index {
                let position = length_add(entry.position, ts_subtree_total_size(*entry.subtree));
                let mut structural_child_index = entry.structural_child_index;
                if !ts_subtree_extra(*entry.subtree) {
                    structural_child_index += 1;
                }
                let next_child = ts_subtree_children(*parent).add(child_index as usize);

                this.cursor.stack.push(TreeCursorEntry {
                    subtree: next_child,
                    position,
                    child_index,
                    structural_child_index,
                    descendant_index: 0,
                });

                if iterator_tree_is_visible(this) {
                    if ts_subtree_padding(*next_child).bytes > 0 {
                        this.in_padding = true;
                    } else {
                        this.visible_depth += 1;
                    }
                } else {
                    iterator_descend(this, 0);
                }
                break;
            }
        }
    }
}

#[derive(PartialEq, Eq)]
enum IteratorComparison {
    Differs,
    MayDiffer,
    Matches,
}

unsafe fn iterator_compare(old_iter: &Iterator, new_iter: &Iterator) -> IteratorComparison {
    unsafe {
        let mut old_tree = NULL_SUBTREE;
        let mut new_tree = NULL_SUBTREE;
        let mut old_start = 0u32;
        let mut new_start = 0u32;
        let mut old_alias_symbol: TSSymbol = 0;
        let mut new_alias_symbol: TSSymbol = 0;
        iterator_get_visible_state(
            old_iter,
            &mut old_tree,
            &mut old_alias_symbol,
            &mut old_start,
        );
        iterator_get_visible_state(
            new_iter,
            &mut new_tree,
            &mut new_alias_symbol,
            &mut new_start,
        );
        // As in the C, the symbols are read before the null checks (a null tree would crash here).
        let old_symbol = ts_subtree_symbol(old_tree);
        let new_symbol = ts_subtree_symbol(new_tree);

        if old_tree.ptr.is_null() && new_tree.ptr.is_null() {
            return IteratorComparison::Matches;
        }
        if old_tree.ptr.is_null() || new_tree.ptr.is_null() {
            return IteratorComparison::Differs;
        }
        if old_alias_symbol != new_alias_symbol || old_symbol != new_symbol {
            return IteratorComparison::Differs;
        }

        let old_size = ts_subtree_size(old_tree).bytes;
        let new_size = ts_subtree_size(new_tree).bytes;
        let old_state = ts_subtree_parse_state(old_tree);
        let new_state = ts_subtree_parse_state(new_tree);
        let old_has_external_tokens = ts_subtree_has_external_tokens(old_tree);
        let new_has_external_tokens = ts_subtree_has_external_tokens(new_tree);
        let old_error_cost = ts_subtree_error_cost(old_tree);
        let new_error_cost = ts_subtree_error_cost(new_tree);

        if old_start != new_start
            || old_symbol == ts_builtin_sym_error
            || old_size != new_size
            || old_state == TS_TREE_STATE_NONE
            || new_state == TS_TREE_STATE_NONE
            || ((old_state == ERROR_STATE) != (new_state == ERROR_STATE))
            || old_error_cost != new_error_cost
            || old_has_external_tokens != new_has_external_tokens
            || ts_subtree_has_changes(old_tree)
            || (old_has_external_tokens
                && !ts_subtree_external_scanner_state_eq(
                    old_iter.prev_external_token,
                    new_iter.prev_external_token,
                ))
        {
            return IteratorComparison::MayDiffer;
        }

        IteratorComparison::Matches
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_subtree_get_changed_ranges(
    old_tree: *const Subtree,
    new_tree: *const Subtree,
    cursor1: *mut TreeCursor,
    cursor2: *mut TreeCursor,
    language: *const TSLanguage,
    included_range_differences: *const TSRangeArray,
    ranges: *mut *mut TSRange,
) -> u32 {
    unsafe {
        let mut results: TSRangeArray = Array::new();

        let mut old_iter = iterator_new(cursor1, old_tree, language);
        let mut new_iter = iterator_new(cursor2, new_tree, language);

        let mut included_range_difference_index = 0u32;

        let mut position = iterator_start_position(&old_iter);
        let mut next_position = iterator_start_position(&new_iter);
        if position.bytes < next_position.bytes {
            range_array_add(&mut results, position, next_position);
            position = next_position;
        } else if position.bytes > next_position.bytes {
            range_array_add(&mut results, next_position, position);
            next_position = position;
        }

        loop {
            // Compare the old and new subtrees.
            let mut comparison = iterator_compare(&old_iter, &new_iter);

            // Even identical-looking subtrees could differ internally if they contain text that was
            // previously excluded from the parse, or the other way round.
            if comparison == IteratorComparison::Matches
                && ts_range_array_intersects(
                    included_range_differences,
                    included_range_difference_index,
                    position.bytes,
                    iterator_end_position(&old_iter).bytes,
                )
            {
                comparison = IteratorComparison::MayDiffer;
            }

            let mut is_changed = false;
            match comparison {
                // Definitely identical: move to the end of both subtrees.
                IteratorComparison::Matches => {
                    next_position = iterator_end_position(&old_iter);
                }

                // Might differ internally: descend into both, to the first child spanning the position.
                IteratorComparison::MayDiffer => {
                    if iterator_descend(&mut old_iter, position.bytes) {
                        if !iterator_descend(&mut new_iter, position.bytes) {
                            is_changed = true;
                            next_position = iterator_end_position(&old_iter);
                        }
                    } else if iterator_descend(&mut new_iter, position.bytes) {
                        is_changed = true;
                        next_position = iterator_end_position(&new_iter);
                    } else {
                        next_position = length_min(
                            iterator_end_position(&old_iter),
                            iterator_end_position(&new_iter),
                        );
                    }
                }

                // Different: record a change and move to the end of both subtrees.
                IteratorComparison::Differs => {
                    is_changed = true;
                    next_position = length_min(
                        iterator_end_position(&old_iter),
                        iterator_end_position(&new_iter),
                    );
                }
            }

            // Ensure that both iterators are caught up to the current position.
            while !iterator_done(&old_iter)
                && iterator_end_position(&old_iter).bytes <= next_position.bytes
            {
                iterator_advance(&mut old_iter);
            }
            while !iterator_done(&new_iter)
                && iterator_end_position(&new_iter).bytes <= next_position.bytes
            {
                iterator_advance(&mut new_iter);
            }

            // Ensure that both iterators are at the same depth in the tree.
            while old_iter.visible_depth > new_iter.visible_depth {
                iterator_ascend(&mut old_iter);
            }
            while new_iter.visible_depth > old_iter.visible_depth {
                iterator_ascend(&mut new_iter);
            }

            if is_changed {
                range_array_add(&mut results, position, next_position);
            }

            position = next_position;

            // Track the position in the included range differences to avoid rescanning them.
            while included_range_difference_index < (*included_range_differences).size {
                let range = (*included_range_differences).get(included_range_difference_index);
                if (*range).end_byte <= position.bytes {
                    included_range_difference_index += 1;
                } else {
                    break;
                }
            }

            if iterator_done(&old_iter) || iterator_done(&new_iter) {
                break;
            }
        }

        let old_size = ts_subtree_total_size(*old_tree);
        let new_size = ts_subtree_total_size(*new_tree);
        if old_size.bytes < new_size.bytes {
            range_array_add(&mut results, old_size, new_size);
        } else if new_size.bytes < old_size.bytes {
            range_array_add(&mut results, new_size, old_size);
        }

        *cursor1 = old_iter.cursor;
        *cursor2 = new_iter.cursor;
        *ranges = results.contents;
        results.size
    }
}
