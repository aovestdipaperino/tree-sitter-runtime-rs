//! The Rust allocator unit against the C original (renamed in the shim).
//!
//! The hooks are process-global, so everything runs in one test.
use core::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use tree_sitter_runtime_rs::alloc::*;

unsafe extern "C" {
    fn c_set_allocator(
        m: Option<unsafe extern "C" fn(usize) -> *mut c_void>,
        c: Option<unsafe extern "C" fn(usize, usize) -> *mut c_void>,
        r: Option<unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void>,
        f: Option<unsafe extern "C" fn(*mut c_void)>,
    );
    fn c_malloc(n: usize) -> *mut c_void;
    fn c_calloc(n: usize, s: usize) -> *mut c_void;
    fn c_realloc(p: *mut c_void, n: usize) -> *mut c_void;
    fn c_free(p: *mut c_void);
    fn malloc(n: usize) -> *mut c_void;
    fn calloc(n: usize, s: usize) -> *mut c_void;
    fn realloc(p: *mut c_void, n: usize) -> *mut c_void;
}

static HITS: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];

unsafe extern "C" fn m(n: usize) -> *mut c_void {
    HITS[0].fetch_add(1, SeqCst);
    unsafe { malloc(n) }
}
unsafe extern "C" fn c(n: usize, s: usize) -> *mut c_void {
    HITS[1].fetch_add(1, SeqCst);
    unsafe { calloc(n, s) }
}
unsafe extern "C" fn r(p: *mut c_void, n: usize) -> *mut c_void {
    HITS[2].fetch_add(1, SeqCst);
    unsafe { realloc(p, n) }
}
unsafe extern "C" fn f(p: *mut c_void) {
    HITS[3].fetch_add(1, SeqCst);
    unsafe { tree_sitter_free(p) }
}
unsafe extern "C" {
    #[link_name = "free"]
    fn tree_sitter_free(p: *mut c_void);
}

fn reset() {
    for h in &HITS {
        h.store(0, SeqCst);
    }
}

/// Drives the four hooks and reports which of the custom functions ran.
fn rust_run() -> [usize; 4] {
    reset();
    unsafe {
        let p = ts_current_malloc.unwrap()(16);
        let q = ts_current_calloc.unwrap()(2, 8);
        let p = ts_current_realloc.unwrap()(p, 64);
        ts_current_free.unwrap()(p);
        ts_current_free.unwrap()(q);
    }
    std::array::from_fn(|i| HITS[i].load(SeqCst))
}

fn c_run() -> [usize; 4] {
    reset();
    unsafe {
        let p = c_malloc(16);
        let q = c_calloc(2, 8);
        let p = c_realloc(p, 64);
        c_free(p);
        c_free(q);
    }
    std::array::from_fn(|i| HITS[i].load(SeqCst))
}

#[test]
fn set_allocator_matches_c_for_every_null_combination() {
    type M = unsafe extern "C" fn(usize) -> *mut c_void;
    type C = unsafe extern "C" fn(usize, usize) -> *mut c_void;
    type R = unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void;
    type F = unsafe extern "C" fn(*mut c_void);
    for mask in 0..16u32 {
        let (mm, cc, rr, ff): (Option<M>, Option<C>, Option<R>, Option<F>) = (
            (mask & 1 != 0).then_some(m as M),
            (mask & 2 != 0).then_some(c as C),
            (mask & 4 != 0).then_some(r as R),
            (mask & 8 != 0).then_some(f as F),
        );
        unsafe {
            ts_set_allocator(mm, cc, rr, ff);
            c_set_allocator(mm, cc, rr, ff);
        }
        let (a, b) = (rust_run(), c_run());
        assert_eq!(a, b, "mask {mask:#06b}");
        // A custom function is used exactly when it was given.
        assert_eq!(a[0] == 1, mask & 1 != 0);
        assert_eq!(a[1] == 1, mask & 2 != 0);
        assert_eq!(a[2] == 1, mask & 4 != 0);
        assert_eq!(a[3] == 2, mask & 8 != 0);
    }
    // Defaults work: zero sizes, growth, and contents survive realloc.
    unsafe {
        ts_set_allocator(None, None, None, None);
        let z = ts_current_malloc.unwrap()(0);
        ts_current_free.unwrap()(z);
        let p = ts_current_calloc.unwrap()(4, 4) as *mut u8;
        assert!((0..16).all(|i| *p.add(i) == 0));
        *p = 7;
        let p = ts_current_realloc.unwrap()(p as *mut c_void, 4096) as *mut u8;
        assert_eq!(*p, 7);
        ts_current_free.unwrap()(p as *mut c_void);
    }
}
