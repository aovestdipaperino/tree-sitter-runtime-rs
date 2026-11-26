//! The subtree port against the C original (src/subtree.c compiled with renamed symbols).
//!
//! 1. Bit layout: the Rust accessors for inline and heap subtrees read what the C compiler's
//!    bitfields hold, for random data.
//! 2. Random operation scripts run in lockstep on both implementations; after every step the full
//!    observable state of every live tree (through the C header's accessors) must be identical.
#![cfg(feature = "reference")]
use core::ffi::{CStr, c_char, c_int, c_void};
mod common;
use common::*;
use tree_sitter_runtime_rs::{
    api::*, array::Array, language::*, length::*, parser_abi::*, subtree::*,
};

unsafe extern "C" {
    fn shim_heap_flag_bits(which: c_int) -> u16;
    fn shim_inline_from_bits(bits: u64) -> Subtree;
    #[allow(clippy::too_many_arguments)]
    fn shim_inline_build(
        ps: u16,
        sym: u8,
        pc: u8,
        pr: u8,
        pb: u8,
        sb: u8,
        la: u8,
        v: c_int,
        n: c_int,
        e: c_int,
        c: c_int,
        m: c_int,
        k: c_int,
    ) -> u64;

    fn c_ref_external_scanner_state_init(s: *mut ExternalScannerState, d: *const c_char, n: u32);
    fn c_ref_external_scanner_state_copy(s: *const ExternalScannerState) -> ExternalScannerState;
    fn c_ref_external_scanner_state_delete(s: *mut ExternalScannerState);
    fn c_ref_external_scanner_state_data(s: *const ExternalScannerState) -> *const c_char;
    fn c_ref_external_scanner_state_eq(
        s: *const ExternalScannerState,
        b: *const c_char,
        n: u32,
    ) -> bool;
    fn c_ref_subtree_array_copy(a: SubtreeArray, d: *mut SubtreeArray);
    fn c_ref_subtree_array_clear(p: *mut SubtreePool, a: *mut SubtreeArray);
    fn c_ref_subtree_array_delete(p: *mut SubtreePool, a: *mut SubtreeArray);
    fn c_ref_subtree_array_remove_trailing_extras(a: *mut SubtreeArray, d: *mut SubtreeArray);
    fn c_ref_subtree_array_reverse(a: *mut SubtreeArray);
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
    fn c_ref_subtree_new_missing_leaf(
        p: *mut SubtreePool,
        sym: u16,
        st: u16,
        pad: Length,
        la: u32,
        l: *const TSLanguage,
    ) -> Subtree;
    fn c_ref_subtree_make_mut(p: *mut SubtreePool, s: Subtree) -> MutableSubtree;
    fn c_ref_subtree_retain(s: Subtree);
    fn c_ref_subtree_release(p: *mut SubtreePool, s: Subtree);
    fn c_ref_subtree_compare(a: Subtree, b: Subtree, p: *mut SubtreePool) -> c_int;
    fn c_ref_subtree_set_symbol(s: *mut MutableSubtree, sym: u16, l: *const TSLanguage);
    fn c_ref_subtree_compress(
        s: MutableSubtree,
        n: u32,
        l: *const TSLanguage,
        st: *mut MutableSubtreeArray,
    );
    fn c_ref_subtree_edit(s: Subtree, e: *const TSInputEdit, p: *mut SubtreePool) -> Subtree;
    fn c_ref_subtree_string(
        s: Subtree,
        a: u16,
        an: bool,
        l: *const TSLanguage,
        all: bool,
    ) -> *mut c_char;
    fn c_ref_subtree_last_external_token(s: Subtree) -> Subtree;
    fn c_ref_subtree_external_scanner_state_eq(a: Subtree, b: Subtree) -> bool;
    fn free(p: *mut c_void);
}

// ---- 1. bit layout ----

#[test]
fn heap_flag_bits_match_c() {
    // Flag i must be bit i of the 16 bits following parse_state.
    for i in 0..11 {
        assert_eq!(unsafe { shim_heap_flag_bits(i) }, 1u16 << i, "flag {i}");
    }
}

#[test]
fn inline_accessors_match_c_for_random_bits() {
    let mut rng = Rng(0xfeed_f00d_1234_5678);
    for _ in 0..200_000 {
        let bits = rng.next() | 1; // is_inline set
        let s = Subtree {
            data: SubtreeInlineData(bits),
        };
        let c = unsafe { shim_inline_from_bits(bits) };
        // The same bytes seen through C and Rust.
        assert_eq!(unsafe { c.data }.0, bits);
        let mut rec = [0u64; N_SLOTS];
        unsafe { common::shim_dump_node(c, rec.as_mut_ptr()) };
        assert_eq!(dump_rust(s), rec.to_vec(), "bits {bits:#018x}");
    }
}

#[test]
fn inline_setters_match_c_bitfield_assignment() {
    let mut rng = Rng(99);
    for _ in 0..100_000 {
        let (ps, sym, pc, pr, pb, sb, la) = (
            rng.next() as u16,
            rng.next() as u8,
            rng.next() as u8,
            rng.next() as u8,
            rng.next() as u8,
            rng.next() as u8,
            rng.next() as u8,
        );
        let f: [bool; 6] = std::array::from_fn(|_| rng.below(2) == 1);
        let c = unsafe {
            shim_inline_build(
                ps,
                sym,
                pc,
                pr,
                pb,
                sb,
                la,
                f[0] as c_int,
                f[1] as c_int,
                f[2] as c_int,
                f[3] as c_int,
                f[4] as c_int,
                f[5] as c_int,
            )
        };
        let mut d = SubtreeInlineData(0);
        d.set_parse_state(ps);
        d.set_symbol(sym);
        d.set_padding_columns(pc);
        d.set_padding_rows(pr & 0xf);
        d.set_padding_bytes(pb);
        d.set_size_bytes(sb);
        d.set_lookahead_bytes(la & 0xf);
        d.set_visible(f[0]);
        d.set_named(f[1]);
        d.set_extra(f[2]);
        d.set_has_changes(f[3]);
        d.set_is_missing(f[4]);
        d.set_is_keyword(f[5]);
        d.set_is_inline(true);
        assert_eq!(d.0, c);
    }
}

// ---- 2. lockstep scripts ----

struct Side {
    pool: SubtreePool,
    trees: Vec<Subtree>,
}

fn lengths(rng: &mut Rng) -> (Length, Length) {
    let small = rng.below(3) != 0;
    let big = |rng: &mut Rng| if small { rng.below(10) } else { rng.below(400) } as u32;
    let rows = if small { rng.below(3) } else { rng.below(40) } as u32;
    let pad = Length {
        bytes: big(rng),
        extent: TSPoint {
            row: rows.min(if small { 15 } else { 40 }),
            column: big(rng),
        },
    };
    let rows_s = if rng.below(5) == 0 {
        rng.below(3) as u32
    } else {
        0
    };
    let size = Length {
        bytes: big(rng),
        extent: TSPoint {
            row: rows_s,
            column: big(rng),
        },
    };
    (pad, size)
}

fn random_edit(rng: &mut Rng) -> TSInputEdit {
    let start = rng.below(200) as u32;
    let old_end = start + rng.below(40) as u32;
    let new_end = start + rng.below(40) as u32;
    let pt = |rng: &mut Rng, b: u32| TSPoint {
        row: (b / 30) + rng.below(2) as u32,
        column: b % 30,
    };
    TSInputEdit {
        start_byte: start,
        old_end_byte: old_end,
        new_end_byte: new_end,
        start_point: pt(rng, start),
        old_end_point: pt(rng, old_end),
        new_end_point: pt(rng, new_end),
    }
}

fn check(r: &Side, c: &Side, ctx: &str) {
    assert_eq!(r.trees.len(), c.trees.len(), "{ctx}");
    for (i, (&a, &b)) in r.trees.iter().zip(&c.trees).enumerate() {
        let (dc_b, dr_a) = (dump_c(b), dump_rust(a));
        assert_eq!(
            dump_c(a),
            dc_b,
            "{ctx}: tree {i} (rust-built vs c-built, both read by C)"
        );
        assert_eq!(
            dr_a, dc_b,
            "{ctx}: tree {i} (rust accessors vs c accessors)"
        );
    }
}

fn cstring(p: *mut c_char) -> Vec<u8> {
    let v = unsafe { CStr::from_ptr(p) }.to_bytes().to_vec();
    unsafe { free(p as *mut c_void) };
    v
}

#[test]
fn random_scripts_match_c() {
    let lang = language();
    let nsym = unsafe { ts_language_symbol_count(lang) } as u64;
    let nprod = unsafe { (*lang).production_id_count } as u64;
    let mut ops = 0u64;
    for seed in 1..=3000u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        unsafe {
            let mut r = Side {
                pool: ts_subtree_pool_new(32),
                trees: vec![],
            };
            let mut c = Side {
                pool: c_ref_subtree_pool_new(32),
                trees: vec![],
            };
            for step in 0..60 {
                let op = rng.below(16);
                let ctx = format!("seed {seed} step {step} op {op}");
                if std::env::var_os("TRACE").is_some() {
                    eprintln!("{ctx}");
                }
                let n = r.trees.len();
                match op {
                    0..=2 | 15 => {
                        // Leaf (maybe external), missing leaf or error leaf.
                        let (pad, size) = lengths(&mut rng);
                        let sym = rng.below(nsym) as u16;
                        let la = rng.below(20) as u32;
                        let st = [0u16, 1, 7, 255, 65535][rng.below(5) as usize];
                        let (ext, col, kw) =
                            (rng.below(5) == 0, rng.below(4) == 0, rng.below(4) == 0);
                        let kind = rng.below(6);
                        let (a, b) = match kind {
                            0 => (
                                ts_subtree_new_missing_leaf(&mut r.pool, sym, st, pad, la, lang),
                                c_ref_subtree_new_missing_leaf(&mut c.pool, sym, st, pad, la, lang),
                            ),
                            1 => {
                                let ch =
                                    [-1, 0, 10, 9, 13, 65, 200, 0x1f600][rng.below(8) as usize];
                                (
                                    ts_subtree_new_error(&mut r.pool, ch, pad, size, la, st, lang),
                                    c_ref_subtree_new_error(
                                        &mut c.pool,
                                        ch,
                                        pad,
                                        size,
                                        la,
                                        st,
                                        lang,
                                    ),
                                )
                            }
                            _ => (
                                ts_subtree_new_leaf(
                                    &mut r.pool,
                                    sym,
                                    pad,
                                    size,
                                    la,
                                    st,
                                    ext,
                                    col,
                                    kw,
                                    lang,
                                ),
                                c_ref_subtree_new_leaf(
                                    &mut c.pool,
                                    sym,
                                    pad,
                                    size,
                                    la,
                                    st,
                                    ext,
                                    col,
                                    kw,
                                    lang,
                                ),
                            ),
                        };
                        if kind >= 2 && ext && !a.is_inline() {
                            let len = rng.below(41) as usize;
                            let data: Vec<c_char> =
                                (0..len).map(|_| rng.next() as c_char).collect();
                            ts_external_scanner_state_init(
                                &mut (*(a.ptr as *mut SubtreeHeapData)).u.external_scanner_state,
                                data.as_ptr(),
                                len as u32,
                            );
                            (*(a.ptr as *mut SubtreeHeapData)).set_has_external_tokens(true);
                            c_ref_external_scanner_state_init(
                                &mut (*(b.ptr as *mut SubtreeHeapData)).u.external_scanner_state,
                                data.as_ptr(),
                                len as u32,
                            );
                        }
                        r.trees.push(a);
                        c.trees.push(b);
                    }
                    3 | 4 if n > 0 => {
                        let k = rng.below(7) as usize;
                        let picks: Vec<usize> =
                            (0..k).map(|_| rng.below(n as u64) as usize).collect();
                        let mut ra: Array<Subtree> = Array::new();
                        let mut ca: Array<Subtree> = Array::new();
                        for &i in &picks {
                            ts_subtree_retain(r.trees[i]);
                            c_ref_subtree_retain(c.trees[i]);
                            ra.push(r.trees[i]);
                            ca.push(c.trees[i]);
                        }
                        let (a, b) = if op == 3 {
                            let sym = rng.below(nsym + 2) as u16;
                            let sym = if sym as u64 >= nsym {
                                [ts_builtin_sym_error, ts_builtin_sym_error_repeat]
                                    [(sym as u64 - nsym) as usize]
                            } else {
                                sym
                            };
                            // An `_ERROR` child must carry its extent cost; only build such nodes from nodes we made below.
                            // Often reuse a child's symbol, so repetition (`repeat_depth`) logic is reached.
                            let sym = match picks.first() {
                                Some(&i) if rng.below(3) == 0 => ts_subtree_symbol(r.trees[i]),
                                _ => sym,
                            };
                            let prod = rng.below(nprod.max(1)) as u32;
                            (
                                ts_subtree_from_mut(ts_subtree_new_node(sym, &mut ra, prod, lang)),
                                ts_subtree_from_mut(c_ref_subtree_new_node(
                                    sym, &mut ca, prod, lang,
                                )),
                            )
                        } else {
                            let extra = rng.below(2) == 0;
                            (
                                ts_subtree_new_error_node(&mut ra, extra, lang),
                                c_ref_subtree_new_error_node(&mut ca, extra, lang),
                            )
                        };
                        r.trees.push(a);
                        c.trees.push(b);
                    }
                    5 if n > 0 => {
                        let i = rng.below(n as u64) as usize;
                        ts_subtree_release(&mut r.pool, r.trees.swap_remove(i));
                        c_ref_subtree_release(&mut c.pool, c.trees.swap_remove(i));
                    }
                    6 if n > 0 => {
                        let i = rng.below(n as u64) as usize;
                        ts_subtree_retain(r.trees[i]);
                        c_ref_subtree_retain(c.trees[i]);
                        r.trees.push(r.trees[i]);
                        c.trees.push(c.trees[i]);
                    }
                    7 if n > 0 => {
                        let i = rng.below(n as u64) as usize;
                        let (mut ma, mut mb) = (
                            ts_subtree_make_mut(&mut r.pool, r.trees[i]),
                            c_ref_subtree_make_mut(&mut c.pool, c.trees[i]),
                        );
                        let extra = rng.below(2) == 0;
                        ts_subtree_set_extra(&mut ma, extra);
                        // The C `set_extra` is an inline function of the header; do the same by hand.
                        if mb.data.is_inline() {
                            let mut d = mb.data;
                            d.set_extra(extra);
                            mb.data = d
                        } else {
                            (*mb.ptr).set_extra(extra)
                        }
                        if rng.below(2) == 0 {
                            let sym = rng.below(nsym) as u16;
                            if !ma.data.is_inline() || sym < 255 {
                                ts_subtree_set_symbol(&mut ma, sym, lang);
                                c_ref_subtree_set_symbol(&mut mb, sym, lang);
                            }
                        }
                        r.trees[i] = ts_subtree_from_mut(ma);
                        c.trees[i] = ts_subtree_from_mut(mb);
                    }
                    8 if n > 0 => {
                        let i = rng.below(n as u64) as usize;
                        let e = random_edit(&mut rng);
                        r.trees[i] = ts_subtree_edit(r.trees[i], &e, &mut r.pool);
                        c.trees[i] = c_ref_subtree_edit(c.trees[i], &e, &mut c.pool);
                    }
                    9 if n > 1 => {
                        let (i, j) = (rng.below(n as u64) as usize, rng.below(n as u64) as usize);
                        assert_eq!(
                            ts_subtree_compare(r.trees[i], r.trees[j], &mut r.pool),
                            c_ref_subtree_compare(c.trees[i], c.trees[j], &mut c.pool),
                            "{ctx} compare"
                        );
                    }
                    10 | 11 if n > 0 => {
                        let i = rng.below(n as u64) as usize;
                        let alias = if rng.below(3) == 0 {
                            rng.below(nsym) as u16
                        } else {
                            0
                        };
                        let (named, all) = (rng.below(2) == 0, rng.below(2) == 0);
                        let a = cstring(ts_subtree_string(r.trees[i], alias, named, lang, all));
                        let b = cstring(c_ref_subtree_string(c.trees[i], alias, named, lang, all));
                        assert_eq!(
                            String::from_utf8_lossy(&a),
                            String::from_utf8_lossy(&b),
                            "{ctx} string"
                        );
                    }
                    12 if n > 1 => {
                        let (i, j) = (rng.below(n as u64) as usize, rng.below(n as u64) as usize);
                        assert_eq!(
                            ts_subtree_external_scanner_state_eq(r.trees[i], r.trees[j]),
                            c_ref_subtree_external_scanner_state_eq(c.trees[i], c.trees[j]),
                            "{ctx} state_eq"
                        );
                    }
                    13 if n > 0 => {
                        let i = rng.below(n as u64) as usize;
                        let (a, b) = (
                            ts_subtree_last_external_token(r.trees[i]),
                            c_ref_subtree_last_external_token(c.trees[i]),
                        );
                        assert_eq!(
                            a.ptr.is_null(),
                            b.ptr.is_null(),
                            "{ctx} last_external_token null"
                        );
                        if !a.ptr.is_null() {
                            assert_eq!(dump_rust(a), dump_c(b), "{ctx} last_external_token");
                        }
                    }
                    14 if n > 0 => {
                        // Array operations on a random selection.
                        let k = rng.below(8) as usize;
                        let picks: Vec<usize> =
                            (0..k).map(|_| rng.below(n as u64) as usize).collect();
                        let mut ra: Array<Subtree> = Array::new();
                        let mut ca: Array<Subtree> = Array::new();
                        for &i in &picks {
                            ts_subtree_retain(r.trees[i]);
                            c_ref_subtree_retain(c.trees[i]);
                            ra.push(r.trees[i]);
                            ca.push(c.trees[i]);
                        }
                        let (mut rc, mut cc): (Array<Subtree>, Array<Subtree>) =
                            (Array::new(), Array::new());
                        ts_subtree_array_copy(ra, &mut rc);
                        c_ref_subtree_array_copy(ca, &mut cc);
                        let (mut rd, mut cd): (Array<Subtree>, Array<Subtree>) =
                            (Array::new(), Array::new());
                        ts_subtree_array_remove_trailing_extras(&mut rc, &mut rd);
                        c_ref_subtree_array_remove_trailing_extras(&mut cc, &mut cd);
                        ts_subtree_array_reverse(&mut ra);
                        c_ref_subtree_array_reverse(&mut ca);
                        let sizes = |a: &Array<Subtree>, b: &Array<Subtree>| {
                            assert_eq!((a.size, a.capacity), (b.size, b.capacity), "{ctx}")
                        };
                        sizes(&ra, &ca);
                        sizes(&rc, &cc);
                        sizes(&rd, &cd);
                        for (x, y) in [(&ra, &ca), (&rc, &cc), (&rd, &cd)] {
                            for i in 0..x.size {
                                assert_eq!(
                                    dump_rust(*x.contents.add(i as usize)),
                                    dump_c(*y.contents.add(i as usize)),
                                    "{ctx} array elem {i}"
                                );
                            }
                        }
                        ts_subtree_array_delete(&mut r.pool, &mut ra);
                        c_ref_subtree_array_delete(&mut c.pool, &mut ca);
                        ts_subtree_array_delete(&mut r.pool, &mut rc);
                        c_ref_subtree_array_delete(&mut c.pool, &mut cc);
                        ts_subtree_array_clear(&mut r.pool, &mut rd);
                        c_ref_subtree_array_clear(&mut c.pool, &mut cd);
                        ts_subtree_array_delete(&mut r.pool, &mut rd);
                        c_ref_subtree_array_delete(&mut c.pool, &mut cd);
                    }
                    _ => {}
                }
                check(&r, &c, &ctx);
                ops += 1;
            }
            for t in r.trees.drain(..) {
                ts_subtree_release(&mut r.pool, t);
            }
            for t in c.trees.drain(..) {
                c_ref_subtree_release(&mut c.pool, t);
            }
            ts_subtree_pool_delete(&mut r.pool);
            c_ref_subtree_pool_delete(&mut c.pool);
        }
    }
    assert!(ops > 150_000);
}

#[test]
fn external_scanner_state_matches_c() {
    let mut rng = Rng(5);
    for len in 0..70usize {
        let data: Vec<c_char> = (0..len).map(|_| rng.next() as c_char).collect();
        unsafe {
            let (mut a, mut b) = (
                core::mem::zeroed::<ExternalScannerState>(),
                core::mem::zeroed::<ExternalScannerState>(),
            );
            ts_external_scanner_state_init(&mut a, data.as_ptr(), len as u32);
            c_ref_external_scanner_state_init(&mut b, data.as_ptr(), len as u32);
            let (ca, cb) = (
                ts_external_scanner_state_copy(&a),
                c_ref_external_scanner_state_copy(&b),
            );
            for (x, y) in [(&a, &b), (&ca, &cb)] {
                assert_eq!(x.length, y.length);
                let (dx, dy) = (
                    ts_external_scanner_state_data(x) as *const u8,
                    c_ref_external_scanner_state_data(y) as *const u8,
                );
                assert_eq!(
                    std::slice::from_raw_parts(dx, len),
                    std::slice::from_raw_parts(dy, len)
                );
                assert!(
                    ts_external_scanner_state_eq(x, data.as_ptr(), len as u32)
                        && c_ref_external_scanner_state_eq(y, data.as_ptr(), len as u32)
                );
                let mut other = data.clone();
                other.push(1);
                assert!(!ts_external_scanner_state_eq(
                    x,
                    other.as_ptr(),
                    other.len() as u32
                ));
                if len > 0 {
                    other.pop();
                    other[0] = other[0].wrapping_add(1);
                    assert_eq!(
                        ts_external_scanner_state_eq(x, other.as_ptr(), len as u32),
                        c_ref_external_scanner_state_eq(y, other.as_ptr(), len as u32)
                    );
                }
            }
            let (mut ma, mut mb) = (ca, cb);
            ts_external_scanner_state_delete(&mut a);
            c_ref_external_scanner_state_delete(&mut b);
            ts_external_scanner_state_delete(&mut ma);
            c_ref_external_scanner_state_delete(&mut mb);
        }
    }
}

/// `ts_subtree_compress` on a left-leaning chain of one repeated symbol, as `repeat` rules produce.
#[test]
fn compress_matches_c() {
    let lang = language();
    // Any non-visible, non-named symbol works as the repeat symbol; find one.
    let rep = (0..unsafe { ts_language_symbol_count(lang) } as u16)
        .find(|&s| {
            let m = unsafe { ts_language_symbol_metadata(lang, s) };
            !m.visible && !m.named
        })
        .unwrap();
    for depth in 2..14usize {
        unsafe {
            let mut r = ts_subtree_pool_new(32);
            let mut c = c_ref_subtree_pool_new(32);
            let leaf_len = Length {
                bytes: 1,
                extent: TSPoint { row: 0, column: 1 },
            };
            let zero = length_zero();
            let build = |is_c: bool, pool: *mut SubtreePool| -> Subtree {
                let leaf = |pool: *mut SubtreePool| {
                    if is_c {
                        c_ref_subtree_new_leaf(
                            pool, 1, zero, leaf_len, 0, 1, false, false, false, lang,
                        )
                    } else {
                        ts_subtree_new_leaf(
                            pool, 1, zero, leaf_len, 0, 1, false, false, false, lang,
                        )
                    }
                };
                let mut tree = leaf(pool);
                for _ in 0..depth {
                    let mut arr: Array<Subtree> = Array::new();
                    arr.push(tree);
                    arr.push(leaf(pool));
                    let m = if is_c {
                        c_ref_subtree_new_node(rep, &mut arr, 0, lang)
                    } else {
                        ts_subtree_new_node(rep, &mut arr, 0, lang)
                    };
                    tree = ts_subtree_from_mut(m);
                }
                tree
            };
            let (ra, ca) = (build(false, &mut r), build(true, &mut c));
            assert_eq!(dump_rust(ra), dump_c(ca));
            let (mut rs, mut cs): (MutableSubtreeArray, MutableSubtreeArray) =
                (Array::new(), Array::new());
            ts_subtree_compress(ts_subtree_to_mut_unsafe(ra), depth as u32, lang, &mut rs);
            c_ref_subtree_compress(ts_subtree_to_mut_unsafe(ca), depth as u32, lang, &mut cs);
            assert_eq!(dump_rust(ra), dump_c(ca), "depth {depth}");
            assert_eq!(rs.size, cs.size);
            ts_subtree_release(&mut r, ra);
            c_ref_subtree_release(&mut c, ca);
            rs.delete();
            cs.delete();
            ts_subtree_pool_delete(&mut r);
            c_ref_subtree_pool_delete(&mut c);
        }
    }
}
