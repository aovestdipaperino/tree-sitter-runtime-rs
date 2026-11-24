//! Shared helpers for the differential tests: a small RNG and subtree dumps taken both through the
//! Rust accessors and through the C header's accessors (via the test shim).
#![allow(dead_code)]
use tree_sitter_runtime_rs::{api::*, subtree::*};

pub const N_SLOTS: usize = 40;

unsafe extern "C" {
    pub fn shim_dump_node(s: Subtree, out: *mut u64);
    pub fn shim_child(s: Subtree, i: u32) -> Subtree;
}

pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

pub fn dump_c(s: Subtree) -> Vec<u64> {
    let mut all = Vec::new();
    fn go(s: Subtree, all: &mut Vec<u64>) {
        let mut rec = [0u64; N_SLOTS];
        unsafe { shim_dump_node(s, rec.as_mut_ptr()) };
        let n = rec[16] as u32;
        all.extend_from_slice(&rec);
        for i in 0..n {
            go(unsafe { shim_child(s, i) }, all);
        }
    }
    go(s, &mut all);
    all
}

pub fn dump_rust(s: Subtree) -> Vec<u64> {
    let mut all = Vec::new();
    fn go(s: Subtree, all: &mut Vec<u64>) {
        let heap = !s.is_inline();
        let p = ts_subtree_padding(s);
        let z = ts_subtree_size(s);
        let node = |f: fn(&NodeData) -> u64| -> u64 {
            if heap && ts_subtree_child_count(s) > 0 {
                f(unsafe { &(*s.ptr).u.node })
            } else {
                0
            }
        };
        let e = unsafe { ts_subtree_external_scanner_state(s) };
        let mut h = 1469598103934665603u64;
        unsafe {
            let d = ts_external_scanner_state_data(e) as *const u8;
            for i in 0..(*e).length {
                h = (h ^ u64::from(*d.add(i as usize))).wrapping_mul(1099511628211);
            }
        }
        let rec: [u64; N_SLOTS] = [
            u64::from(s.is_inline()),
            u64::from(ts_subtree_symbol(s)),
            u64::from(ts_subtree_parse_state(s)),
            u64::from(ts_subtree_visible(s)),
            u64::from(ts_subtree_named(s)),
            u64::from(ts_subtree_extra(s)),
            u64::from(ts_subtree_has_changes(s)),
            u64::from(ts_subtree_missing(s)),
            u64::from(ts_subtree_is_keyword(s)),
            u64::from(ts_subtree_lookahead_bytes(s)),
            u64::from(p.bytes),
            u64::from(p.extent.row),
            u64::from(p.extent.column),
            u64::from(z.bytes),
            u64::from(z.extent.row),
            u64::from(z.extent.column),
            u64::from(ts_subtree_child_count(s)),
            u64::from(ts_subtree_error_cost(s)),
            u64::from(ts_subtree_visible_child_count(s)),
            node(|n| u64::from(n.named_child_count)),
            u64::from(ts_subtree_visible_descendant_count(s)),
            i64::from(ts_subtree_dynamic_precedence(s)) as u64,
            u64::from(ts_subtree_repeat_depth(s)),
            u64::from(ts_subtree_production_id(s)),
            u64::from(ts_subtree_fragile_left(s)),
            u64::from(ts_subtree_fragile_right(s)),
            u64::from(ts_subtree_has_external_tokens(s)),
            u64::from(ts_subtree_has_external_scanner_state_change(s)),
            u64::from(ts_subtree_depends_on_column(s)),
            u64::from(ts_subtree_leaf_symbol(s)),
            u64::from(ts_subtree_leaf_parse_state(s)),
            if heap {
                u64::from(unsafe { (*s.ptr).ref_count })
            } else {
                0
            },
            u64::from(ts_subtree_total_bytes(s)),
            u64::from(ts_subtree_is_repetition(s)),
            u64::from(ts_subtree_is_fragile(s)),
            u64::from(ts_subtree_is_error(s)),
            u64::from(ts_subtree_is_eof(s)),
            u64::from(unsafe { (*e).length }),
            h,
            if heap && ts_subtree_child_count(s) == 0 && ts_subtree_is_error(s) {
                u64::from(unsafe { (*s.ptr).u.lookahead_char } as u32)
            } else {
                0
            },
        ];
        all.extend_from_slice(&rec);
        for i in 0..ts_subtree_child_count(s) {
            go(unsafe { *ts_subtree_children(s).add(i as usize) }, all);
        }
    }
    go(s, &mut all);
    all
}

pub fn language() -> *const TSLanguage {
    unsafe { (tree_sitter_lite_rs::javascript::LANGUAGE.into_raw())() as *const TSLanguage }
}
