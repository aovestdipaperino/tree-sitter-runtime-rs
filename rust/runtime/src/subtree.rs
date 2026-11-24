//! Port of `src/subtree.c` / `src/subtree.h`.
//!
//! The C `Subtree` is a union of a pointer to heap data and an inline 8-byte struct of bitfields,
//! discriminated by the lowest bit (which is the `is_inline` bit and is always 0 in a valid
//! pointer). Rust has no bitfields, so the inline form is a `u64` with accessors whose bit
//! positions are checked against the C compiler's layout by `tests/subtree.rs`. Little-endian
//! only (as ADR-011 notes, big-endian bitfield order is not supported).
//!
//! Children of a heap subtree live immediately *before* its `SubtreeHeapData`.
#![allow(clippy::missing_safety_doc)]
use crate::alloc::{ts_calloc, ts_free, ts_malloc, ts_realloc};
use crate::api::*;
use crate::array::Array;
use crate::consts::*;
use crate::language::*;
use crate::length::*;
use crate::parser_abi::*;
use crate::ts_assert;
use core::ffi::{CStr, c_char, c_int, c_void};
use core::ptr;
use core::sync::atomic::{AtomicU32, Ordering};

#[cfg(target_endian = "big")]
compile_error!("subtree bitfield layout is only implemented for little-endian targets");

pub const TS_TREE_STATE_NONE: u16 = u16::MAX;
const TS_MAX_INLINE_TREE_LENGTH: u32 = u8::MAX as u32;
const TS_MAX_TREE_POOL_SIZE: u32 = 32;

// ---- ExternalScannerState ----

#[repr(C)]
#[derive(Clone, Copy)]
pub union ExternalScannerStateData {
    pub long_data: *mut c_char,
    pub short_data: [c_char; 24],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ExternalScannerState {
    pub data: ExternalScannerStateData,
    pub length: u32,
}

// ---- Inline subtree data (8 bytes of bitfields) ----

/// Bit layout (little endian): is_inline:0 visible:1 named:2 extra:3 has_changes:4 is_missing:5
/// is_keyword:6 depends_on_column:7, symbol:8..16, parse_state:16..32, padding_columns:32..40, padding_rows:40..44,
/// lookahead_bytes:44..48, padding_bytes:48..56, size_bytes:56..64.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SubtreeInlineData(pub u64);

macro_rules! bit {
    ($get:ident, $set:ident, $bit:expr) => {
        #[inline]
        pub fn $get(&self) -> bool {
            self.0 >> $bit & 1 != 0
        }
        #[inline]
        pub fn $set(&mut self, v: bool) {
            self.0 = (self.0 & !(1 << $bit)) | (u64::from(v) << $bit);
        }
    };
}
macro_rules! field {
    ($get:ident, $set:ident, $shift:expr, $bits:expr, $ty:ty) => {
        #[inline]
        pub fn $get(&self) -> $ty {
            ((self.0 >> $shift) & ((1u64 << $bits) - 1)) as $ty
        }
        #[inline]
        pub fn $set(&mut self, v: $ty) {
            let mask = ((1u64 << $bits) - 1) << $shift;
            self.0 = (self.0 & !mask) | ((u64::from(v) << $shift) & mask);
        }
    };
}

impl SubtreeInlineData {
    bit!(is_inline, set_is_inline, 0);
    bit!(visible, set_visible, 1);
    bit!(named, set_named, 2);
    bit!(extra, set_extra, 3);
    bit!(has_changes, set_has_changes, 4);
    bit!(is_missing, set_is_missing, 5);
    bit!(is_keyword, set_is_keyword, 6);
    bit!(depends_on_column, set_depends_on_column, 7);
    field!(symbol, set_symbol, 8, 8, u8);
    field!(parse_state, set_parse_state, 16, 16, u16);
    field!(padding_columns, set_padding_columns, 32, 8, u8);
    field!(padding_rows, set_padding_rows, 40, 4, u8);
    field!(lookahead_bytes, set_lookahead_bytes, 44, 4, u8);
    field!(padding_bytes, set_padding_bytes, 48, 8, u8);
    field!(size_bytes, set_size_bytes, 56, 8, u8);
}

// ---- Heap subtree data ----

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FirstLeaf {
    pub symbol: TSSymbol,
    pub parse_state: TSStateId,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct NodeData {
    pub visible_child_count: u32,
    pub named_child_count: u32,
    pub visible_descendant_count: u32,
    pub dynamic_precedence: i32,
    pub repeat_depth: u16,
    pub production_id: u16,
    pub first_leaf: FirstLeaf,
}

#[repr(C)]
pub union HeapUnion {
    pub node: NodeData,
    pub external_scanner_state: ExternalScannerState,
    pub lookahead_char: i32,
}

#[repr(C)]
pub struct SubtreeHeapData {
    pub ref_count: u32,
    pub padding: Length,
    pub size: Length,
    pub lookahead_bytes: u32,
    pub error_cost: u32,
    pub child_count: u32,
    pub symbol: TSSymbol,
    pub parse_state: TSStateId,
    /// Eleven one-bit flags, in declaration order from bit 0 (see the `FLAG_*` constants).
    pub flags: u16,
    pub u: HeapUnion,
}

const FLAG_VISIBLE: u16 = 1 << 0;
const FLAG_NAMED: u16 = 1 << 1;
const FLAG_EXTRA: u16 = 1 << 2;
const FLAG_FRAGILE_LEFT: u16 = 1 << 3;
const FLAG_FRAGILE_RIGHT: u16 = 1 << 4;
const FLAG_HAS_CHANGES: u16 = 1 << 5;
const FLAG_HAS_EXTERNAL_TOKENS: u16 = 1 << 6;
const FLAG_HAS_EXTERNAL_SCANNER_STATE_CHANGE: u16 = 1 << 7;
const FLAG_DEPENDS_ON_COLUMN: u16 = 1 << 8;
const FLAG_IS_MISSING: u16 = 1 << 9;
const FLAG_IS_KEYWORD: u16 = 1 << 10;

macro_rules! flag {
    ($get:ident, $set:ident, $flag:ident) => {
        #[inline]
        pub fn $get(&self) -> bool {
            self.flags & $flag != 0
        }
        #[inline]
        pub fn $set(&mut self, v: bool) {
            if v {
                self.flags |= $flag
            } else {
                self.flags &= !$flag
            }
        }
    };
}

impl SubtreeHeapData {
    flag!(visible, set_visible, FLAG_VISIBLE);
    flag!(named, set_named, FLAG_NAMED);
    flag!(extra, set_extra, FLAG_EXTRA);
    flag!(fragile_left, set_fragile_left, FLAG_FRAGILE_LEFT);
    flag!(fragile_right, set_fragile_right, FLAG_FRAGILE_RIGHT);
    flag!(has_changes, set_has_changes, FLAG_HAS_CHANGES);
    flag!(
        has_external_tokens,
        set_has_external_tokens,
        FLAG_HAS_EXTERNAL_TOKENS
    );
    flag!(
        has_external_scanner_state_change,
        set_has_external_scanner_state_change,
        FLAG_HAS_EXTERNAL_SCANNER_STATE_CHANGE
    );
    flag!(
        depends_on_column,
        set_depends_on_column,
        FLAG_DEPENDS_ON_COLUMN
    );
    flag!(is_missing, set_is_missing, FLAG_IS_MISSING);
    flag!(is_keyword, set_is_keyword, FLAG_IS_KEYWORD);

    /// An all-zero value, which is what the C compound literals start from.
    fn zeroed() -> Self {
        // SAFETY: every field is plain data for which all-zero is valid.
        unsafe { core::mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union Subtree {
    pub data: SubtreeInlineData,
    pub ptr: *const SubtreeHeapData,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union MutableSubtree {
    pub data: SubtreeInlineData,
    pub ptr: *mut SubtreeHeapData,
}

pub const NULL_SUBTREE: Subtree = Subtree { ptr: ptr::null() };

pub type SubtreeArray = Array<Subtree>;
pub type MutableSubtreeArray = Array<MutableSubtree>;

#[repr(C)]
pub struct SubtreePool {
    pub free_trees: MutableSubtreeArray,
    pub tree_stack: MutableSubtreeArray,
}

impl Subtree {
    #[inline]
    pub fn is_inline(self) -> bool {
        // SAFETY: both union arms are 8 bytes of plain bits.
        unsafe { self.data.is_inline() }
    }
}

// ---- Header inline helpers ----

macro_rules! subtree_flag {
    ($self:ident, $inline:ident, $heap:ident) => {
        // SAFETY: as above.
        unsafe {
            if $self.data.is_inline() {
                $self.data.$inline()
            } else {
                (*$self.ptr).$heap()
            }
        }
    };
}

pub fn ts_subtree_symbol(s: Subtree) -> TSSymbol {
    // SAFETY: the active union arm is selected by the inline bit; heap pointers are valid.
    unsafe {
        if s.data.is_inline() {
            TSSymbol::from(s.data.symbol())
        } else {
            (*s.ptr).symbol
        }
    }
}
pub fn ts_subtree_visible(s: Subtree) -> bool {
    subtree_flag!(s, visible, visible)
}
pub fn ts_subtree_named(s: Subtree) -> bool {
    subtree_flag!(s, named, named)
}
pub fn ts_subtree_extra(s: Subtree) -> bool {
    subtree_flag!(s, extra, extra)
}
pub fn ts_subtree_has_changes(s: Subtree) -> bool {
    subtree_flag!(s, has_changes, has_changes)
}
pub fn ts_subtree_missing(s: Subtree) -> bool {
    subtree_flag!(s, is_missing, is_missing)
}
pub fn ts_subtree_is_keyword(s: Subtree) -> bool {
    subtree_flag!(s, is_keyword, is_keyword)
}
pub fn ts_subtree_parse_state(s: Subtree) -> TSStateId {
    // SAFETY: the active union arm is selected by the inline bit; heap pointers are valid.
    unsafe {
        if s.data.is_inline() {
            s.data.parse_state()
        } else {
            (*s.ptr).parse_state
        }
    }
}
pub fn ts_subtree_lookahead_bytes(s: Subtree) -> u32 {
    // SAFETY: the active union arm is selected by the inline bit; heap pointers are valid.
    unsafe {
        if s.data.is_inline() {
            u32::from(s.data.lookahead_bytes())
        } else {
            (*s.ptr).lookahead_bytes
        }
    }
}

/// Size needed to store a heap subtree with `child_count` children.
pub fn ts_subtree_alloc_size(child_count: u32) -> usize {
    child_count as usize * size_of::<Subtree>() + size_of::<SubtreeHeapData>()
}

/// A subtree's children, allocated immediately before its heap data (null for inline subtrees).
pub fn ts_subtree_children(s: Subtree) -> *mut Subtree {
    // SAFETY: heap subtrees always have `child_count` children before them.
    unsafe {
        if s.data.is_inline() {
            ptr::null_mut()
        } else {
            (s.ptr as *mut Subtree).sub((*s.ptr).child_count as usize)
        }
    }
}

pub fn ts_subtree_set_extra(s: &mut MutableSubtree, is_extra: bool) {
    // SAFETY: the active union arm is selected by the inline bit; heap pointers are valid.
    unsafe {
        if s.data.is_inline() {
            s.data.set_extra(is_extra);
        } else {
            (*s.ptr).set_extra(is_extra);
        }
    }
}

pub fn ts_subtree_leaf_symbol(s: Subtree) -> TSSymbol {
    // SAFETY: the active union arm is selected by the inline bit; heap pointers are valid.
    unsafe {
        if s.data.is_inline() {
            return TSSymbol::from(s.data.symbol());
        }
        if (*s.ptr).child_count == 0 {
            return (*s.ptr).symbol;
        }
        (*s.ptr).u.node.first_leaf.symbol
    }
}

pub fn ts_subtree_leaf_parse_state(s: Subtree) -> TSStateId {
    // SAFETY: the active union arm is selected by the inline bit; heap pointers are valid.
    unsafe {
        if s.data.is_inline() {
            return s.data.parse_state();
        }
        if (*s.ptr).child_count == 0 {
            return (*s.ptr).parse_state;
        }
        (*s.ptr).u.node.first_leaf.parse_state
    }
}

pub fn ts_subtree_padding(s: Subtree) -> Length {
    // SAFETY: the active union arm is selected by the inline bit; heap pointers are valid.
    unsafe {
        if s.data.is_inline() {
            Length {
                bytes: u32::from(s.data.padding_bytes()),
                extent: TSPoint {
                    row: u32::from(s.data.padding_rows()),
                    column: u32::from(s.data.padding_columns()),
                },
            }
        } else {
            (*s.ptr).padding
        }
    }
}

pub fn ts_subtree_size(s: Subtree) -> Length {
    // SAFETY: the active union arm is selected by the inline bit; heap pointers are valid.
    unsafe {
        if s.data.is_inline() {
            Length {
                bytes: u32::from(s.data.size_bytes()),
                extent: TSPoint {
                    row: 0,
                    column: u32::from(s.data.size_bytes()),
                },
            }
        } else {
            (*s.ptr).size
        }
    }
}

pub fn ts_subtree_total_size(s: Subtree) -> Length {
    length_add(ts_subtree_padding(s), ts_subtree_size(s))
}

pub fn ts_subtree_total_bytes(s: Subtree) -> u32 {
    ts_subtree_total_size(s).bytes
}

pub fn ts_subtree_child_count(s: Subtree) -> u32 {
    // SAFETY: the active union arm is selected by the inline bit; heap pointers are valid.
    unsafe {
        if s.data.is_inline() {
            0
        } else {
            (*s.ptr).child_count
        }
    }
}

pub fn ts_subtree_repeat_depth(s: Subtree) -> u32 {
    // SAFETY: the active union arm is selected by the inline bit; heap pointers are valid. (`repeat_depth` is read without checking `child_count`, as in C.)
    unsafe {
        if s.data.is_inline() {
            0
        } else {
            u32::from((*s.ptr).u.node.repeat_depth)
        }
    }
}

pub fn ts_subtree_is_repetition(s: Subtree) -> u32 {
    // SAFETY: the active union arm is selected by the inline bit; heap pointers are valid.
    unsafe {
        if s.data.is_inline() {
            0
        } else {
            u32::from(!(*s.ptr).named() && !(*s.ptr).visible() && (*s.ptr).child_count != 0)
        }
    }
}

pub fn ts_subtree_visible_descendant_count(s: Subtree) -> u32 {
    // SAFETY: the active union arm is selected by the inline bit; heap pointers are valid.
    unsafe {
        if s.data.is_inline() || (*s.ptr).child_count == 0 {
            0
        } else {
            (*s.ptr).u.node.visible_descendant_count
        }
    }
}

pub fn ts_subtree_visible_child_count(s: Subtree) -> u32 {
    // SAFETY: the active union arm is selected by the inline bit; heap pointers are valid.
    unsafe {
        if ts_subtree_child_count(s) > 0 {
            (*s.ptr).u.node.visible_child_count
        } else {
            0
        }
    }
}

pub fn ts_subtree_error_cost(s: Subtree) -> u32 {
    if ts_subtree_missing(s) {
        ERROR_COST_PER_MISSING_TREE + ERROR_COST_PER_RECOVERY
    } else {
        // SAFETY: the active union arm is selected by the inline bit; heap pointers are valid.
        unsafe {
            if s.data.is_inline() {
                0
            } else {
                (*s.ptr).error_cost
            }
        }
    }
}

pub fn ts_subtree_dynamic_precedence(s: Subtree) -> i32 {
    // SAFETY: the active union arm is selected by the inline bit; heap pointers are valid.
    unsafe {
        if s.data.is_inline() || (*s.ptr).child_count == 0 {
            0
        } else {
            (*s.ptr).u.node.dynamic_precedence
        }
    }
}

pub fn ts_subtree_production_id(s: Subtree) -> u16 {
    // SAFETY: the active union arm is selected by the inline bit; heap pointers are valid.
    unsafe {
        if ts_subtree_child_count(s) > 0 {
            (*s.ptr).u.node.production_id
        } else {
            0
        }
    }
}

pub fn ts_subtree_fragile_left(s: Subtree) -> bool {
    // SAFETY: the active union arm is selected by the inline bit; heap pointers are valid.
    unsafe { !s.data.is_inline() && (*s.ptr).fragile_left() }
}
pub fn ts_subtree_fragile_right(s: Subtree) -> bool {
    // SAFETY: the active union arm is selected by the inline bit; heap pointers are valid.
    unsafe { !s.data.is_inline() && (*s.ptr).fragile_right() }
}
pub fn ts_subtree_has_external_tokens(s: Subtree) -> bool {
    // SAFETY: the active union arm is selected by the inline bit; heap pointers are valid.
    unsafe { !s.data.is_inline() && (*s.ptr).has_external_tokens() }
}
pub fn ts_subtree_has_external_scanner_state_change(s: Subtree) -> bool {
    // SAFETY: the active union arm is selected by the inline bit; heap pointers are valid.
    unsafe { !s.data.is_inline() && (*s.ptr).has_external_scanner_state_change() }
}
pub fn ts_subtree_depends_on_column(s: Subtree) -> bool {
    // SAFETY: the active union arm is selected by the inline bit; heap pointers are valid.
    unsafe {
        if s.data.is_inline() {
            s.data.depends_on_column()
        } else {
            (*s.ptr).depends_on_column()
        }
    }
}
pub fn ts_subtree_is_fragile(s: Subtree) -> bool {
    // SAFETY: the active union arm is selected by the inline bit; heap pointers are valid.
    unsafe { !s.data.is_inline() && ((*s.ptr).fragile_left() || (*s.ptr).fragile_right()) }
}
pub fn ts_subtree_is_error(s: Subtree) -> bool {
    ts_subtree_symbol(s) == ts_builtin_sym_error
}
pub fn ts_subtree_is_eof(s: Subtree) -> bool {
    ts_subtree_symbol(s) == ts_builtin_sym_end
}
pub fn ts_subtree_from_mut(s: MutableSubtree) -> Subtree {
    // SAFETY: both arms are the same 8 bytes.
    Subtree {
        data: unsafe { s.data },
    }
}
pub fn ts_subtree_to_mut_unsafe(s: Subtree) -> MutableSubtree {
    // SAFETY: as above.
    MutableSubtree {
        data: unsafe { s.data },
    }
}

// ---- ExternalScannerState ----

const SHORT_DATA: usize = 24;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_external_scanner_state_init(
    this: *mut ExternalScannerState,
    data: *const c_char,
    length: u32,
) {
    unsafe {
        (*this).length = length;
        if length as usize > SHORT_DATA {
            let p = ts_malloc(length as usize) as *mut c_char;
            (*this).data.long_data = p;
            ptr::copy_nonoverlapping(data, p, length as usize);
        } else {
            ptr::copy_nonoverlapping(data, (*this).data.short_data.as_mut_ptr(), length as usize);
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_external_scanner_state_copy(
    this: *const ExternalScannerState,
) -> ExternalScannerState {
    unsafe {
        let mut result = *this;
        if (*this).length as usize > SHORT_DATA {
            let p = ts_malloc((*this).length as usize) as *mut c_char;
            result.data.long_data = p;
            ptr::copy_nonoverlapping((*this).data.long_data, p, (*this).length as usize);
        }
        result
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_external_scanner_state_delete(this: *mut ExternalScannerState) {
    unsafe {
        if (*this).length as usize > SHORT_DATA {
            ts_free((*this).data.long_data as *mut c_void);
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_external_scanner_state_data(
    this: *const ExternalScannerState,
) -> *const c_char {
    unsafe {
        if (*this).length as usize > SHORT_DATA {
            (*this).data.long_data
        } else {
            (*this).data.short_data.as_ptr()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_external_scanner_state_eq(
    this: *const ExternalScannerState,
    buffer: *const c_char,
    length: u32,
) -> bool {
    unsafe {
        (*this).length == length && {
            let a = ts_external_scanner_state_data(this) as *const u8;
            core::slice::from_raw_parts(a, length as usize)
                == core::slice::from_raw_parts(buffer as *const u8, length as usize)
        }
    }
}

// ---- SubtreeArray ----

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_subtree_array_copy(this: SubtreeArray, dest: *mut SubtreeArray) {
    unsafe {
        (*dest).size = this.size;
        (*dest).capacity = this.capacity;
        (*dest).contents = this.contents;
        if this.capacity > 0 {
            (*dest).contents =
                ts_calloc(this.capacity as usize, size_of::<Subtree>()) as *mut Subtree;
            ptr::copy_nonoverlapping(this.contents, (*dest).contents, this.size as usize);
            for i in 0..this.size {
                ts_subtree_retain(*(*dest).get(i));
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_subtree_array_clear(pool: *mut SubtreePool, this: *mut SubtreeArray) {
    unsafe {
        for i in 0..(*this).size {
            ts_subtree_release(pool, *(*this).get(i));
        }
        (*this).clear();
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_subtree_array_delete(pool: *mut SubtreePool, this: *mut SubtreeArray) {
    unsafe {
        ts_subtree_array_clear(pool, this);
        (*this).delete();
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_subtree_array_remove_trailing_extras(
    this: *mut SubtreeArray,
    destination: *mut SubtreeArray,
) {
    unsafe {
        (*destination).clear();
        while (*this).size > 0 {
            let last = *(*this).get((*this).size - 1);
            if ts_subtree_extra(last) {
                (*this).size -= 1;
                (*destination).push(last);
            } else {
                break;
            }
        }
        ts_subtree_array_reverse(destination);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_subtree_array_reverse(this: *mut SubtreeArray) {
    unsafe {
        let limit = (*this).size / 2;
        for i in 0..limit {
            let reverse_index = (*this).size - 1 - i;
            let swap = *(*this).get(i);
            *(*this).get(i) = *(*this).get(reverse_index);
            *(*this).get(reverse_index) = swap;
        }
    }
}

// ---- SubtreePool ----

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_subtree_pool_new(capacity: u32) -> SubtreePool {
    unsafe {
        let mut this = SubtreePool {
            free_trees: Array::new(),
            tree_stack: Array::new(),
        };
        this.free_trees.reserve(capacity);
        this
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_subtree_pool_delete(this: *mut SubtreePool) {
    unsafe {
        if !(*this).free_trees.contents.is_null() {
            for i in 0..(*this).free_trees.size {
                ts_free((*(*this).free_trees.get(i)).ptr as *mut c_void);
            }
            (*this).free_trees.delete();
        }
        if !(*this).tree_stack.contents.is_null() {
            (*this).tree_stack.delete();
        }
    }
}

unsafe fn ts_subtree_pool_allocate(this: *mut SubtreePool) -> *mut SubtreeHeapData {
    unsafe {
        if (*this).free_trees.size > 0 {
            (*this).free_trees.pop().ptr
        } else {
            ts_malloc(size_of::<SubtreeHeapData>()) as *mut SubtreeHeapData
        }
    }
}

unsafe fn ts_subtree_pool_free(this: *mut SubtreePool, tree: *mut SubtreeHeapData) {
    unsafe {
        if (*this).free_trees.capacity > 0 && (*this).free_trees.size < TS_MAX_TREE_POOL_SIZE {
            (*this).free_trees.push(MutableSubtree { ptr: tree });
        } else {
            ts_free(tree as *mut c_void);
        }
    }
}

// ---- Subtree ----

fn ts_subtree_can_inline(padding: Length, size: Length, lookahead_bytes: u32) -> bool {
    padding.bytes < TS_MAX_INLINE_TREE_LENGTH
        && padding.extent.row < 16
        && padding.extent.column < TS_MAX_INLINE_TREE_LENGTH
        && size.bytes < TS_MAX_INLINE_TREE_LENGTH
        && size.extent.row == 0
        && size.extent.column < TS_MAX_INLINE_TREE_LENGTH
        && lookahead_bytes < 16
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_subtree_new_leaf(
    pool: *mut SubtreePool,
    symbol: TSSymbol,
    padding: Length,
    size: Length,
    lookahead_bytes: u32,
    parse_state: TSStateId,
    has_external_tokens: bool,
    depends_on_column: bool,
    is_keyword: bool,
    language: *const TSLanguage,
) -> Subtree {
    unsafe {
        let metadata = ts_language_symbol_metadata(language, symbol);
        let extra = symbol == ts_builtin_sym_end;

        let is_inline = symbol <= u16::from(u8::MAX)
            && !has_external_tokens
            && ts_subtree_can_inline(padding, size, lookahead_bytes);

        if is_inline {
            // C assigns each value to a narrower bitfield, truncating.
            let mut d = SubtreeInlineData(0);
            d.set_parse_state(parse_state);
            d.set_symbol(symbol as u8);
            d.set_padding_bytes(padding.bytes as u8);
            d.set_padding_rows(padding.extent.row as u8 & 0xf);
            d.set_padding_columns(padding.extent.column as u8);
            d.set_size_bytes(size.bytes as u8);
            d.set_lookahead_bytes(lookahead_bytes as u8 & 0xf);
            d.set_visible(metadata.visible);
            d.set_named(metadata.named);
            d.set_extra(extra);
            d.set_has_changes(false);
            d.set_is_missing(false);
            d.set_is_keyword(is_keyword);
            d.set_depends_on_column(depends_on_column);
            d.set_is_inline(true);
            Subtree { data: d }
        } else {
            let data = ts_subtree_pool_allocate(pool);
            let mut value = SubtreeHeapData::zeroed();
            value.ref_count = 1;
            value.padding = padding;
            value.size = size;
            value.lookahead_bytes = lookahead_bytes;
            value.symbol = symbol;
            value.parse_state = parse_state;
            value.set_visible(metadata.visible);
            value.set_named(metadata.named);
            value.set_extra(extra);
            value.set_has_external_tokens(has_external_tokens);
            value.set_depends_on_column(depends_on_column);
            value.set_is_keyword(is_keyword);
            ptr::write(data, value);
            Subtree { ptr: data }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_subtree_set_symbol(
    this: *mut MutableSubtree,
    symbol: TSSymbol,
    language: *const TSLanguage,
) {
    unsafe {
        let metadata = ts_language_symbol_metadata(language, symbol);
        if (*this).data.is_inline() {
            ts_assert!(symbol < u16::from(u8::MAX));
            (*this).data.set_symbol(symbol as u8);
            (*this).data.set_named(metadata.named);
            (*this).data.set_visible(metadata.visible);
        } else {
            (*(*this).ptr).symbol = symbol;
            (*(*this).ptr).set_named(metadata.named);
            (*(*this).ptr).set_visible(metadata.visible);
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_subtree_new_error(
    pool: *mut SubtreePool,
    lookahead_char: i32,
    padding: Length,
    size: Length,
    bytes_scanned: u32,
    parse_state: TSStateId,
    language: *const TSLanguage,
) -> Subtree {
    unsafe {
        let result = ts_subtree_new_leaf(
            pool,
            ts_builtin_sym_error,
            padding,
            size,
            bytes_scanned,
            parse_state,
            false,
            false,
            false,
            language,
        );
        let data = result.ptr as *mut SubtreeHeapData;
        (*data).set_fragile_left(true);
        (*data).set_fragile_right(true);
        (*data).u.lookahead_char = lookahead_char;
        result
    }
}

/// Clones a subtree (heap form only).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_subtree_clone(this: Subtree) -> MutableSubtree {
    unsafe {
        let child_count = (*this.ptr).child_count;
        let alloc_size = ts_subtree_alloc_size(child_count);
        let new_children = ts_malloc(alloc_size) as *mut Subtree;
        let old_children = ts_subtree_children(this);
        ptr::copy_nonoverlapping(
            old_children as *const u8,
            new_children as *mut u8,
            alloc_size,
        );
        let result = new_children.add(child_count as usize) as *mut SubtreeHeapData;
        if child_count > 0 {
            for i in 0..child_count {
                ts_subtree_retain(*new_children.add(i as usize));
            }
        } else if (*this.ptr).has_external_tokens() {
            (*result).u.external_scanner_state =
                ts_external_scanner_state_copy(&(*this.ptr).u.external_scanner_state);
        }
        (*result).ref_count = 1;
        MutableSubtree { ptr: result }
    }
}

/// Takes ownership of `this`; copies it if it has more than one owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_subtree_make_mut(
    pool: *mut SubtreePool,
    this: Subtree,
) -> MutableSubtree {
    unsafe {
        if this.data.is_inline() {
            return MutableSubtree { data: this.data };
        }
        if (*this.ptr).ref_count == 1 {
            return ts_subtree_to_mut_unsafe(this);
        }
        let result = ts_subtree_clone(this);
        ts_subtree_release(pool, this);
        result
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_subtree_compress(
    this: MutableSubtree,
    count: u32,
    language: *const TSLanguage,
    stack: *mut MutableSubtreeArray,
) {
    unsafe {
        let initial_stack_size = (*stack).size;

        let mut tree = this;
        let symbol = (*tree.ptr).symbol;
        for _ in 0..count {
            if (*tree.ptr).ref_count > 1 || (*tree.ptr).child_count < 2 {
                break;
            }

            let child = ts_subtree_to_mut_unsafe(*ts_subtree_children(ts_subtree_from_mut(tree)));
            if child.data.is_inline()
                || (*child.ptr).child_count < 2
                || (*child.ptr).ref_count > 1
                || (*child.ptr).symbol != symbol
            {
                break;
            }

            let grandchild =
                ts_subtree_to_mut_unsafe(*ts_subtree_children(ts_subtree_from_mut(child)));
            if grandchild.data.is_inline()
                || (*grandchild.ptr).child_count < 2
                || (*grandchild.ptr).ref_count > 1
                || (*grandchild.ptr).symbol != symbol
            {
                break;
            }

            *ts_subtree_children(ts_subtree_from_mut(tree)) = ts_subtree_from_mut(grandchild);
            *ts_subtree_children(ts_subtree_from_mut(child)) =
                *ts_subtree_children(ts_subtree_from_mut(grandchild))
                    .add((*grandchild.ptr).child_count as usize - 1);
            *ts_subtree_children(ts_subtree_from_mut(grandchild))
                .add((*grandchild.ptr).child_count as usize - 1) = ts_subtree_from_mut(child);
            (*stack).push(tree);
            tree = grandchild;
        }

        while (*stack).size > initial_stack_size {
            tree = (*stack).pop();
            let child = ts_subtree_to_mut_unsafe(*ts_subtree_children(ts_subtree_from_mut(tree)));
            let grandchild = ts_subtree_to_mut_unsafe(
                *ts_subtree_children(ts_subtree_from_mut(child))
                    .add((*child.ptr).child_count as usize - 1),
            );
            ts_subtree_summarize_children(grandchild, language);
            ts_subtree_summarize_children(child, language);
            ts_subtree_summarize_children(tree, language);
        }
    }
}

/// The part of an error node's cost that penalizes the extent it spans.
fn ts_subtree_error_extent_cost(size: Length) -> u32 {
    ERROR_COST_PER_RECOVERY
        .wrapping_add(ERROR_COST_PER_SKIPPED_CHAR.wrapping_mul(size.bytes))
        .wrapping_add(ERROR_COST_PER_SKIPPED_LINE.wrapping_mul(size.extent.row))
}

/// Assigns all of the node's properties that depend on its children.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_subtree_summarize_children(
    this: MutableSubtree,
    language: *const TSLanguage,
) {
    unsafe {
        ts_assert!(!this.data.is_inline());
        let p = &mut *this.ptr;

        p.u.node.named_child_count = 0;
        p.u.node.visible_child_count = 0;
        p.error_cost = 0;
        p.u.node.repeat_depth = 0;
        p.u.node.visible_descendant_count = 0;
        p.set_has_external_tokens(false);
        p.set_depends_on_column(false);
        p.set_has_external_scanner_state_change(false);
        p.u.node.dynamic_precedence = 0;

        let mut structural_index = 0u32;
        let alias_sequence =
            ts_language_alias_sequence(language, u32::from(p.u.node.production_id));
        let mut lookahead_end_byte = 0u32;

        let children = ts_subtree_children(ts_subtree_from_mut(this)) as *const Subtree;
        for i in 0..p.child_count {
            let child = *children.add(i as usize);

            if (i == 0 || p.size.extent.row == 0) && ts_subtree_depends_on_column(child) {
                p.set_depends_on_column(true);
            }

            if ts_subtree_has_external_scanner_state_change(child) {
                p.set_has_external_scanner_state_change(true);
            }

            if i == 0 {
                p.padding = ts_subtree_padding(child);
                p.size = ts_subtree_size(child);
            } else {
                p.size = length_add(p.size, ts_subtree_total_size(child));
            }

            let child_lookahead_end_byte = p
                .padding
                .bytes
                .wrapping_add(p.size.bytes)
                .wrapping_add(ts_subtree_lookahead_bytes(child));
            if child_lookahead_end_byte > lookahead_end_byte {
                lookahead_end_byte = child_lookahead_end_byte;
            }

            let grandchild_count = ts_subtree_child_count(child);
            if ts_subtree_symbol(child) == ts_builtin_sym_error_repeat {
                // Refund an `_ERROR` child's extent penalty, which this node re-charges below.
                let extent_cost = ts_subtree_error_extent_cost(ts_subtree_size(child));
                ts_assert!(ts_subtree_error_cost(child) >= extent_cost);
                p.error_cost = p
                    .error_cost
                    .wrapping_add(ts_subtree_error_cost(child).wrapping_sub(extent_cost));
            } else {
                p.error_cost = p.error_cost.wrapping_add(ts_subtree_error_cost(child));
                if (p.symbol == ts_builtin_sym_error || p.symbol == ts_builtin_sym_error_repeat)
                    && !ts_subtree_extra(child)
                    && !(ts_subtree_is_error(child) && grandchild_count == 0)
                {
                    if ts_subtree_visible(child) {
                        p.error_cost = p.error_cost.wrapping_add(ERROR_COST_PER_SKIPPED_TREE);
                    } else if grandchild_count > 0 {
                        p.error_cost = p.error_cost.wrapping_add(
                            ERROR_COST_PER_SKIPPED_TREE
                                .wrapping_mul((*child.ptr).u.node.visible_child_count),
                        );
                    }
                }
            }

            p.u.node.dynamic_precedence =
                p.u.node
                    .dynamic_precedence
                    .wrapping_add(ts_subtree_dynamic_precedence(child));
            p.u.node.visible_descendant_count =
                p.u.node
                    .visible_descendant_count
                    .wrapping_add(ts_subtree_visible_descendant_count(child));

            if !ts_subtree_extra(child)
                && ts_subtree_symbol(child) != 0
                && !alias_sequence.is_null()
                && *alias_sequence.add(structural_index as usize) != 0
            {
                p.u.node.visible_descendant_count =
                    p.u.node.visible_descendant_count.wrapping_add(1);
                p.u.node.visible_child_count = p.u.node.visible_child_count.wrapping_add(1);
                if ts_language_symbol_metadata(
                    language,
                    *alias_sequence.add(structural_index as usize),
                )
                .named
                {
                    p.u.node.named_child_count = p.u.node.named_child_count.wrapping_add(1);
                }
            } else if ts_subtree_visible(child) {
                p.u.node.visible_descendant_count =
                    p.u.node.visible_descendant_count.wrapping_add(1);
                p.u.node.visible_child_count = p.u.node.visible_child_count.wrapping_add(1);
                if ts_subtree_named(child) {
                    p.u.node.named_child_count = p.u.node.named_child_count.wrapping_add(1);
                }
            } else if grandchild_count > 0 {
                p.u.node.visible_child_count =
                    p.u.node
                        .visible_child_count
                        .wrapping_add((*child.ptr).u.node.visible_child_count);
                p.u.node.named_child_count =
                    p.u.node
                        .named_child_count
                        .wrapping_add((*child.ptr).u.node.named_child_count);
            }

            if ts_subtree_has_external_tokens(child) {
                p.set_has_external_tokens(true);
            }

            if ts_subtree_is_error(child) {
                p.set_fragile_left(true);
                p.set_fragile_right(true);
                p.parse_state = TS_TREE_STATE_NONE;
            }

            if !ts_subtree_extra(child) {
                structural_index += 1;
            }
        }

        p.lookahead_bytes = lookahead_end_byte
            .wrapping_sub(p.size.bytes)
            .wrapping_sub(p.padding.bytes);

        if p.symbol == ts_builtin_sym_error || p.symbol == ts_builtin_sym_error_repeat {
            p.error_cost = p
                .error_cost
                .wrapping_add(ts_subtree_error_extent_cost(p.size));
        }

        if p.child_count > 0 {
            let first_child = *children;
            let last_child = *children.add(p.child_count as usize - 1);

            p.u.node.first_leaf.symbol = ts_subtree_leaf_symbol(first_child);
            p.u.node.first_leaf.parse_state = ts_subtree_leaf_parse_state(first_child);

            if ts_subtree_fragile_left(first_child) {
                p.set_fragile_left(true);
            }
            if ts_subtree_fragile_right(last_child) {
                p.set_fragile_right(true);
            }

            if p.child_count >= 2
                && !p.visible()
                && !p.named()
                && ts_subtree_symbol(first_child) == p.symbol
            {
                if ts_subtree_repeat_depth(first_child) > ts_subtree_repeat_depth(last_child) {
                    p.u.node.repeat_depth = (ts_subtree_repeat_depth(first_child) + 1) as u16;
                } else {
                    p.u.node.repeat_depth = (ts_subtree_repeat_depth(last_child) + 1) as u16;
                }
            }
        }
    }
}

/// Creates a new parent node with the given children, taking ownership of the children array.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_subtree_new_node(
    symbol: TSSymbol,
    children: *mut SubtreeArray,
    production_id: u32,
    language: *const TSLanguage,
) -> MutableSubtree {
    unsafe {
        let metadata = ts_language_symbol_metadata(language, symbol);
        let fragile = symbol == ts_builtin_sym_error || symbol == ts_builtin_sym_error_repeat;

        // Allocate the node's data at the end of the array of children.
        let new_byte_size = ts_subtree_alloc_size((*children).size);
        if ((*children).capacity as usize) * size_of::<Subtree>() < new_byte_size {
            (*children).contents =
                ts_realloc((*children).contents as *mut c_void, new_byte_size) as *mut Subtree;
            (*children).capacity = (new_byte_size / size_of::<Subtree>()) as u32;
        }
        let data = (*children).contents.add((*children).size as usize) as *mut SubtreeHeapData;

        let mut value = SubtreeHeapData::zeroed();
        value.ref_count = 1;
        value.symbol = symbol;
        value.child_count = (*children).size;
        value.set_visible(metadata.visible);
        value.set_named(metadata.named);
        value.set_has_changes(false);
        value.set_has_external_scanner_state_change(false);
        value.set_fragile_left(fragile);
        value.set_fragile_right(fragile);
        value.set_is_keyword(false);
        value.u.node.visible_descendant_count = 0;
        value.u.node.production_id = production_id as u16;
        ptr::write(data, value);
        let result = MutableSubtree { ptr: data };
        ts_subtree_summarize_children(result, language);
        result
    }
}

/// Creates a new error node containing the given children.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_subtree_new_error_node(
    children: *mut SubtreeArray,
    extra: bool,
    language: *const TSLanguage,
) -> Subtree {
    unsafe {
        let result = ts_subtree_new_node(ts_builtin_sym_error, children, 0, language);
        (*result.ptr).set_extra(extra);
        ts_subtree_from_mut(result)
    }
}

/// Creates a new "missing leaf" node.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_subtree_new_missing_leaf(
    pool: *mut SubtreePool,
    symbol: TSSymbol,
    state: TSStateId,
    padding: Length,
    lookahead_bytes: u32,
    language: *const TSLanguage,
) -> Subtree {
    unsafe {
        let mut result = ts_subtree_new_leaf(
            pool,
            symbol,
            padding,
            length_zero(),
            lookahead_bytes,
            state,
            false,
            false,
            false,
            language,
        );
        if result.data.is_inline() {
            result.data.set_is_missing(true);
        } else {
            (*(result.ptr as *mut SubtreeHeapData)).set_is_missing(true);
        }
        result
    }
}

#[inline]
unsafe fn ref_count<'a>(p: *const SubtreeHeapData) -> &'a AtomicU32 {
    // SAFETY: `ref_count` is the first, 4-aligned u32 field; atomic ops model C's `atomic_*`.
    unsafe { &*(p as *const AtomicU32) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_subtree_retain(this: Subtree) {
    unsafe {
        if this.data.is_inline() {
            return;
        }
        ts_assert!(ref_count(this.ptr).load(Ordering::Relaxed) > 0);
        ref_count(this.ptr).fetch_add(1, Ordering::SeqCst);
        ts_assert!(ref_count(this.ptr).load(Ordering::Relaxed) != 0);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_subtree_release(pool: *mut SubtreePool, this: Subtree) {
    unsafe {
        if this.data.is_inline() {
            return;
        }
        (*pool).tree_stack.clear();

        ts_assert!(ref_count(this.ptr).load(Ordering::Relaxed) > 0);
        if ref_count(this.ptr).fetch_sub(1, Ordering::SeqCst) == 1 {
            (*pool).tree_stack.push(ts_subtree_to_mut_unsafe(this));
        }

        while (*pool).tree_stack.size > 0 {
            let tree = (*pool).tree_stack.pop();
            if (*tree.ptr).child_count > 0 {
                let children = ts_subtree_children(ts_subtree_from_mut(tree));
                for i in 0..(*tree.ptr).child_count {
                    let child = *children.add(i as usize);
                    if child.data.is_inline() {
                        continue;
                    }
                    ts_assert!(ref_count(child.ptr).load(Ordering::Relaxed) > 0);
                    if ref_count(child.ptr).fetch_sub(1, Ordering::SeqCst) == 1 {
                        (*pool).tree_stack.push(ts_subtree_to_mut_unsafe(child));
                    }
                }
                ts_free(children as *mut c_void);
            } else {
                if (*tree.ptr).has_external_tokens() {
                    ts_external_scanner_state_delete(&mut (*tree.ptr).u.external_scanner_state);
                }
                ts_subtree_pool_free(pool, tree.ptr);
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_subtree_compare(
    mut left: Subtree,
    mut right: Subtree,
    pool: *mut SubtreePool,
) -> c_int {
    unsafe {
        (*pool).tree_stack.push(ts_subtree_to_mut_unsafe(left));
        (*pool).tree_stack.push(ts_subtree_to_mut_unsafe(right));

        while (*pool).tree_stack.size > 0 {
            right = ts_subtree_from_mut((*pool).tree_stack.pop());
            left = ts_subtree_from_mut((*pool).tree_stack.pop());

            let mut result = 0;
            if ts_subtree_symbol(left) < ts_subtree_symbol(right) {
                result = -1;
            } else if ts_subtree_symbol(right) < ts_subtree_symbol(left) {
                result = 1;
            } else if ts_subtree_child_count(left) < ts_subtree_child_count(right) {
                result = -1;
            } else if ts_subtree_child_count(right) < ts_subtree_child_count(left) {
                result = 1;
            }
            if result != 0 {
                (*pool).tree_stack.clear();
                return result;
            }

            let mut i = ts_subtree_child_count(left);
            while i > 0 {
                let left_child = *ts_subtree_children(left).add(i as usize - 1);
                let right_child = *ts_subtree_children(right).add(i as usize - 1);
                (*pool)
                    .tree_stack
                    .push(ts_subtree_to_mut_unsafe(left_child));
                (*pool)
                    .tree_stack
                    .push(ts_subtree_to_mut_unsafe(right_child));
                i -= 1;
            }
        }
        0
    }
}

unsafe fn ts_subtree_set_has_changes(this: *mut MutableSubtree) {
    unsafe {
        if (*this).data.is_inline() {
            (*this).data.set_has_changes(true);
        } else {
            (*(*this).ptr).set_has_changes(true);
        }
    }
}

#[derive(Clone, Copy)]
struct Edit {
    start: Length,
    old_end: Length,
    new_end: Length,
}

#[derive(Clone, Copy)]
struct EditEntry {
    tree: *mut Subtree,
    edit: Edit,
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_subtree_edit(
    mut this: Subtree,
    input_edit: *const TSInputEdit,
    pool: *mut SubtreePool,
) -> Subtree {
    unsafe {
        let ie = &*input_edit;
        let mut stack: Array<EditEntry> = Array::new();
        stack.push(EditEntry {
            tree: &mut this,
            edit: Edit {
                start: Length {
                    bytes: ie.start_byte,
                    extent: ie.start_point,
                },
                old_end: Length {
                    bytes: ie.old_end_byte,
                    extent: ie.old_end_point,
                },
                new_end: Length {
                    bytes: ie.new_end_byte,
                    extent: ie.new_end_point,
                },
            },
        });

        while stack.size != 0 {
            let entry = stack.pop();
            let mut edit = entry.edit;
            let is_noop =
                edit.old_end.bytes == edit.start.bytes && edit.new_end.bytes == edit.start.bytes;
            let is_pure_insertion = edit.old_end.bytes == edit.start.bytes;
            let parent_depends_on_column = ts_subtree_depends_on_column(*entry.tree);
            let column_shifted = edit.new_end.extent.column != edit.old_end.extent.column;

            let mut size = ts_subtree_size(*entry.tree);
            let mut padding = ts_subtree_padding(*entry.tree);
            let total_size = length_add(padding, size);
            let lookahead_bytes = ts_subtree_lookahead_bytes(*entry.tree);
            let end_byte = total_size.bytes.wrapping_add(lookahead_bytes);
            if edit.start.bytes > end_byte || (is_noop && edit.start.bytes == end_byte) {
                continue;
            }

            // An edit entirely within the space before this subtree shifts it without resizing.
            if edit.old_end.bytes <= padding.bytes {
                padding = length_add(edit.new_end, length_sub(padding, edit.old_end));
            }
            // An edit that starts before this subtree and extends into it shrinks its content.
            else if edit.start.bytes < padding.bytes {
                size = length_saturating_sub(size, length_sub(edit.old_end, padding));
                padding = edit.new_end;
            }
            // An edit within this subtree resizes it.
            else if edit.start.bytes < total_size.bytes
                || (edit.start.bytes == total_size.bytes && is_pure_insertion)
            {
                size = length_add(
                    length_sub(edit.new_end, padding),
                    length_saturating_sub(total_size, edit.old_end),
                );
            }

            let mut result = ts_subtree_make_mut(pool, *entry.tree);
            if result.data.is_inline() {
                if ts_subtree_can_inline(padding, size, lookahead_bytes) {
                    result.data.set_padding_bytes(padding.bytes as u8);
                    result.data.set_padding_rows(padding.extent.row as u8 & 0xf);
                    result.data.set_padding_columns(padding.extent.column as u8);
                    result.data.set_size_bytes(size.bytes as u8);
                } else {
                    let data = ts_subtree_pool_allocate(pool);
                    let mut value = SubtreeHeapData::zeroed();
                    value.ref_count = 1;
                    value.padding = padding;
                    value.size = size;
                    value.lookahead_bytes = lookahead_bytes;
                    value.error_cost = 0;
                    value.child_count = 0;
                    value.symbol = TSSymbol::from(result.data.symbol());
                    value.parse_state = result.data.parse_state();
                    value.set_visible(result.data.visible());
                    value.set_named(result.data.named());
                    value.set_extra(result.data.extra());
                    value.set_fragile_left(false);
                    value.set_fragile_right(false);
                    value.set_has_changes(false);
                    value.set_has_external_tokens(false);
                    value.set_depends_on_column(result.data.depends_on_column());
                    value.set_is_missing(result.data.is_missing());
                    value.set_is_keyword(result.data.is_keyword());
                    ptr::write(data, value);
                    result.ptr = data;
                }
            } else {
                (*result.ptr).padding = padding;
                (*result.ptr).size = size;
            }

            ts_subtree_set_has_changes(&mut result);
            *entry.tree = ts_subtree_from_mut(result);

            let mut child_right = length_zero();
            let n = ts_subtree_child_count(*entry.tree);
            for i in 0..n {
                let child = ts_subtree_children(*entry.tree).add(i as usize);
                let child_size = ts_subtree_total_size(*child);
                let child_left = child_right;
                child_right = length_add(child_left, child_size);

                // A child that ends before the edit is not affected.
                if child_right
                    .bytes
                    .wrapping_add(ts_subtree_lookahead_bytes(*child))
                    < edit.start.bytes
                {
                    continue;
                }

                // Keep editing children until one starts after the edit (and, for column-dependent
                // nodes, until a line break).
                if (child_left.bytes > edit.old_end.bytes
                    || (child_left.bytes == edit.old_end.bytes && child_size.bytes > 0 && i > 0))
                    && (!parent_depends_on_column || child_left.extent.row > padding.extent.row)
                    && (!ts_subtree_depends_on_column(*child)
                        || !column_shifted
                        || child_left.extent.row > edit.old_end.extent.row)
                {
                    break;
                }

                // Transform the edit into the child's coordinate space.
                let mut child_edit = Edit {
                    start: length_saturating_sub(edit.start, child_left),
                    old_end: length_saturating_sub(edit.old_end, child_left),
                    new_end: length_saturating_sub(edit.new_end, child_left),
                };

                // Inserted text applies to the *first* child that touches the edit; later children
                // are only shrunk to compensate.
                if child_right.bytes > edit.start.bytes
                    || (child_right.bytes == edit.start.bytes && is_pure_insertion)
                {
                    edit.new_end = edit.start;
                } else {
                    // Children before the edit are not reshaped by it.
                    child_edit.old_end = child_edit.start;
                    child_edit.new_end = child_edit.start;
                }

                stack.push(EditEntry {
                    tree: child,
                    edit: child_edit,
                });
            }
        }

        stack.delete();
        this
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_subtree_last_external_token(mut tree: Subtree) -> Subtree {
    unsafe {
        if !ts_subtree_has_external_tokens(tree) {
            return NULL_SUBTREE;
        }
        while (*tree.ptr).child_count > 0 {
            let mut i = (*tree.ptr).child_count - 1;
            while i.wrapping_add(1) > 0 {
                let child = *ts_subtree_children(tree).add(i as usize);
                if ts_subtree_has_external_tokens(child) {
                    tree = child;
                    break;
                }
                i = i.wrapping_sub(1);
            }
        }
        tree
    }
}

// ---- Printing ----

/// Which field name a node is printed with: none, the root marker, or a name from the language.
#[derive(Clone, Copy)]
enum FieldName {
    None,
    Root,
    Named(*const c_char),
}

fn cstr_or_null(p: *const c_char) -> Vec<u8> {
    if p.is_null() {
        b"(null)".to_vec()
    } else {
        // SAFETY: symbol and field names are NUL-terminated strings from the language tables.
        unsafe { CStr::from_ptr(p) }.to_bytes().to_vec()
    }
}

fn write_char(out: &mut Vec<u8>, chr: i32) {
    match chr {
        -1 => out.extend_from_slice(b"INVALID"),
        0 => out.extend_from_slice(b"'\\0'"),
        10 => out.extend_from_slice(b"'\\n'"),
        9 => out.extend_from_slice(b"'\\t'"),
        13 => out.extend_from_slice(b"'\\r'"),
        c if 0 < c && c < 128 && (32..127).contains(&c) => {
            out.extend_from_slice(b"'");
            out.push(c as u8);
            out.extend_from_slice(b"'");
        }
        c => out.extend_from_slice(c.to_string().as_bytes()),
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn write_to_string(
    this: Subtree,
    out: &mut Vec<u8>,
    language: *const TSLanguage,
    include_all: bool,
    alias_symbol: TSSymbol,
    alias_is_named: bool,
    field_name: FieldName,
) {
    unsafe {
        if this.ptr.is_null() {
            out.extend_from_slice(b"(NULL)");
            return;
        }

        let is_root = matches!(field_name, FieldName::Root);
        let is_visible = include_all
            || ts_subtree_missing(this)
            || (if alias_symbol != 0 {
                alias_is_named
            } else {
                ts_subtree_visible(this) && ts_subtree_named(this)
            });

        if is_visible {
            if !is_root {
                out.push(b' ');
                if let FieldName::Named(name) = field_name {
                    out.extend_from_slice(&cstr_or_null(name));
                    out.extend_from_slice(b": ");
                }
            }

            if ts_subtree_is_error(this)
                && ts_subtree_child_count(this) == 0
                && (*this.ptr).size.bytes > 0
            {
                out.extend_from_slice(b"(UNEXPECTED ");
                write_char(out, (*this.ptr).u.lookahead_char);
            } else {
                let symbol = if alias_symbol != 0 {
                    alias_symbol
                } else {
                    ts_subtree_symbol(this)
                };
                let symbol_name = cstr_or_null(ts_language_symbol_name(language, symbol));
                if ts_subtree_missing(this) {
                    out.extend_from_slice(b"(MISSING ");
                    if alias_is_named || ts_subtree_named(this) {
                        out.extend_from_slice(&symbol_name);
                    } else {
                        out.push(b'"');
                        out.extend_from_slice(&symbol_name);
                        out.push(b'"');
                    }
                } else {
                    out.push(b'(');
                    out.extend_from_slice(&symbol_name);
                }
            }
        } else if is_root {
            let symbol = if alias_symbol != 0 {
                alias_symbol
            } else {
                ts_subtree_symbol(this)
            };
            let symbol_name = cstr_or_null(ts_language_symbol_name(language, symbol));
            if ts_subtree_child_count(this) > 0 {
                out.push(b'(');
                out.extend_from_slice(&symbol_name);
            } else if ts_subtree_named(this) {
                out.push(b'(');
                out.extend_from_slice(&symbol_name);
                out.push(b')');
            } else {
                out.extend_from_slice(b"(\"");
                out.extend_from_slice(&symbol_name);
                out.extend_from_slice(b"\")");
            }
        }

        if ts_subtree_child_count(this) != 0 {
            let production_id = u32::from((*this.ptr).u.node.production_id);
            let alias_sequence = ts_language_alias_sequence(language, production_id);
            let mut field_map: *const TSFieldMapEntry = ptr::null();
            let mut field_map_end: *const TSFieldMapEntry = ptr::null();
            ts_language_field_map(language, production_id, &mut field_map, &mut field_map_end);

            let mut structural_child_index = 0u32;
            for i in 0..(*this.ptr).child_count {
                let child = *ts_subtree_children(this).add(i as usize);
                if ts_subtree_extra(child) {
                    write_to_string(child, out, language, include_all, 0, false, FieldName::None);
                } else {
                    let subtree_alias_symbol = if alias_sequence.is_null() {
                        0
                    } else {
                        *alias_sequence.add(structural_child_index as usize)
                    };
                    let subtree_alias_is_named = if subtree_alias_symbol != 0 {
                        ts_language_symbol_metadata(language, subtree_alias_symbol).named
                    } else {
                        false
                    };

                    let mut child_field_name = if is_visible {
                        FieldName::None
                    } else {
                        field_name
                    };
                    let mut map = field_map;
                    while map < field_map_end {
                        if !(*map).inherited
                            && u32::from((*map).child_index) == structural_child_index
                        {
                            child_field_name = FieldName::Named(
                                *(*language).field_names.add(usize::from((*map).field_id)),
                            );
                            break;
                        }
                        map = map.add(1);
                    }

                    write_to_string(
                        child,
                        out,
                        language,
                        include_all,
                        subtree_alias_symbol,
                        subtree_alias_is_named,
                        child_field_name,
                    );
                    structural_child_index += 1;
                }
            }
        }

        if is_visible {
            out.push(b')');
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_subtree_string(
    this: Subtree,
    alias_symbol: TSSymbol,
    alias_is_named: bool,
    language: *const TSLanguage,
    include_all: bool,
) -> *mut c_char {
    unsafe {
        let mut out = Vec::new();
        write_to_string(
            this,
            &mut out,
            language,
            include_all,
            alias_symbol,
            alias_is_named,
            FieldName::Root,
        );
        let result = ts_malloc(out.len() + 1) as *mut u8;
        ptr::copy_nonoverlapping(out.as_ptr(), result, out.len());
        *result.add(out.len()) = 0;
        result as *mut c_char
    }
}

unsafe extern "C" {
    fn fprintf(f: *mut c_void, fmt: *const c_char, ...) -> c_int;
    fn fputc(c: c_int, f: *mut c_void) -> c_int;
    fn fputs(s: *const c_char, f: *mut c_void) -> c_int;
}

/// `ts_language_write_symbol_as_dot_string` from language.h.
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

unsafe fn print_dot_graph(
    this: *const Subtree,
    start_offset: u32,
    language: *const TSLanguage,
    alias_symbol: TSSymbol,
    f: *mut c_void,
) {
    unsafe {
        let s = *this;
        let subtree_symbol = ts_subtree_symbol(s);
        let symbol = if alias_symbol != 0 {
            alias_symbol
        } else {
            subtree_symbol
        };
        let end_offset = start_offset.wrapping_add(ts_subtree_total_bytes(s));
        fprintf(f, c"tree_%p [label=\"".as_ptr(), this as *const c_void);
        write_symbol_as_dot_string(language, f, symbol);
        fprintf(f, c"\"".as_ptr());

        if ts_subtree_child_count(s) == 0 {
            fprintf(f, c", shape=plaintext".as_ptr());
        }
        if ts_subtree_extra(s) {
            fprintf(f, c", fontcolor=gray".as_ptr());
        }
        if ts_subtree_has_changes(s) {
            fprintf(f, c", color=green, penwidth=2".as_ptr());
        }

        fprintf(
            f,
            c", tooltip=\"range: %u - %u\nstate: %d\nerror-cost: %u\nhas-changes: %u\ndepends-on-column: %u\ndescendant-count: %u\nrepeat-depth: %u\nlookahead-bytes: %u"
                .as_ptr(),
            start_offset,
            end_offset,
            c_int::from(ts_subtree_parse_state(s)),
            ts_subtree_error_cost(s),
            c_int::from(ts_subtree_has_changes(s)),
            c_int::from(ts_subtree_depends_on_column(s)),
            ts_subtree_visible_descendant_count(s),
            ts_subtree_repeat_depth(s),
            ts_subtree_lookahead_bytes(s),
        );

        if ts_subtree_is_error(s)
            && ts_subtree_child_count(s) == 0
            && (*s.ptr).u.lookahead_char != 0
        {
            fprintf(f, c"\ncharacter: '%c'".as_ptr(), (*s.ptr).u.lookahead_char);
        }

        fprintf(f, c"\"]\n".as_ptr());

        let mut child_start_offset = start_offset;
        let mut child_info_offset = u32::from((*language).max_alias_sequence_length)
            .wrapping_mul(u32::from(ts_subtree_production_id(s)));
        for i in 0..ts_subtree_child_count(s) {
            let child = ts_subtree_children(s).add(i as usize);
            let mut subtree_alias_symbol = 0;
            if !ts_subtree_extra(*child) && child_info_offset != 0 {
                subtree_alias_symbol = *(*language).alias_sequences.add(child_info_offset as usize);
                child_info_offset += 1;
            }
            print_dot_graph(child, child_start_offset, language, subtree_alias_symbol, f);
            fprintf(
                f,
                c"tree_%p -> tree_%p [tooltip=%u]\n".as_ptr(),
                this as *const c_void,
                child as *const c_void,
                i,
            );
            child_start_offset = child_start_offset.wrapping_add(ts_subtree_total_bytes(*child));
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_subtree_print_dot_graph(
    this: Subtree,
    language: *const TSLanguage,
    f: *mut c_void,
) {
    unsafe {
        fprintf(f, c"digraph tree {\n".as_ptr());
        fprintf(f, c"edge [arrowhead=none]\n".as_ptr());
        print_dot_graph(&this, 0, language, 0, f);
        fprintf(f, c"}\n".as_ptr());
    }
}

static EMPTY_STATE: ExternalScannerState = ExternalScannerState {
    data: ExternalScannerStateData {
        short_data: [0; 24],
    },
    length: 0,
};

// SAFETY: the empty state is immutable plain data containing no pointers.
unsafe impl Sync for ExternalScannerState {}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_subtree_external_scanner_state(
    this: Subtree,
) -> *const ExternalScannerState {
    unsafe {
        if !this.ptr.is_null()
            && !this.data.is_inline()
            && (*this.ptr).has_external_tokens()
            && (*this.ptr).child_count == 0
        {
            &(*this.ptr).u.external_scanner_state
        } else {
            &EMPTY_STATE
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_subtree_external_scanner_state_eq(
    this: Subtree,
    other: Subtree,
) -> bool {
    unsafe {
        let state_self = ts_subtree_external_scanner_state(this);
        let state_other = ts_subtree_external_scanner_state(other);
        ts_external_scanner_state_eq(
            state_self,
            ts_external_scanner_state_data(state_other),
            (*state_other).length,
        )
    }
}
