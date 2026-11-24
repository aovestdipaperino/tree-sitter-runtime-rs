//! Port of `src/reusable_node.h`: walks the old tree during an incremental parse, offering
//! subtrees that may be reused.
#![allow(clippy::missing_safety_doc)]
use crate::array::Array;
use crate::subtree::*;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct StackEntry {
    pub tree: Subtree,
    pub child_index: u32,
    pub byte_offset: u32,
}

#[repr(C)]
pub struct ReusableNode {
    pub stack: Array<StackEntry>,
    pub last_external_token: Subtree,
}

pub fn reusable_node_new() -> ReusableNode {
    ReusableNode {
        stack: Array::new(),
        last_external_token: NULL_SUBTREE,
    }
}

pub fn reusable_node_clear(this: &mut ReusableNode) {
    this.stack.clear();
    this.last_external_token = NULL_SUBTREE;
}

pub unsafe fn reusable_node_tree(this: &ReusableNode) -> Subtree {
    unsafe {
        if this.stack.size > 0 {
            (*this.stack.contents.add(this.stack.size as usize - 1)).tree
        } else {
            NULL_SUBTREE
        }
    }
}

pub unsafe fn reusable_node_byte_offset(this: &ReusableNode) -> u32 {
    unsafe {
        if this.stack.size > 0 {
            (*this.stack.contents.add(this.stack.size as usize - 1)).byte_offset
        } else {
            u32::MAX
        }
    }
}

pub unsafe fn reusable_node_delete(this: &mut ReusableNode) {
    unsafe { this.stack.delete() }
}

pub unsafe fn reusable_node_advance(this: &mut ReusableNode) {
    unsafe {
        let last_entry = *this.stack.back();
        let byte_offset = last_entry
            .byte_offset
            .wrapping_add(ts_subtree_total_bytes(last_entry.tree));
        if ts_subtree_has_external_tokens(last_entry.tree) {
            this.last_external_token = ts_subtree_last_external_token(last_entry.tree);
        }

        let mut tree;
        let mut next_index;
        loop {
            let popped_entry = this.stack.pop();
            next_index = popped_entry.child_index + 1;
            if this.stack.size == 0 {
                return;
            }
            tree = (*this.stack.back()).tree;
            if ts_subtree_child_count(tree) > next_index {
                break;
            }
        }

        this.stack.push(StackEntry {
            tree: *ts_subtree_children(tree).add(next_index as usize),
            child_index: next_index,
            byte_offset,
        });
    }
}

pub unsafe fn reusable_node_descend(this: &mut ReusableNode) -> bool {
    unsafe {
        let last_entry = *this.stack.back();
        if ts_subtree_child_count(last_entry.tree) > 0 {
            this.stack.push(StackEntry {
                tree: *ts_subtree_children(last_entry.tree),
                child_index: 0,
                byte_offset: last_entry.byte_offset,
            });
            true
        } else {
            false
        }
    }
}

pub unsafe fn reusable_node_advance_past_leaf(this: &mut ReusableNode) {
    unsafe {
        while reusable_node_descend(this) {}
        reusable_node_advance(this);
    }
}

pub unsafe fn reusable_node_reset(this: &mut ReusableNode, tree: Subtree) {
    unsafe {
        reusable_node_clear(this);
        this.stack.push(StackEntry {
            tree,
            child_index: 0,
            byte_offset: 0,
        });

        // Never reuse the root node: it has a non-standard structure (EOF child and extras added on accept).
        if !reusable_node_descend(this) {
            reusable_node_clear(this);
        }
    }
}
