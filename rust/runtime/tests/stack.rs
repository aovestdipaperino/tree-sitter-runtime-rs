//! The stack port against the C original (src/stack.c compiled with renamed symbols).
//!
//! Random operation scripts run in lockstep on both stacks; after every step every observable
//! property of every version, and everything the operation returned, must be identical. The C
//! stack is fed subtrees from the C subtree implementation, the Rust stack from the Rust one.
#![cfg(feature = "reference")]
mod common;
use common::*;
use core::ffi::{c_char, c_int, c_void};
use tree_sitter_runtime_rs::{api::*, array::Array, length::*, stack::*, subtree::*};

unsafe extern "C" {
    fn c_ref_stack_new(p: *mut SubtreePool) -> *mut c_void;
    fn c_ref_stack_delete(s: *mut c_void);
    fn c_ref_stack_version_count(s: *const c_void) -> u32;
    fn c_ref_stack_halted_version_count(s: *mut c_void) -> u32;
    fn c_ref_stack_state(s: *const c_void, v: u32) -> u16;
    fn c_ref_stack_last_external_token(s: *const c_void, v: u32) -> Subtree;
    fn c_ref_stack_set_last_external_token(s: *mut c_void, v: u32, t: Subtree);
    fn c_ref_stack_position(s: *const c_void, v: u32) -> Length;
    fn c_ref_stack_push(s: *mut c_void, v: u32, t: Subtree, pending: bool, state: u16);
    fn c_ref_stack_pop_count(s: *mut c_void, v: u32, n: u32) -> StackSliceArray;
    fn c_ref_stack_pop_error(s: *mut c_void, v: u32) -> SubtreeArray;
    fn c_ref_stack_pop_pending(s: *mut c_void, v: u32) -> StackSliceArray;
    fn c_ref_stack_pop_all(s: *mut c_void, v: u32) -> StackSliceArray;
    fn c_ref_stack_node_count_since_error(s: *const c_void, v: u32) -> u32;
    fn c_ref_stack_dynamic_precedence(s: *mut c_void, v: u32) -> c_int;
    fn c_ref_stack_has_advanced_since_error(s: *const c_void, v: u32) -> bool;
    fn c_ref_stack_record_summary(s: *mut c_void, v: u32, d: u32);
    fn c_ref_stack_get_summary(s: *mut c_void, v: u32) -> *mut StackSummary;
    fn c_ref_stack_error_cost(s: *const c_void, v: u32) -> u32;
    fn c_ref_stack_merge(s: *mut c_void, a: u32, b: u32) -> bool;
    fn c_ref_stack_can_merge(s: *mut c_void, a: u32, b: u32) -> bool;
    fn c_ref_stack_resume(s: *mut c_void, v: u32) -> Subtree;
    fn c_ref_stack_pause(s: *mut c_void, v: u32, t: Subtree);
    fn c_ref_stack_halt(s: *mut c_void, v: u32);
    fn c_ref_stack_is_active(s: *const c_void, v: u32) -> bool;
    fn c_ref_stack_is_paused(s: *const c_void, v: u32) -> bool;
    fn c_ref_stack_is_halted(s: *const c_void, v: u32) -> bool;
    fn c_ref_stack_renumber_version(s: *mut c_void, a: u32, b: u32);
    fn c_ref_stack_swap_versions(s: *mut c_void, a: u32, b: u32);
    fn c_ref_stack_copy_version(s: *mut c_void, v: u32) -> u32;
    fn c_ref_stack_remove_version(s: *mut c_void, v: u32);
    fn c_ref_stack_clear(s: *mut c_void);
    fn c_ref_stack_print_dot_graph(s: *mut c_void, l: *const TSLanguage, f: *mut c_void) -> bool;

    fn c_ref_subtree_pool_new(cap: u32) -> SubtreePool;
    fn c_ref_subtree_pool_delete(p: *mut SubtreePool);
    #[allow(clippy::too_many_arguments)]
    fn c_ref_subtree_new_leaf(
        p: *mut SubtreePool,
        sym: u16,
        pad: Length,
        size: Length,
        la: u32,
        st: u16,
        ext: bool,
        col: bool,
        kw: bool,
        l: *const TSLanguage,
    ) -> Subtree;
    fn c_ref_subtree_new_error(
        p: *mut SubtreePool,
        c: i32,
        pad: Length,
        size: Length,
        scanned: u32,
        st: u16,
        l: *const TSLanguage,
    ) -> Subtree;
    fn c_ref_subtree_new_node(
        sym: u16,
        ch: *mut SubtreeArray,
        prod: u32,
        l: *const TSLanguage,
    ) -> MutableSubtree;
    fn c_ref_subtree_new_error_node(
        ch: *mut SubtreeArray,
        extra: bool,
        l: *const TSLanguage,
    ) -> Subtree;
    fn c_ref_subtree_make_mut(p: *mut SubtreePool, s: Subtree) -> MutableSubtree;
    fn c_ref_subtree_release(p: *mut SubtreePool, s: Subtree);
    fn c_ref_subtree_array_delete(p: *mut SubtreePool, a: *mut SubtreeArray);
    fn c_ref_external_scanner_state_init(s: *mut ExternalScannerState, d: *const c_char, n: u32);

    fn tmpfile() -> *mut c_void;
    fn rewind(f: *mut c_void);
    fn fread(buf: *mut c_void, size: usize, n: usize, f: *mut c_void) -> usize;
    fn fclose(f: *mut c_void) -> c_int;
}

#[derive(Clone, Debug)]
enum Spec {
    Null,
    ErrNode {
        kids: Vec<Spec>,
        extra: bool,
    },
    Leaf {
        sym: u16,
        pad: u32,
        size: u32,
        state: u16,
        extra: bool,
        ext: Option<Vec<c_char>>,
    },
    Err {
        ch: i32,
        size: u32,
    },
    Node {
        kids: Vec<Spec>,
        sym: u16,
        prec: i32,
    },
}

fn len(b: u32) -> Length {
    Length {
        bytes: b,
        extent: TSPoint { row: 0, column: b },
    }
}

fn spec(rng: &mut Rng, depth: u32, nsym: u64, allow_null: bool) -> Spec {
    match rng.below(10) {
        0 if allow_null => Spec::Null,
        4 if depth < 2 => Spec::ErrNode {
            kids: (0..rng.below(4))
                .map(|_| spec(rng, depth + 1, nsym, false))
                .collect(),
            extra: rng.below(4) == 0,
        },
        1 => Spec::Err {
            ch: [-1, 10, 65, 0x1f600][rng.below(4) as usize],
            size: rng.below(4) as u32,
        },
        2 | 3 if depth < 2 => Spec::Node {
            kids: (0..1 + rng.below(3))
                .map(|_| spec(rng, depth + 1, nsym, false))
                .collect(),
            sym: rng.below(nsym) as u16,
            prec: rng.below(5) as i32 - 2,
        },
        _ => Spec::Leaf {
            sym: rng.below(nsym) as u16,
            pad: rng.below(4) as u32,
            size: rng.below(5) as u32,
            state: rng.below(30) as u16,
            extra: rng.below(5) == 0,
            ext: (rng.below(6) == 0)
                .then(|| (0..rng.below(30)).map(|_| rng.below(3) as c_char).collect()),
        },
    }
}

/// Builds a subtree on one side; `c` selects the C subtree implementation.
unsafe fn build(s: &Spec, c: bool, pool: *mut SubtreePool, lang: *const TSLanguage) -> Subtree {
    unsafe {
        match s {
            Spec::Null => NULL_SUBTREE,
            Spec::Leaf {
                sym,
                pad,
                size,
                state,
                extra,
                ext,
            } => {
                let has_ext = ext.is_some();
                let t = if c {
                    c_ref_subtree_new_leaf(
                        pool,
                        *sym,
                        len(*pad),
                        len(*size),
                        0,
                        *state,
                        has_ext,
                        false,
                        false,
                        lang,
                    )
                } else {
                    ts_subtree_new_leaf(
                        pool,
                        *sym,
                        len(*pad),
                        len(*size),
                        0,
                        *state,
                        has_ext,
                        false,
                        false,
                        lang,
                    )
                };
                if let Some(data) = ext {
                    let p = t.ptr as *mut SubtreeHeapData;
                    if c {
                        c_ref_external_scanner_state_init(
                            &mut (*p).u.external_scanner_state,
                            data.as_ptr(),
                            data.len() as u32,
                        );
                    } else {
                        ts_external_scanner_state_init(
                            &mut (*p).u.external_scanner_state,
                            data.as_ptr(),
                            data.len() as u32,
                        );
                    }
                }
                if *extra {
                    let mut m = if c {
                        c_ref_subtree_make_mut(pool, t)
                    } else {
                        ts_subtree_make_mut(pool, t)
                    };
                    ts_subtree_set_extra(&mut m, true);
                    return ts_subtree_from_mut(m);
                }
                t
            }
            Spec::Err { ch, size } => {
                if c {
                    c_ref_subtree_new_error(pool, *ch, len(0), len(*size), 0, 3, lang)
                } else {
                    ts_subtree_new_error(pool, *ch, len(0), len(*size), 0, 3, lang)
                }
            }
            Spec::ErrNode { kids, extra } => {
                let mut arr: Array<Subtree> = Array::new();
                for k in kids {
                    arr.push(build(k, c, pool, lang));
                }
                if c {
                    c_ref_subtree_new_error_node(&mut arr, *extra, lang)
                } else {
                    ts_subtree_new_error_node(&mut arr, *extra, lang)
                }
            }
            Spec::Node { kids, sym, prec } => {
                let mut arr: Array<Subtree> = Array::new();
                for k in kids {
                    arr.push(build(k, c, pool, lang));
                }
                let m = if c {
                    c_ref_subtree_new_node(*sym, &mut arr, 0, lang)
                } else {
                    ts_subtree_new_node(*sym, &mut arr, 0, lang)
                };
                (*m.ptr).u.node.dynamic_precedence = *prec;
                ts_subtree_from_mut(m)
            }
        }
    }
}

struct Side {
    c: bool,
    stack: *mut c_void,
    pool: Box<SubtreePool>,
}

impl Side {
    unsafe fn new(c: bool) -> Side {
        unsafe {
            let mut pool = Box::new(if c {
                c_ref_subtree_pool_new(32)
            } else {
                ts_subtree_pool_new(32)
            });
            let stack = if c {
                c_ref_stack_new(&mut *pool)
            } else {
                ts_stack_new(&mut *pool) as *mut c_void
            };
            Side { c, stack, pool }
        }
    }
    fn p(&mut self) -> *mut SubtreePool {
        &mut *self.pool
    }
}

/// Dispatches a call to the Rust or the C stack.
macro_rules! st {
    ($side:expr, $rust:ident, $c:ident $(, $a:expr)*) => {
        if $side.c { $c($side.stack $(, $a)*) } else { $rust($side.stack as *mut Stack $(, $a)*) }
    };
}

unsafe fn release(side: &mut Side, t: Subtree) {
    unsafe {
        if t.ptr.is_null() {
            return;
        }
        if side.c {
            c_ref_subtree_release(side.p(), t)
        } else {
            ts_subtree_release(side.p(), t)
        }
    }
}

unsafe fn delete_array(side: &mut Side, a: &mut SubtreeArray) {
    unsafe {
        if side.c {
            c_ref_subtree_array_delete(side.p(), a)
        } else {
            ts_subtree_array_delete(side.p(), a)
        }
    }
}

unsafe fn dump(t: Subtree) -> Vec<u64> {
    if unsafe { t.ptr.is_null() } {
        vec![]
    } else {
        dump_c(t)
    }
}

/// Everything observable about slices, then frees their subtrees.
unsafe fn take_slices(side: &mut Side, mut sl: StackSliceArray) -> Vec<(u32, Vec<Vec<u64>>)> {
    unsafe {
        let mut out = Vec::new();
        for i in 0..sl.size {
            let s = &mut *sl.contents.add(i as usize);
            let subs = (0..s.subtrees.size)
                .map(|j| dump(*s.subtrees.contents.add(j as usize)))
                .collect();
            out.push((s.version, subs));
            delete_array(side, &mut s.subtrees);
        }
        let _ = &mut sl;
        out
    }
}

/// Pops from a merged version on both sides and compares what comes out: a merge that kept an
/// ambiguity shows up as several slices.
unsafe fn probe(r: &mut Side, c: &mut Side, v: u32, ctx: &str) {
    unsafe {
        let (a, b) = (
            ts_stack_pop_all(r.stack as *mut Stack, v),
            c_ref_stack_pop_all(c.stack, v),
        );
        assert_eq!(take_slices(r, a), take_slices(c, b), "{ctx} probe pop_all");
        let (a, b) = (
            ts_stack_pop_count(r.stack as *mut Stack, v, 1),
            c_ref_stack_pop_count(c.stack, v, 1),
        );
        assert_eq!(
            take_slices(r, a),
            take_slices(c, b),
            "{ctx} probe pop_count"
        );
    }
}

/// (position bytes, row, column, depth, state)
type SummaryRow = (u32, u32, u32, u32, u16);

#[derive(PartialEq, Debug)]
struct VersionView {
    state: u16,
    position: (u32, u32, u32),
    error_cost: u32,
    since_error: u32,
    dynamic: i32,
    flags: (bool, bool, bool, bool),
    last_external: Vec<u64>,
    summary: Option<Vec<SummaryRow>>,
}

unsafe fn view(side: &mut Side, v: u32) -> VersionView {
    unsafe {
        let pos = st!(side, ts_stack_position, c_ref_stack_position, v);
        let sm = st!(side, ts_stack_get_summary, c_ref_stack_get_summary, v);
        VersionView {
            state: st!(side, ts_stack_state, c_ref_stack_state, v),
            position: (pos.bytes, pos.extent.row, pos.extent.column),
            error_cost: st!(side, ts_stack_error_cost, c_ref_stack_error_cost, v),
            since_error: st!(
                side,
                ts_stack_node_count_since_error,
                c_ref_stack_node_count_since_error,
                v
            ),
            dynamic: st!(
                side,
                ts_stack_dynamic_precedence,
                c_ref_stack_dynamic_precedence,
                v
            ),
            flags: (
                st!(side, ts_stack_is_active, c_ref_stack_is_active, v),
                st!(side, ts_stack_is_paused, c_ref_stack_is_paused, v),
                st!(side, ts_stack_is_halted, c_ref_stack_is_halted, v),
                st!(
                    side,
                    ts_stack_has_advanced_since_error,
                    c_ref_stack_has_advanced_since_error,
                    v
                ),
            ),
            last_external: dump(st!(
                side,
                ts_stack_last_external_token,
                c_ref_stack_last_external_token,
                v
            )),
            summary: if sm.is_null() {
                None
            } else {
                Some(
                    (0..(*sm).size)
                        .map(|i| {
                            let e = *(*sm).contents.add(i as usize);
                            (
                                e.position.bytes,
                                e.position.extent.row,
                                e.position.extent.column,
                                e.depth,
                                e.state,
                            )
                        })
                        .collect(),
                )
            },
        }
    }
}

unsafe fn dot(side: &mut Side, lang: *const TSLanguage) -> String {
    unsafe {
        let f = tmpfile();
        if side.c {
            c_ref_stack_print_dot_graph(side.stack, lang, f);
        } else {
            ts_stack_print_dot_graph(side.stack as *mut Stack, lang, f);
        }
        let mut text = Vec::new();
        rewind(f);
        let mut buf = vec![0u8; 4096];
        loop {
            let n = fread(buf.as_mut_ptr() as *mut c_void, 1, buf.len(), f);
            if n == 0 {
                break;
            }
            text.extend_from_slice(&buf[..n]);
        }
        fclose(f);
        // Pointers differ between runs: number them in order of first appearance.
        let s = String::from_utf8_lossy(&text).into_owned();
        let mut seen: Vec<String> = Vec::new();
        let mut out = String::new();
        let b = s.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'0' && i + 1 < b.len() && b[i + 1] == b'x' {
                let mut j = i + 2;
                while j < b.len() && b[j].is_ascii_hexdigit() {
                    j += 1;
                }
                let tok = s[i..j].to_string();
                let idx = seen.iter().position(|t| *t == tok).unwrap_or_else(|| {
                    seen.push(tok);
                    seen.len() - 1
                });
                out.push_str(&format!("<ptr{idx}>"));
                i = j;
            } else {
                out.push(b[i] as char);
                i += 1;
            }
        }
        out
    }
}

#[test]
fn random_scripts_match_c() {
    let lang = language();
    let nsym = unsafe { (*lang).symbol_count } as u64;
    let mut steps = 0u64;
    let (mut merges, mut popped, mut summaries) = (0u32, 0u32, 0u32);
    for seed in 1..=1500u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        // Pushing a null subtree (error recovery) is only done in a third of the scripts. In the C,
        // merging two versions whose paths contain null links to the same node dereferences the null
        // subtree (`ts_subtree_dynamic_precedence`), so those scripts never merge.
        let allow_null = seed % 3 == 0;
        unsafe {
            let mut r = Side::new(false);
            let mut c = Side::new(true);
            for step in 0..70 {
                let count = st!(r, ts_stack_version_count, c_ref_stack_version_count);
                let op = rng.below(25);
                let v = rng.below(u64::from(count)) as u32;
                let ctx = format!("seed {seed} step {step} op {op} v {v}/{count}");
                if std::env::var_os("TRACE").is_some() {
                    eprintln!("{ctx}");
                }
                match op {
                    0..=6 => {
                        let sp = spec(&mut rng, 0, nsym, allow_null);
                        let pending = rng.below(2) == 0;
                        let state = rng.below(25) as u16;
                        let (a, b) = (
                            build(&sp, false, r.p(), lang),
                            build(&sp, true, c.p(), lang),
                        );
                        ts_stack_push(r.stack as *mut Stack, v, a, pending, state);
                        c_ref_stack_push(c.stack, v, b, pending, state);
                    }
                    7 | 8 => {
                        let k = rng.below(5) as u32;
                        let (a, b) = (
                            st!(r, ts_stack_pop_count, c_ref_stack_pop_count, v, k),
                            st!(c, ts_stack_pop_count, c_ref_stack_pop_count, v, k),
                        );
                        let (x, y) = (take_slices(&mut r, a), take_slices(&mut c, b));
                        assert_eq!(x, y, "{ctx} pop_count {k}");
                        popped += x.len() as u32;
                    }
                    9 => {
                        let (a, b) = (
                            ts_stack_pop_pending(r.stack as *mut Stack, v),
                            c_ref_stack_pop_pending(c.stack, v),
                        );
                        assert_eq!(
                            take_slices(&mut r, a),
                            take_slices(&mut c, b),
                            "{ctx} pop_pending"
                        );
                    }
                    10 => {
                        let (a, b) = (
                            ts_stack_pop_all(r.stack as *mut Stack, v),
                            c_ref_stack_pop_all(c.stack, v),
                        );
                        assert_eq!(
                            take_slices(&mut r, a),
                            take_slices(&mut c, b),
                            "{ctx} pop_all"
                        );
                    }
                    11 => {
                        let (mut a, mut b) = (
                            ts_stack_pop_error(r.stack as *mut Stack, v),
                            c_ref_stack_pop_error(c.stack, v),
                        );
                        let da: Vec<_> = (0..a.size)
                            .map(|j| dump(*a.contents.add(j as usize)))
                            .collect();
                        let db: Vec<_> = (0..b.size)
                            .map(|j| dump(*b.contents.add(j as usize)))
                            .collect();
                        assert_eq!(da, db, "{ctx} pop_error");
                        delete_array(&mut r, &mut a);
                        delete_array(&mut c, &mut b);
                    }
                    20..=22 if !allow_null => {
                        // Fork a version, push equivalent trees on both branches, merge them again:
                        // the usual shape of a GLR merge.
                        let (a, b) = (
                            ts_stack_copy_version(r.stack as *mut Stack, v),
                            c_ref_stack_copy_version(c.stack, v),
                        );
                        assert_eq!(a, b);
                        let state = rng.below(25) as u16;
                        // One time in three, an error leaf against an error node: the same symbol with
                        // and without an error cost.
                        let err_pair = rng.below(3) == 0;
                        for target in [v, a] {
                            let sp = if err_pair {
                                if target == v {
                                    Spec::Err { ch: 65, size: 1 }
                                } else {
                                    Spec::ErrNode {
                                        kids: vec![Spec::Leaf {
                                            sym: 3,
                                            pad: 0,
                                            size: 2,
                                            state: 1,
                                            extra: false,
                                            ext: None,
                                        }],
                                        extra: false,
                                    }
                                }
                            } else {
                                spec(&mut rng, 0, nsym, false)
                            };
                            let pending = rng.below(2) == 0;
                            let (x, y) = (
                                build(&sp, false, r.p(), lang),
                                build(&sp, true, c.p(), lang),
                            );
                            ts_stack_push(r.stack as *mut Stack, target, x, pending, state);
                            c_ref_stack_push(c.stack, target, y, pending, state);
                        }
                        let (x, y) = (
                            ts_stack_merge(r.stack as *mut Stack, v, a),
                            c_ref_stack_merge(c.stack, v, a),
                        );
                        assert_eq!(x, y, "{ctx} fork merge");
                        merges += u32::from(x);
                        if x {
                            probe(&mut r, &mut c, v, &ctx);
                        }
                    }
                    23 if !allow_null => {
                        // Many branches merged into one node: exercises the link limit.
                        let state = rng.below(25) as u16;
                        let k = 2 + rng.below(10) as usize;
                        let mut branches = vec![v];
                        for _ in 0..k {
                            let (a, b) = (
                                ts_stack_copy_version(r.stack as *mut Stack, v),
                                c_ref_stack_copy_version(c.stack, v),
                            );
                            assert_eq!(a, b);
                            branches.push(a);
                        }
                        let size = rng.below(3) as u32;
                        for (i, &target) in branches.iter().enumerate() {
                            // Distinct symbols so the links are not equivalent, equal sizes so the nodes can merge.
                            let sp = Spec::Leaf {
                                sym: (i as u16 * 3) % nsym as u16,
                                pad: 0,
                                size,
                                state: 1,
                                extra: false,
                                ext: None,
                            };
                            let (x, y) = (
                                build(&sp, false, r.p(), lang),
                                build(&sp, true, c.p(), lang),
                            );
                            ts_stack_push(r.stack as *mut Stack, target, x, false, state);
                            c_ref_stack_push(c.stack, target, y, false, state);
                        }
                        // Merge from the back so the version numbers stay valid.
                        for &w in branches.iter().skip(1).rev() {
                            let (x, y) = (
                                ts_stack_merge(r.stack as *mut Stack, v, w),
                                c_ref_stack_merge(c.stack, v, w),
                            );
                            assert_eq!(x, y, "{ctx} multi merge");
                            merges += u32::from(x);
                        }
                    }
                    24 if !allow_null => {
                        // Two levels: branches differ below, then push the same tree on top, so the
                        // merge recurses into the lower nodes.
                        let (a, b) = (
                            ts_stack_copy_version(r.stack as *mut Stack, v),
                            c_ref_stack_copy_version(c.stack, v),
                        );
                        assert_eq!(a, b);
                        let size = rng.below(3) as u32;
                        for (i, &target) in [v, a].iter().enumerate() {
                            let lower = Spec::Leaf {
                                sym: (i as u16 + 1) % nsym as u16,
                                pad: 0,
                                size,
                                state: 1,
                                extra: false,
                                ext: None,
                            };
                            let (x, y) = (
                                build(&lower, false, r.p(), lang),
                                build(&lower, true, c.p(), lang),
                            );
                            ts_stack_push(r.stack as *mut Stack, target, x, false, 7);
                            c_ref_stack_push(c.stack, target, y, false, 7);
                            let upper = Spec::Leaf {
                                sym: 5 % nsym as u16,
                                pad: 0,
                                size: 1,
                                state: 1,
                                extra: false,
                                ext: None,
                            };
                            let (x, y) = (
                                build(&upper, false, r.p(), lang),
                                build(&upper, true, c.p(), lang),
                            );
                            ts_stack_push(r.stack as *mut Stack, target, x, false, 9);
                            c_ref_stack_push(c.stack, target, y, false, 9);
                        }
                        let (x, y) = (
                            ts_stack_merge(r.stack as *mut Stack, v, a),
                            c_ref_stack_merge(c.stack, v, a),
                        );
                        assert_eq!(x, y, "{ctx} two-level merge");
                        merges += u32::from(x);
                        if x {
                            probe(&mut r, &mut c, v, &ctx);
                        }
                    }
                    12 if count > 1 && !allow_null => {
                        let w = rng.below(u64::from(count)) as u32;
                        if w != v {
                            let (a, b) = (
                                ts_stack_can_merge(r.stack as *mut Stack, v, w),
                                c_ref_stack_can_merge(c.stack, v, w),
                            );
                            assert_eq!(a, b, "{ctx} can_merge");
                            let (a, b) = (
                                ts_stack_merge(r.stack as *mut Stack, v, w),
                                c_ref_stack_merge(c.stack, v, w),
                            );
                            assert_eq!(a, b, "{ctx} merge");
                            merges += u32::from(a);
                        }
                    }
                    13 => {
                        let (a, b) = (
                            ts_stack_copy_version(r.stack as *mut Stack, v),
                            c_ref_stack_copy_version(c.stack, v),
                        );
                        assert_eq!(a, b);
                    }
                    14 if count > 1 => {
                        ts_stack_remove_version(r.stack as *mut Stack, v);
                        c_ref_stack_remove_version(c.stack, v);
                    }
                    15 if count > 1 => {
                        let w = rng.below(u64::from(count)) as u32;
                        let (hi, lo) = (v.max(w), v.min(w));
                        ts_stack_renumber_version(r.stack as *mut Stack, hi, lo);
                        c_ref_stack_renumber_version(c.stack, hi, lo);
                    }
                    16 if count > 1 => {
                        let w = rng.below(u64::from(count)) as u32;
                        ts_stack_swap_versions(r.stack as *mut Stack, v, w);
                        c_ref_stack_swap_versions(c.stack, v, w);
                    }
                    17 => {
                        // Pause, halt or resume a version.
                        match rng.below(3) {
                            0 if ts_stack_is_active(r.stack as *mut Stack, v) => {
                                let sp = Spec::Leaf {
                                    sym: 1,
                                    pad: 0,
                                    size: 1,
                                    state: 2,
                                    extra: false,
                                    ext: None,
                                };
                                ts_stack_pause(
                                    r.stack as *mut Stack,
                                    v,
                                    build(&sp, false, r.p(), lang),
                                );
                                c_ref_stack_pause(c.stack, v, build(&sp, true, c.p(), lang));
                            }
                            1 => {
                                ts_stack_halt(r.stack as *mut Stack, v);
                                c_ref_stack_halt(c.stack, v);
                            }
                            _ if ts_stack_is_paused(r.stack as *mut Stack, v) => {
                                let (a, b) = (
                                    ts_stack_resume(r.stack as *mut Stack, v),
                                    c_ref_stack_resume(c.stack, v),
                                );
                                assert_eq!(dump(a), dump(b), "{ctx} resume");
                                release(&mut r, a);
                                release(&mut c, b);
                            }
                            _ => {}
                        }
                    }
                    18 => {
                        let sp = if rng.below(3) == 0 {
                            Spec::Null
                        } else {
                            Spec::Leaf {
                                sym: rng.below(nsym) as u16,
                                pad: 0,
                                size: 1,
                                state: 1,
                                extra: false,
                                ext: Some(
                                    (0..rng.below(30)).map(|_| rng.below(3) as c_char).collect(),
                                ),
                            }
                        };
                        let (a, b) = (
                            build(&sp, false, r.p(), lang),
                            build(&sp, true, c.p(), lang),
                        );
                        ts_stack_set_last_external_token(r.stack as *mut Stack, v, a);
                        c_ref_stack_set_last_external_token(c.stack, v, b);
                        release(&mut r, a);
                        release(&mut c, b);
                    }
                    19 => {
                        if rng.below(10) == 0 {
                            ts_stack_clear(r.stack as *mut Stack);
                            c_ref_stack_clear(c.stack);
                        } else {
                            let d = rng.below(7) as u32;
                            summaries += 1;
                            ts_stack_record_summary(r.stack as *mut Stack, v, d);
                            c_ref_stack_record_summary(c.stack, v, d);
                        }
                    }
                    _ => {}
                }

                // Compare the whole stack.
                let count = st!(r, ts_stack_version_count, c_ref_stack_version_count);
                assert_eq!(
                    count,
                    st!(c, ts_stack_version_count, c_ref_stack_version_count),
                    "{ctx} count"
                );
                assert_eq!(
                    st!(
                        r,
                        ts_stack_halted_version_count,
                        c_ref_stack_halted_version_count
                    ),
                    st!(
                        c,
                        ts_stack_halted_version_count,
                        c_ref_stack_halted_version_count
                    ),
                    "{ctx} halted"
                );
                for w in 0..count {
                    assert_eq!(view(&mut r, w), view(&mut c, w), "{ctx} version {w}");
                }
                if count > 1 {
                    let (a, b) = (
                        rng.below(u64::from(count)) as u32,
                        rng.below(u64::from(count)) as u32,
                    );
                    assert_eq!(
                        ts_stack_can_merge(r.stack as *mut Stack, a, b),
                        c_ref_stack_can_merge(c.stack, a, b),
                        "{ctx} can_merge {a} {b}"
                    );
                }
                if step % 23 == 22 {
                    assert_eq!(dot(&mut r, lang), dot(&mut c, lang), "{ctx} dot graph");
                }
                steps += 1;
            }
            ts_stack_delete(r.stack as *mut Stack);
            c_ref_stack_delete(c.stack);
            ts_subtree_pool_delete(r.p());
            c_ref_subtree_pool_delete(c.p());
        }
    }
    assert!(steps > 100_000);
    eprintln!("merges {merges}, popped slices {popped}, summaries {summaries}");
    assert!(merges > 200 && popped > 2000 && summaries > 500);
}
