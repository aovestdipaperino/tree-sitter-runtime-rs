//! `ts_range_edit` and the range-array helpers against the C originals (renamed in the shim).
//! The tree-level changed-ranges comparison lives in tests/tree.rs.
mod common;
use common::Rng;
use tree_sitter_runtime_rs::{api::*, array::Array, get_changed_ranges::*};

unsafe extern "C" {
    fn c_ref_range_edit(r: *mut TSRange, e: *const TSInputEdit);
    fn c_ref_range_array_intersects(a: *const Array<TSRange>, i: u32, s: u32, e: u32) -> bool;
    fn c_ref_range_array_get_changed_ranges(
        o: *const TSRange,
        on: u32,
        n: *const TSRange,
        nn: u32,
        d: *mut Array<TSRange>,
    );
}

type Row = (u32, u32, u32, u32, u32, u32);
fn row(r: &TSRange) -> Row {
    (
        r.start_byte,
        r.end_byte,
        r.start_point.row,
        r.start_point.column,
        r.end_point.row,
        r.end_point.column,
    )
}

fn rand_u32(rng: &mut Rng) -> u32 {
    [0, 1, 2, 7, 40, 100, 1000, u32::MAX - 1, u32::MAX][rng.below(9) as usize]
}

fn point(rng: &mut Rng) -> TSPoint {
    TSPoint {
        row: rand_u32(rng) % 5,
        column: rand_u32(rng),
    }
}

#[test]
fn range_edit_matches_c() {
    let mut rng = Rng(77);
    for _ in 0..200_000 {
        let mut a = TSRange {
            start_point: point(&mut rng),
            end_point: point(&mut rng),
            start_byte: rand_u32(&mut rng),
            end_byte: rand_u32(&mut rng),
        };
        let mut b = a;
        let e = TSInputEdit {
            start_byte: rand_u32(&mut rng),
            old_end_byte: rand_u32(&mut rng),
            new_end_byte: rand_u32(&mut rng),
            start_point: point(&mut rng),
            old_end_point: point(&mut rng),
            new_end_point: point(&mut rng),
        };
        unsafe {
            ts_range_edit(&mut a, &e);
            c_ref_range_edit(&mut b, &e);
        }
        assert_eq!(row(&a), row(&b), "edit {e:?}");
    }
}

fn ranges(rng: &mut Rng) -> Vec<TSRange> {
    let mut v = Vec::new();
    let mut at = 0u32;
    for _ in 0..rng.below(5) {
        let s = at + rng.below(6) as u32;
        let e = s + rng.below(6) as u32;
        v.push(TSRange {
            start_byte: s,
            end_byte: e,
            start_point: TSPoint { row: 0, column: s },
            end_point: TSPoint { row: 0, column: e },
        });
        at = e;
    }
    v
}

#[test]
fn range_arrays_match_c() {
    let mut rng = Rng(1234);
    for _ in 0..50_000 {
        let (old, new) = (ranges(&mut rng), ranges(&mut rng));
        let (mut da, mut db): (Array<TSRange>, Array<TSRange>) = (Array::new(), Array::new());
        unsafe {
            ts_range_array_get_changed_ranges(
                old.as_ptr(),
                old.len() as u32,
                new.as_ptr(),
                new.len() as u32,
                &mut da,
            );
            c_ref_range_array_get_changed_ranges(
                old.as_ptr(),
                old.len() as u32,
                new.as_ptr(),
                new.len() as u32,
                &mut db,
            );
            let (x, y): (Vec<Row>, Vec<Row>) = (
                (0..da.size)
                    .map(|i| row(&*da.contents.add(i as usize)))
                    .collect(),
                (0..db.size)
                    .map(|i| row(&*db.contents.add(i as usize)))
                    .collect(),
            );
            assert_eq!(x, y);
            for _ in 0..4 {
                let (i, s, e) = (
                    rng.below(da.size as u64 + 1) as u32,
                    rng.below(30) as u32,
                    rng.below(30) as u32,
                );
                assert_eq!(
                    ts_range_array_intersects(&da, i, s, e),
                    c_ref_range_array_intersects(&db, i, s, e)
                );
            }
            da.delete();
            db.delete();
        }
    }
}
