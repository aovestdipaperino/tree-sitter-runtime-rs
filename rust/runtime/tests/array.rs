//! `array.h` and `reduce_action.h`: random operation scripts run on the Rust `Array` and on the
//! C `_array__*` helpers side by side; size, capacity and contents must agree after every step.
use core::ffi::c_void;
use tree_sitter_runtime_rs::{array::*, reduce_action::*};

unsafe extern "C" {
    fn shim_array_erase(c: *mut u8, size: *mut u32, es: usize, i: u32);
    fn shim_array_reserve(c: *mut u8, cap: *mut u32, es: usize, n: u32) -> *mut u8;
    fn shim_array_assign(
        c: *mut u8,
        sz: *mut u32,
        cap: *mut u32,
        oc: *const u8,
        os: u32,
        es: usize,
    ) -> *mut u8;
    fn shim_array_grow(c: *mut u8, sz: u32, cap: *mut u32, n: u32, es: usize) -> *mut u8;
    fn shim_array_splice(
        c: *mut u8,
        sz: *mut u32,
        cap: *mut u32,
        es: usize,
        i: u32,
        oc: u32,
        nc: u32,
        el: *const u8,
    ) -> *mut u8;
    fn shim_array_search_sorted_by(
        c: *const Sort,
        size: u32,
        needle: u32,
        exists: *mut bool,
    ) -> u32;
    fn shim_array_search_sorted_with(
        c: *const Sort,
        size: u32,
        needle: u32,
        exists: *mut bool,
    ) -> u32;
    fn shim_reduce_action_set_add(
        c: *mut ReduceAction,
        size: *mut u32,
        cap: *mut u32,
        a: ReduceAction,
    ) -> *mut ReduceAction;
}

/// An element with padding, to catch element-size mistakes.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Elem {
    a: u32,
    b: u16,
    c: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Sort {
    key: u32,
    tag: u16,
}

/// The C side: raw parts, driven through the shim.
struct CArr {
    contents: *mut u8,
    size: u32,
    capacity: u32,
}

const ES: usize = size_of::<Elem>();

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u32) -> u32 {
        (self.next() % u64::from(n.max(1))) as u32
    }
}

fn bytes_of(a: *mut u8, n: u32) -> Vec<u8> {
    if a.is_null() {
        vec![]
    } else {
        unsafe { std::slice::from_raw_parts(a, n as usize * ES).to_vec() }
    }
}

fn assert_same(r: &Array<Elem>, c: &CArr, ctx: &str) {
    assert_eq!((r.size, r.capacity), (c.size, c.capacity), "{ctx}");
    assert_eq!(r.contents.is_null(), c.contents.is_null(), "{ctx}");
    assert_eq!(
        bytes_of(r.contents as *mut u8, r.size),
        bytes_of(c.contents, c.size),
        "{ctx}"
    );
}

fn elem(rng: &mut Rng) -> Elem {
    Elem {
        a: rng.next() as u32,
        b: rng.next() as u16,
        c: rng.next(),
    }
}

#[test]
fn random_scripts_match_c() {
    for seed in 1..=60u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let mut r: Array<Elem> = Array::new();
        let mut c = CArr {
            contents: core::ptr::null_mut(),
            size: 0,
            capacity: 0,
        };
        let mut other_r: Array<Elem> = Array::new();
        for step in 0..400 {
            let op = rng.below(11);
            let ctx = format!("seed {seed} step {step} op {op}");
            unsafe {
                match op {
                    0 | 1 => {
                        let e = elem(&mut rng);
                        r.push(e);
                        c.contents = shim_array_grow(c.contents, c.size, &mut c.capacity, 1, ES);
                        *(c.contents as *mut Elem).add(c.size as usize) = e;
                        c.size += 1;
                    }
                    2 => {
                        let n = rng.below(20);
                        r.grow_by(n);
                        if n > 0 {
                            c.contents =
                                shim_array_grow(c.contents, c.size, &mut c.capacity, n, ES);
                            core::ptr::write_bytes(
                                c.contents.add(c.size as usize * ES),
                                0,
                                n as usize * ES,
                            );
                            c.size += n;
                        }
                    }
                    3 if r.size > 0 => {
                        let i = rng.below(r.size);
                        r.erase(i);
                        shim_array_erase(c.contents, &mut c.size, ES, i);
                    }
                    4 => {
                        let i = rng.below(r.size + 1);
                        let e = elem(&mut rng);
                        r.insert(i, e);
                        c.contents = shim_array_splice(
                            c.contents,
                            &mut c.size,
                            &mut c.capacity,
                            ES,
                            i,
                            0,
                            1,
                            &e as *const Elem as *const u8,
                        );
                    }
                    5 => {
                        let i = rng.below(r.size + 1);
                        let old = rng.below(r.size - i + 1);
                        let new = rng.below(12);
                        let els: Vec<Elem> = (0..new).map(|_| elem(&mut rng)).collect();
                        // Null `elements` zero-fills: exercise it when there are none to copy.
                        let p = if rng.below(4) == 0 {
                            core::ptr::null()
                        } else {
                            els.as_ptr()
                        };
                        let new = if p.is_null() { new } else { els.len() as u32 };
                        r.splice(i, old, new, p);
                        c.contents = shim_array_splice(
                            c.contents,
                            &mut c.size,
                            &mut c.capacity,
                            ES,
                            i,
                            old,
                            new,
                            p as *const u8,
                        );
                    }
                    6 => {
                        let n = rng.below(10);
                        let els: Vec<Elem> = (0..n).map(|_| elem(&mut rng)).collect();
                        r.extend(n, els.as_ptr());
                        c.contents = shim_array_splice(
                            c.contents,
                            &mut c.size,
                            &mut c.capacity,
                            ES,
                            c.size,
                            0,
                            n,
                            els.as_ptr() as *const u8,
                        );
                    }
                    7 => {
                        let n = rng.below(50);
                        r.reserve(n);
                        c.contents = shim_array_reserve(c.contents, &mut c.capacity, ES, n);
                    }
                    8 => {
                        let n = rng.below(15);
                        for _ in 0..n {
                            let e = elem(&mut rng);
                            other_r.push(e);
                        }
                        r.assign(&other_r);
                        c.contents = shim_array_assign(
                            c.contents,
                            &mut c.size,
                            &mut c.capacity,
                            other_r.contents as *const u8,
                            other_r.size,
                            ES,
                        );
                        other_r.clear();
                    }
                    9 if r.size > 0 => {
                        let p = r.pop();
                        c.size -= 1;
                        assert_eq!(
                            p,
                            *(c.contents as *const Elem).add(c.size as usize),
                            "{ctx}"
                        );
                    }
                    10 if rng.below(8) == 0 => {
                        r.clear();
                        c.size = 0;
                    }
                    _ => {}
                }
            }
            assert_same(&r, &c, &ctx);
        }
        unsafe {
            r.delete();
            other_r.delete();
            if !c.contents.is_null() {
                tree_sitter_runtime_rs::alloc::ts_free(c.contents as *mut c_void);
            }
        }
    }
}

#[test]
fn swap_front_back_push_all() {
    unsafe {
        let mut a: Array<u32> = Array::new();
        let mut b: Array<u32> = Array::new();
        for i in 0..20 {
            a.push(i);
        }
        b.push(99);
        a.swap(&mut b);
        assert_eq!((a.size, b.size), (1, 20));
        assert_eq!(
            (*a.front(), *a.back(), *b.front(), *b.back()),
            (99, 99, 0, 19)
        );
        a.push_all(&b);
        assert_eq!(a.size, 21);
        assert_eq!(*a.back(), 19);
        a.delete();
        b.delete();
        assert_eq!((a.size, a.capacity, a.contents.is_null()), (0, 0, true));
    }
}

#[test]
fn sorted_search_matches_c() {
    let mut rng = Rng(0xabcdef);
    for size in 0..40u32 {
        for _ in 0..30 {
            // Sorted, possibly with duplicates, including huge keys (the C casts to int).
            let mut keys: Vec<u32> = (0..size)
                .map(|_| {
                    [0, 1, 2, 3, 5, 8, 0x7fff_ffff, 0x8000_0000, u32::MAX][rng.below(9) as usize]
                })
                .collect();
            keys.sort_unstable();
            let elems: Vec<Sort> = keys.iter().map(|&k| Sort { key: k, tag: 0 }).collect();
            let arr = Array::<Sort> {
                contents: elems.as_ptr() as *mut Sort,
                size,
                capacity: size,
            };
            for needle in [
                0,
                1,
                2,
                4,
                5,
                9,
                0x7fff_fffe,
                0x7fff_ffff,
                0x8000_0000,
                u32::MAX,
            ] {
                let (mut ex_by, mut ex_with) = (false, false);
                let (ci_by, ci_with) = unsafe {
                    (
                        shim_array_search_sorted_by(elems.as_ptr(), size, needle, &mut ex_by),
                        shim_array_search_sorted_with(elems.as_ptr(), size, needle, &mut ex_with),
                    )
                };
                let by = unsafe { arr.search_sorted_by(|e| e.key, needle) };
                let with = unsafe {
                    arr.search_sorted_with(
                        |e, n: u32| (e.key as i32).wrapping_sub(n as i32),
                        needle,
                    )
                };
                assert_eq!(
                    by,
                    (ci_by, ex_by),
                    "by size {size} needle {needle} keys {keys:?}"
                );
                assert_eq!(with, (ci_with, ex_with), "with size {size} needle {needle}");
            }
        }
    }
}

#[test]
fn reduce_action_set_add_matches_c() {
    let mut rng = Rng(77);
    for _ in 0..50 {
        let mut r: ReduceActionSet = Array::new();
        let (mut cc, mut cs, mut ccap): (*mut ReduceAction, u32, u32) =
            (core::ptr::null_mut(), 0, 0);
        for _ in 0..60 {
            let a = ReduceAction {
                count: rng.below(4),
                symbol: rng.below(4) as u16,
                dynamic_precedence: rng.below(3) as i32 - 1,
                production_id: rng.below(5) as u16,
            };
            unsafe {
                ts_reduce_action_set_add(&mut r, a);
                cc = shim_reduce_action_set_add(cc, &mut cs, &mut ccap, a);
            }
            assert_eq!((r.size, r.capacity), (cs, ccap));
            let rv: Vec<ReduceAction> = (0..r.size)
                .map(|i| unsafe { *r.contents.add(i as usize) })
                .collect();
            let cv: Vec<ReduceAction> = (0..cs).map(|i| unsafe { *cc.add(i as usize) }).collect();
            assert_eq!(rv, cv);
        }
        unsafe {
            r.delete();
            tree_sitter_runtime_rs::alloc::ts_free(cc as *mut c_void);
        }
    }
}
