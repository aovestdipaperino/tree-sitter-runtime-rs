//! The Rust ports of the header helpers and `ts_point_edit` against the C originals.
use tree_sitter_runtime_rs::{api::*, length::*, point::*};

unsafe extern "C" {
    fn shim_point_add(a: TSPoint, b: TSPoint) -> TSPoint;
    fn shim_point_sub(a: TSPoint, b: TSPoint) -> TSPoint;
    fn shim_point_lte(a: TSPoint, b: TSPoint) -> bool;
    fn shim_point_lt(a: TSPoint, b: TSPoint) -> bool;
    fn shim_point_gt(a: TSPoint, b: TSPoint) -> bool;
    fn shim_point_gte(a: TSPoint, b: TSPoint) -> bool;
    fn shim_point_eq(a: TSPoint, b: TSPoint) -> bool;
    fn shim_length_add(a: Length, b: Length) -> Length;
    fn shim_length_sub(a: Length, b: Length) -> Length;
    fn shim_length_min(a: Length, b: Length) -> Length;
    fn shim_length_zero() -> Length;
    fn shim_length_saturating_sub(a: Length, b: Length) -> Length;
    fn shim_length_is_undefined(a: Length) -> bool;
    fn shim_length_undefined() -> Length;
    fn shim_length_max() -> Length;
    fn shim_c_ts_point_edit(p: *mut TSPoint, b: *mut u32, e: *const TSInputEdit);
}

const V: [u32; 7] = [0, 1, 2, 7, 0x8000_0000, u32::MAX - 1, u32::MAX];

fn points() -> Vec<TSPoint> {
    V.iter()
        .flat_map(|&row| V.iter().map(move |&column| TSPoint { row, column }))
        .collect()
}

fn lengths() -> Vec<Length> {
    let mut out = Vec::new();
    for &bytes in &V {
        for p in points().into_iter().step_by(3) {
            out.push(Length { bytes, extent: p });
        }
    }
    out
}

#[test]
fn point_helpers_match_c() {
    let pts = points();
    for &a in &pts {
        for &b in &pts {
            unsafe {
                assert_eq!(point_add(a, b), shim_point_add(a, b), "add {a:?} {b:?}");
                assert_eq!(point_sub(a, b), shim_point_sub(a, b), "sub {a:?} {b:?}");
                assert_eq!(point_lte(a, b), shim_point_lte(a, b));
                assert_eq!(point_lt(a, b), shim_point_lt(a, b));
                assert_eq!(point_gt(a, b), shim_point_gt(a, b));
                assert_eq!(point_gte(a, b), shim_point_gte(a, b));
                assert_eq!(point_eq(a, b), shim_point_eq(a, b));
            }
        }
    }
}

#[test]
fn length_helpers_match_c() {
    let ls = lengths();
    for &a in &ls {
        unsafe { assert_eq!(length_is_undefined(a), shim_length_is_undefined(a)) };
        for &b in ls.iter().step_by(5) {
            unsafe {
                assert_eq!(length_add(a, b), shim_length_add(a, b));
                assert_eq!(length_sub(a, b), shim_length_sub(a, b));
                assert_eq!(length_min(a, b), shim_length_min(a, b));
                assert_eq!(
                    length_saturating_sub(a, b),
                    shim_length_saturating_sub(a, b)
                );
            }
        }
    }
    unsafe {
        assert_eq!(length_zero(), shim_length_zero());
        assert_eq!(LENGTH_UNDEFINED, shim_length_undefined());
        assert_eq!(LENGTH_MAX, shim_length_max());
    }
}

#[test]
fn ts_point_edit_matches_c() {
    let pts: Vec<TSPoint> = points().into_iter().step_by(4).collect();
    let mut n = 0u64;
    for &start in V.iter() {
        for &old_end in V.iter() {
            for &new_end in V.iter() {
                for (i, &sp) in pts.iter().enumerate().step_by(2) {
                    let edit = TSInputEdit {
                        start_byte: start,
                        old_end_byte: old_end,
                        new_end_byte: new_end,
                        start_point: sp,
                        old_end_point: pts[(i * 7 + 3) % pts.len()],
                        new_end_point: pts[(i * 11 + 5) % pts.len()],
                    };
                    for &byte in &V {
                        for &pt in pts.iter().step_by(2) {
                            let (mut p1, mut b1, mut p2, mut b2) = (pt, byte, pt, byte);
                            unsafe {
                                ts_point_edit(&mut p1, &mut b1, &edit);
                                shim_c_ts_point_edit(&mut p2, &mut b2, &edit);
                            }
                            assert_eq!((p1, b1), (p2, b2), "edit {edit:?} at {pt:?}/{byte}");
                            n += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(n > 100_000, "only {n} cases");
}
