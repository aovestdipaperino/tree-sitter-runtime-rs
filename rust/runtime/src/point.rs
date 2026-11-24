//! Port of `src/point.h` (inline helpers) and `src/point.c` (`ts_point_edit`).
//!
//! All arithmetic is unsigned and wraps exactly as the C does (ADR-004: reproduce, do not fix).
use crate::api::{TSInputEdit, TSPoint};

#[inline]
pub fn point_new(row: u32, column: u32) -> TSPoint {
    TSPoint { row, column }
}

#[inline]
pub fn point_add(a: TSPoint, b: TSPoint) -> TSPoint {
    if b.row > 0 {
        point_new(a.row.wrapping_add(b.row), b.column)
    } else {
        point_new(a.row, a.column.wrapping_add(b.column))
    }
}

#[inline]
pub fn point_sub(a: TSPoint, b: TSPoint) -> TSPoint {
    if a.row > b.row {
        point_new(a.row - b.row, a.column)
    } else {
        point_new(0, a.column.saturating_sub(b.column))
    }
}

#[inline]
pub fn point_lte(a: TSPoint, b: TSPoint) -> bool {
    a.row < b.row || (a.row == b.row && a.column <= b.column)
}

#[inline]
pub fn point_lt(a: TSPoint, b: TSPoint) -> bool {
    a.row < b.row || (a.row == b.row && a.column < b.column)
}

#[inline]
pub fn point_gt(a: TSPoint, b: TSPoint) -> bool {
    a.row > b.row || (a.row == b.row && a.column > b.column)
}

#[inline]
pub fn point_gte(a: TSPoint, b: TSPoint) -> bool {
    a.row > b.row || (a.row == b.row && a.column >= b.column)
}

#[inline]
pub fn point_eq(a: TSPoint, b: TSPoint) -> bool {
    a.row == b.row && a.column == b.column
}

/// Moves a position (byte offset and point) across an edit.
///
/// # Safety
/// `point`, `byte` and `edit` must be valid, aligned, non-null pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_point_edit(
    point: *mut TSPoint,
    byte: *mut u32,
    edit: *const TSInputEdit,
) {
    // SAFETY: the caller guarantees all three pointers are valid (they are in the C: internal
    // callers pass addresses of locals and of fields of live subtrees and edits).
    let (edit, mut start_byte, mut start_point) = unsafe { (&*edit, *byte, *point) };
    if start_byte >= edit.old_end_byte {
        start_byte = edit
            .new_end_byte
            .wrapping_add(start_byte.wrapping_sub(edit.old_end_byte));
        start_point = point_add(
            edit.new_end_point,
            point_sub(start_point, edit.old_end_point),
        );
    } else if start_byte > edit.start_byte {
        start_byte = edit.new_end_byte;
        start_point = edit.new_end_point;
    }
    // SAFETY: as above.
    unsafe {
        *point = start_point;
        *byte = start_byte;
    }
}
