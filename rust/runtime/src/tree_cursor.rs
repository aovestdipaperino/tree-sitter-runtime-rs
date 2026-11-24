//! Types of `src/tree_cursor.h` (the port of `tree_cursor.c` follows).
use crate::api::TSSymbol;
use crate::array::Array;
use crate::length::Length;
use crate::subtree::Subtree;
use crate::tree::TSTree;

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
pub struct TreeCursor {
    pub tree: *const TSTree,
    pub stack: Array<TreeCursorEntry>,
    pub root_alias_symbol: TSSymbol,
}
