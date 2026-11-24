//! The Rust mirrors of the C structs must have the C layout (ADR-011).
use core::mem::{align_of, offset_of, size_of};
use std::collections::BTreeMap;
use std::ffi::{CStr, c_char};
use tree_sitter_runtime_rs::{api::*, length::Length};

unsafe extern "C" {
    fn shim_layout_len() -> usize;
    fn shim_layout_key(i: usize) -> *const c_char;
    fn shim_layout_value(i: usize) -> usize;
}

fn c_table() -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    unsafe {
        for i in 0..shim_layout_len() {
            let k = CStr::from_ptr(shim_layout_key(i))
                .to_str()
                .unwrap()
                .to_string();
            assert!(m.insert(k, shim_layout_value(i)).is_none());
        }
    }
    m
}

macro_rules! rust_table {
    ($m:ident; $( $t:ident { $($f:ident),* } ),* $(,)?) => {$(
        $m.insert(format!("{}.size", stringify!($t)), size_of::<$t>());
        $m.insert(format!("{}.align", stringify!($t)), align_of::<$t>());
        $( $m.insert(format!("{}.{}", stringify!($t), stringify!($f)), offset_of!($t, $f)); )*
    )*};
}

#[test]
fn rust_layout_equals_c_layout() {
    let mut r: BTreeMap<String, usize> = BTreeMap::new();
    rust_table!(r;
        TSPoint { row, column },
        TSRange { start_point, end_point, start_byte, end_byte },
        TSInput { payload, read, encoding, decode },
        TSParseState { payload, current_byte_offset, has_error },
        TSParseOptions { payload, progress_callback },
        TSLogger { payload, log },
        TSInputEdit { start_byte, old_end_byte, new_end_byte, start_point, old_end_point, new_end_point },
        TSNode { context, id, tree },
        TSTreeCursor { tree, id, context },
        TSQueryCapture { node, index },
        TSQueryMatch { id, pattern_index, capture_count, captures },
        TSQueryPredicateStep { type_, value_id },
        TSQueryCursorState { payload, current_byte_offset },
        TSQueryCursorOptions { payload, progress_callback },
        Length { bytes, extent },
        TSLanguageMetadata { major_version, minor_version, patch_version },
    );
    // The C field is named `type`, a Rust keyword.
    let mut c = c_table();
    if let Some(v) = c.remove("TSQueryPredicateStep.type") {
        c.insert("TSQueryPredicateStep.type_".into(), v);
    }
    assert_eq!(c.len(), r.len(), "tables differ in size");
    for (k, v) in &c {
        assert_eq!(r.get(k), Some(v), "layout mismatch for {k}");
    }
}
