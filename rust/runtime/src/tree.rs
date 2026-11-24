//! Port of `src/tree.c` / `src/tree.h`: the public `TSTree`.
#![allow(clippy::missing_safety_doc)]
use crate::alloc::{ts_calloc, ts_free, ts_malloc};
use crate::api::*;
use crate::array::Array;
use crate::get_changed_ranges::{
    ts_range_array_get_changed_ranges, ts_range_edit, ts_subtree_get_changed_ranges,
};
use crate::language::*;
use crate::length::*;
use crate::subtree::*;
use crate::tree_cursor::{TreeCursor, ts_tree_cursor_init};
use core::ffi::{c_int, c_void};
use core::ptr;

#[repr(C)]
pub struct TSTree {
    pub root: Subtree,
    pub language: *const TSLanguage,
    pub included_ranges: *mut TSRange,
    pub included_range_count: u32,
}

type TSRangeArray = Array<TSRange>;

unsafe extern "C" {
    // node.c
    fn ts_node_new(
        tree: *const TSTree,
        subtree: *const Subtree,
        position: Length,
        alias: TSSymbol,
    ) -> TSNode;
    // libc
    fn dup(fd: c_int) -> c_int;
    fn fdopen(fd: c_int, mode: *const core::ffi::c_char) -> *mut c_void;
    fn fclose(f: *mut c_void) -> c_int;
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_new(
    root: Subtree,
    language: *const TSLanguage,
    included_ranges: *const TSRange,
    included_range_count: u32,
) -> *mut TSTree {
    unsafe {
        let result = ts_malloc(size_of::<TSTree>()) as *mut TSTree;
        (*result).root = root;
        (*result).language = ts_language_copy(language);
        (*result).included_ranges =
            ts_calloc(included_range_count as usize, size_of::<TSRange>()) as *mut TSRange;
        ptr::copy_nonoverlapping(
            included_ranges,
            (*result).included_ranges,
            included_range_count as usize,
        );
        (*result).included_range_count = included_range_count;
        result
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_copy(this: *const TSTree) -> *mut TSTree {
    unsafe {
        ts_subtree_retain((*this).root);
        ts_tree_new(
            (*this).root,
            (*this).language,
            (*this).included_ranges,
            (*this).included_range_count,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_delete(this: *mut TSTree) {
    unsafe {
        if this.is_null() {
            return;
        }
        let mut pool = ts_subtree_pool_new(0);
        ts_subtree_release(&mut pool, (*this).root);
        ts_subtree_pool_delete(&mut pool);
        ts_language_delete((*this).language);
        ts_free((*this).included_ranges as *mut c_void);
        ts_free(this as *mut c_void);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_root_node(this: *const TSTree) -> TSNode {
    unsafe { ts_node_new(this, &(*this).root, ts_subtree_padding((*this).root), 0) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_root_node_with_offset(
    this: *const TSTree,
    offset_bytes: u32,
    offset_extent: TSPoint,
) -> TSNode {
    unsafe {
        let offset = Length {
            bytes: offset_bytes,
            extent: offset_extent,
        };
        ts_node_new(
            this,
            &(*this).root,
            length_add(offset, ts_subtree_padding((*this).root)),
            0,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_language(this: *const TSTree) -> *const TSLanguage {
    unsafe { (*this).language }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_edit(this: *mut TSTree, edit: *const TSInputEdit) {
    unsafe {
        for i in 0..(*this).included_range_count {
            ts_range_edit((*this).included_ranges.add(i as usize), edit);
        }

        let mut pool = ts_subtree_pool_new(0);
        (*this).root = ts_subtree_edit((*this).root, edit, &mut pool);
        ts_subtree_pool_delete(&mut pool);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_included_ranges(
    this: *const TSTree,
    length: *mut u32,
) -> *mut TSRange {
    unsafe {
        *length = (*this).included_range_count;
        let ranges =
            ts_calloc((*this).included_range_count as usize, size_of::<TSRange>()) as *mut TSRange;
        ptr::copy_nonoverlapping(
            (*this).included_ranges,
            ranges,
            (*this).included_range_count as usize,
        );
        ranges
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_get_changed_ranges(
    old_tree: *const TSTree,
    new_tree: *const TSTree,
    length: *mut u32,
) -> *mut TSRange {
    unsafe {
        let mut cursor1 = TreeCursor {
            tree: ptr::null(),
            stack: Array::new(),
            root_alias_symbol: 0,
        };
        let mut cursor2 = TreeCursor {
            tree: ptr::null(),
            stack: Array::new(),
            root_alias_symbol: 0,
        };
        ts_tree_cursor_init(&mut cursor1, ts_tree_root_node(old_tree));
        ts_tree_cursor_init(&mut cursor2, ts_tree_root_node(new_tree));

        let mut included_range_differences: TSRangeArray = Array::new();
        ts_range_array_get_changed_ranges(
            (*old_tree).included_ranges,
            (*old_tree).included_range_count,
            (*new_tree).included_ranges,
            (*new_tree).included_range_count,
            &mut included_range_differences,
        );

        let mut result: *mut TSRange = ptr::null_mut();
        *length = ts_subtree_get_changed_ranges(
            &(*old_tree).root,
            &(*new_tree).root,
            &mut cursor1,
            &mut cursor2,
            (*old_tree).language,
            &included_range_differences,
            &mut result,
        );

        included_range_differences.delete();
        cursor1.stack.delete();
        cursor2.stack.delete();
        result
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _ts_dup(file_descriptor: c_int) -> c_int {
    unsafe { dup(file_descriptor) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_tree_print_dot_graph(this: *const TSTree, file_descriptor: c_int) {
    unsafe {
        let file = fdopen(_ts_dup(file_descriptor), c"a".as_ptr());
        ts_subtree_print_dot_graph((*this).root, (*this).language, file);
        fclose(file);
    }
}
