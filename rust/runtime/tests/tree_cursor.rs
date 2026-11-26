//! The tree cursor port against the C original (src/tree_cursor.c, renamed), walking real parse
//! trees of several grammars at random. After every step the whole cursor state (its stack), the
//! current node, depth, descendant index, field, parent node and query-status facts must agree.
#![cfg(feature = "reference")]
mod common;
use core::ffi::{CStr, c_char, c_void};
use tree_sitter_runtime_rs::{api::*, tree_cursor::*};

unsafe extern "C" {
    fn c_ref_tree_cursor_new(n: TSNode) -> TSTreeCursor;
    fn c_ref_tree_cursor_delete(c: *mut TSTreeCursor);
    fn c_ref_tree_cursor_reset(c: *mut TSTreeCursor, n: TSNode);
    fn c_ref_tree_cursor_goto_first_child(c: *mut TSTreeCursor) -> bool;
    fn c_ref_tree_cursor_goto_last_child(c: *mut TSTreeCursor) -> bool;
    fn c_ref_tree_cursor_goto_next_sibling(c: *mut TSTreeCursor) -> bool;
    fn c_ref_tree_cursor_goto_previous_sibling(c: *mut TSTreeCursor) -> bool;
    fn c_ref_tree_cursor_goto_parent(c: *mut TSTreeCursor) -> bool;
    fn c_ref_tree_cursor_goto_descendant(c: *mut TSTreeCursor, i: u32);
    fn c_ref_tree_cursor_goto_first_child_for_byte(c: *mut TSTreeCursor, b: u32) -> i64;
    fn c_ref_tree_cursor_goto_first_child_for_point(c: *mut TSTreeCursor, p: TSPoint) -> i64;
    fn c_ref_tree_cursor_goto_first_child_internal(c: *mut TSTreeCursor) -> u32;
    fn c_ref_tree_cursor_goto_last_child_internal(c: *mut TSTreeCursor) -> u32;
    fn c_ref_tree_cursor_goto_next_sibling_internal(c: *mut TSTreeCursor) -> u32;
    fn c_ref_tree_cursor_goto_previous_sibling_internal(c: *mut TSTreeCursor) -> u32;
    fn c_ref_tree_cursor_current_descendant_index(c: *const TSTreeCursor) -> u32;
    fn c_ref_tree_cursor_current_node(c: *const TSTreeCursor) -> TSNode;
    fn c_ref_tree_cursor_current_depth(c: *const TSTreeCursor) -> u32;
    fn c_ref_tree_cursor_parent_node(c: *const TSTreeCursor) -> TSNode;
    fn c_ref_tree_cursor_current_field_id(c: *const TSTreeCursor) -> u16;
    fn c_ref_tree_cursor_current_field_name(c: *const TSTreeCursor) -> *const c_char;
    fn c_ref_tree_cursor_copy(c: *const TSTreeCursor) -> TSTreeCursor;
    fn c_ref_tree_cursor_reset_to(d: *mut TSTreeCursor, s: *const TSTreeCursor);
    #[allow(clippy::too_many_arguments)]
    fn c_ref_tree_cursor_current_status(
        c: *const TSTreeCursor,
        f: *mut u16,
        l: *mut bool,
        n: *mut bool,
        s: *mut bool,
        st: *mut u16,
        sc: *mut u32,
    );

    fn ts_parser_new() -> *mut c_void;
    fn ts_parser_delete(p: *mut c_void);
    fn ts_parser_set_language(p: *mut c_void, l: *const TSLanguage) -> bool;
    fn ts_parser_parse_string(
        p: *mut c_void,
        old: *const TSTree,
        s: *const c_char,
        n: u32,
    ) -> *mut TSTree;
    fn ts_tree_delete(t: *mut TSTree);
    fn ts_tree_root_node(t: *const TSTree) -> TSNode;
    fn ts_node_child_count(n: TSNode) -> u32;
    fn ts_node_child(n: TSNode, i: u32) -> TSNode;
    fn ts_node_start_byte(n: TSNode) -> u32;
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

fn wide_source() -> &'static str {
    // Stray tokens end up as flat children of one ERROR node (repeat rules are balanced into small
    // nodes, so only error nodes get more than 255 children).
    Box::leak(format!("const a = 1;\n{}\nconst b = 1;\n", ") ".repeat(300)).into_boxed_str())
}

fn languages() -> Vec<(&'static str, *const TSLanguage, &'static str)> {
    use tree_sitter_lite_rs as g;
    let f = |l: tree_sitter_language::LanguageFn| unsafe { (l.into_raw())() as *const TSLanguage };
    vec![
        (
            "javascript-errors",
            f(g::javascript::LANGUAGE),
            wide_source(),
        ),
        (
            "javascript",
            f(g::javascript::LANGUAGE),
            "class A extends B {\n  constructor(x, y) { super(); this.v = [1, 2, 3].map(a => a * 2); }\n  get n() { return `t${this.v.length}`; }\n}\nconst o = { a: 1, b: { c: [1,2,{d:3}] }, e() { return 1 } };\nfor (const k in o) { console.log(k, o[k]); }\n// trailing comment\n",
        ),
        (
            "typescript",
            f(g::typescript::LANGUAGE_TYPESCRIPT),
            "interface P<T> { x: T; y?: number }\nfunction f<T extends P<number>>(a: T, ...r: string[]): void {\n  let z: Array<T> = [a];\n  if (a.x > 1) { return; } else { z.push(a as T); }\n}\nenum E { A, B = 2 }\nexport default class C implements P<string> { x = ''; }\n",
        ),
        (
            "python",
            f(g::python::LANGUAGE),
            "import os\nclass A(B):\n    def f(self, a, *args, b=2, **kw):\n        \"\"\"doc\"\"\"\n        return [x for x in range(a) if x % 2] + list(args)\n\nasync def g():\n    async with x as y:\n        await y\n    # comment\n    print(f'{a!r} {b}')\n",
        ),
        (
            "rust",
            f(g::rust::LANGUAGE),
            "use std::collections::HashMap;\n#[derive(Debug)]\nstruct S<'a> { x: &'a str, y: Vec<u8> }\nimpl<'a> S<'a> {\n    fn new(x: &'a str) -> Self { S { x, y: vec![1, 2, 3] } }\n    fn go(&self) -> Option<i32> { match self.x.len() { 0 => None, n => Some(n as i32) } }\n}\nfn main() { let m: HashMap<_, _> = HashMap::new(); println!(\"{:?}\", m); }\n",
        ),
        (
            "go",
            f(g::go::LANGUAGE),
            "package main\nimport \"fmt\"\ntype T struct { A int; B string }\nfunc (t *T) M(x int) (int, error) {\n\tfor i := 0; i < x; i++ { fmt.Println(i) }\n\treturn x, nil\n}\n",
        ),
        (
            "java",
            f(g::java::LANGUAGE),
            "package p;\nimport java.util.List;\npublic class C<T> extends B implements I {\n  private final List<T> xs = new ArrayList<>();\n  @Override public int f(int a, String... s) throws Exception { return a > 0 ? a : -a; }\n}\n",
        ),
        (
            "c",
            f(g::c::LANGUAGE),
            "#include <stdio.h>\nstruct s { int a; char *b; };\nstatic int f(int x, const char *y) {\n  for (int i = 0; i < x; i++) { printf(\"%s %d\\n\", y, i); }\n  return x ? 1 : 0;\n}\nint main(void) { return f(2, \"a\"); }\n",
        ),
    ]
}

unsafe fn ts_node_start_byte_of(n: TSNode) -> u32 {
    unsafe { ts_node_start_byte(n) }
}

fn cstr(p: *const c_char) -> Option<String> {
    if p.is_null() {
        None
    } else {
        Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
    }
}

type NodeKey = (usize, usize, usize, [u32; 4]);
fn node_key(n: TSNode) -> NodeKey {
    (n.id as usize, n.tree as usize, 0, n.context)
}

/// The cursor stack as plain data.
fn stack_of(c: &TSTreeCursor) -> Vec<(usize, u32, u32, u32, u32, u32, u32)> {
    unsafe {
        let tc = &*(c as *const TSTreeCursor as *const TreeCursor);
        let mut v = vec![(
            tc.tree as usize,
            tc.root_alias_symbol as u32,
            0,
            0,
            0,
            0,
            tc.stack.size,
        )];
        for i in 0..tc.stack.size {
            let e = *tc.stack.contents.add(i as usize);
            v.push((
                e.subtree as usize,
                e.position.bytes,
                e.position.extent.row,
                e.position.extent.column,
                e.child_index,
                e.structural_child_index,
                e.descendant_index,
            ));
        }
        v
    }
}

#[derive(PartialEq, Debug)]
struct View {
    stack: Vec<(usize, u32, u32, u32, u32, u32, u32)>,
    node: NodeKey,
    parent: NodeKey,
    depth: u32,
    descendant: u32,
    field: (u16, Option<String>),
    status: (u16, bool, bool, bool, Vec<u16>),
}

unsafe fn view(c: &TSTreeCursor, rust: bool) -> View {
    unsafe {
        let mut f = 0u16;
        let (mut l, mut n, mut s) = (false, false, false);
        let mut st = [0u16; 4];
        let mut sc = 4u32;
        let (node, parent, depth, descendant, id, name);
        if rust {
            node = ts_tree_cursor_current_node(c);
            parent = ts_tree_cursor_parent_node(c);
            depth = ts_tree_cursor_current_depth(c);
            descendant = ts_tree_cursor_current_descendant_index(c);
            id = ts_tree_cursor_current_field_id(c);
            name = cstr(ts_tree_cursor_current_field_name(c));
            ts_tree_cursor_current_status(
                c,
                &mut f,
                &mut l,
                &mut n,
                &mut s,
                st.as_mut_ptr(),
                &mut sc,
            );
        } else {
            node = c_ref_tree_cursor_current_node(c);
            parent = c_ref_tree_cursor_parent_node(c);
            depth = c_ref_tree_cursor_current_depth(c);
            descendant = c_ref_tree_cursor_current_descendant_index(c);
            id = c_ref_tree_cursor_current_field_id(c);
            name = cstr(c_ref_tree_cursor_current_field_name(c));
            c_ref_tree_cursor_current_status(
                c,
                &mut f,
                &mut l,
                &mut n,
                &mut s,
                st.as_mut_ptr(),
                &mut sc,
            );
        }
        View {
            stack: stack_of(c),
            node: node_key(node),
            parent: node_key(parent),
            depth,
            descendant,
            field: (id, name),
            status: (f, l, n, s, st[..sc as usize].to_vec()),
        }
    }
}

#[test]
fn random_walks_match_c() {
    let mut steps = 0u64;
    unsafe {
        let parser = ts_parser_new();
        for (name, lang, src) in languages() {
            assert!(ts_parser_set_language(parser, lang));
            let tree = ts_parser_parse_string(
                parser,
                std::ptr::null(),
                src.as_ptr() as *const c_char,
                src.len() as u32,
            );
            let root = ts_tree_root_node(tree);
            for seed in 1..=300u64 {
                let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ name.len() as u64 | 1);
                // Start at the root or at a random child.
                let start = if rng.below(3) == 0 && ts_node_child_count(root) > 0 {
                    ts_node_child(root, rng.below(ts_node_child_count(root) as u64) as u32)
                } else {
                    root
                };
                let mut a = ts_tree_cursor_new(start);
                let mut b = c_ref_tree_cursor_new(start);
                let mut maybe_hidden = false;
                let mut origin = start;
                let mut spare_origin = start;
                let mut spare_a = ts_tree_cursor_copy(&a);
                let mut spare_b = c_ref_tree_cursor_copy(&b);
                assert_eq!(view(&a, true), view(&b, false), "{name} start");
                for step in 0..60 {
                    let op = rng.below(16);
                    let ctx = format!("{name} seed {seed} step {step} op {op}");
                    let size = src.len() as u64;
                    match op {
                        0 => assert_eq!(
                            ts_tree_cursor_goto_first_child(&mut a),
                            c_ref_tree_cursor_goto_first_child(&mut b),
                            "{ctx}"
                        ),
                        1 => assert_eq!(
                            ts_tree_cursor_goto_last_child(&mut a),
                            c_ref_tree_cursor_goto_last_child(&mut b),
                            "{ctx}"
                        ),
                        2 | 3 => assert_eq!(
                            ts_tree_cursor_goto_next_sibling(&mut a),
                            c_ref_tree_cursor_goto_next_sibling(&mut b),
                            "{ctx}"
                        ),
                        4 | 5 => assert_eq!(
                            ts_tree_cursor_goto_previous_sibling(&mut a),
                            c_ref_tree_cursor_goto_previous_sibling(&mut b),
                            "{ctx}"
                        ),
                        6 => assert_eq!(
                            ts_tree_cursor_goto_parent(&mut a),
                            c_ref_tree_cursor_goto_parent(&mut b),
                            "{ctx}"
                        ),
                        7 => {
                            let i = rng.below(if src.len() > 1000 { 700 } else { 80 }) as u32;
                            ts_tree_cursor_goto_descendant(&mut a, i);
                            c_ref_tree_cursor_goto_descendant(&mut b, i);
                        }
                        8 => {
                            let byte = rng.below(size + 3) as u32;
                            assert_eq!(
                                ts_tree_cursor_goto_first_child_for_byte(&mut a, byte),
                                c_ref_tree_cursor_goto_first_child_for_byte(&mut b, byte),
                                "{ctx}"
                            );
                        }
                        9 => {
                            let p = TSPoint {
                                row: rng.below(12) as u32,
                                column: rng.below(30) as u32,
                            };
                            assert_eq!(
                                ts_tree_cursor_goto_first_child_for_point(&mut a, p),
                                c_ref_tree_cursor_goto_first_child_for_point(&mut b, p),
                                "{ctx}"
                            );
                        }
                        10 => assert_eq!(
                            ts_tree_cursor_goto_first_child_internal(&mut a),
                            c_ref_tree_cursor_goto_first_child_internal(&mut b),
                            "{ctx}"
                        ),
                        11 => assert_eq!(
                            ts_tree_cursor_goto_last_child_internal(&mut a),
                            c_ref_tree_cursor_goto_last_child_internal(&mut b),
                            "{ctx}"
                        ),
                        12 => assert_eq!(
                            ts_tree_cursor_goto_next_sibling_internal(&mut a),
                            c_ref_tree_cursor_goto_next_sibling_internal(&mut b),
                            "{ctx}"
                        ),
                        13 => assert_eq!(
                            ts_tree_cursor_goto_previous_sibling_internal(&mut a),
                            c_ref_tree_cursor_goto_previous_sibling_internal(&mut b),
                            "{ctx}"
                        ),
                        14 => {
                            // Save and restore through a copy.
                            if rng.below(2) == 0 {
                                ts_tree_cursor_delete(&mut spare_a);
                                c_ref_tree_cursor_delete(&mut spare_b);
                                spare_a = ts_tree_cursor_copy(&a);
                                spare_b = c_ref_tree_cursor_copy(&b);
                                spare_origin = origin;
                            } else {
                                ts_tree_cursor_reset_to(&mut a, &spare_a);
                                c_ref_tree_cursor_reset_to(&mut b, &spare_b);
                                origin = spare_origin;
                            }
                        }
                        _ => {
                            let n = if rng.below(2) == 0 { root } else { start };
                            ts_tree_cursor_reset(&mut a, n);
                            c_ref_tree_cursor_reset(&mut b, n);
                            origin = n;
                        }
                    }
                    assert_eq!(view(&a, true), view(&b, false), "{ctx}");
                    // Independent of the C: a visible node's descendant index, given to a fresh cursor,
                    // must lead back to the same node (this is what the reverse-traversal fix restored).
                    if op == 15 {
                        maybe_hidden = false;
                    }
                    if (10..=13).contains(&op) || op == 14 {
                        maybe_hidden = true;
                    }
                    if !maybe_hidden {
                        let mut fresh = ts_tree_cursor_new(origin);
                        ts_tree_cursor_goto_descendant(
                            &mut fresh,
                            ts_tree_cursor_current_descendant_index(&a),
                        );
                        let (x, y) = (
                            ts_tree_cursor_current_node(&a),
                            ts_tree_cursor_current_node(&fresh),
                        );
                        assert_eq!(
                            (x.id, ts_node_start_byte_of(x)),
                            (y.id, ts_node_start_byte_of(y)),
                            "{ctx} descendant index"
                        );
                        ts_tree_cursor_delete(&mut fresh);
                    }
                    steps += 1;
                }
                ts_tree_cursor_delete(&mut a);
                c_ref_tree_cursor_delete(&mut b);
                ts_tree_cursor_delete(&mut spare_a);
                c_ref_tree_cursor_delete(&mut spare_b);
            }
            ts_tree_delete(tree);
        }
        ts_parser_delete(parser);
    }
    assert!(steps > 100_000);
}

/// Walks backwards over every child of a node with 600 children, from the last to the first.
///
/// Upstream 0.26.13 compared `(int8_t)child_index` with -1 in the backward iterator, ending the
/// walk at child 511 (and 255). It was fixed upstream (9a15b6afa) and in the port and the reference
/// (rust/patches, ADR-013); no grammar produces nodes that flat, so the tree is built by hand.
#[test]
fn previous_sibling_walk_over_wide_node_reaches_first_child() {
    use tree_sitter_runtime_rs::{array::Array, length::*, subtree::*, tree::*};
    unsafe {
        let lang = common::language();
        // A visible symbol for the leaves, and one for the parent.
        let leaf_sym = (1..(*lang).symbol_count as u16)
            .find(|&y| {
                let m = tree_sitter_runtime_rs::language::ts_language_symbol_metadata(lang, y);
                m.visible && m.named
            })
            .unwrap();
        let mut pool = ts_subtree_pool_new(32);
        let mut kids: Array<Subtree> = Array::new();
        for _ in 0..600 {
            let len = Length {
                bytes: 1,
                extent: TSPoint { row: 0, column: 1 },
            };
            kids.push(ts_subtree_new_leaf(
                &mut pool,
                leaf_sym,
                length_zero(),
                len,
                0,
                1,
                false,
                false,
                false,
                lang,
            ));
        }
        let root = ts_subtree_from_mut(ts_subtree_new_node(leaf_sym, &mut kids, 0, lang));
        let range = TSRange {
            start_point: TSPoint { row: 0, column: 0 },
            end_point: TSPoint {
                row: u32::MAX,
                column: u32::MAX,
            },
            start_byte: 0,
            end_byte: u32::MAX,
        };
        let tree = ts_tree_new(root, lang, &range, 1);
        let mut a = ts_tree_cursor_new(ts_tree_root_node(tree));
        let mut b = c_ref_tree_cursor_new(ts_tree_root_node(tree));
        assert!(
            ts_tree_cursor_goto_first_child(&mut a) && c_ref_tree_cursor_goto_first_child(&mut b)
        );
        let mut forward = 0;
        while ts_tree_cursor_goto_next_sibling(&mut a) {
            assert!(c_ref_tree_cursor_goto_next_sibling(&mut b));
            forward += 1;
        }
        assert!(!c_ref_tree_cursor_goto_next_sibling(&mut b));
        assert_eq!(forward, 599);
        let mut back = 0;
        loop {
            let (x, y) = (
                ts_tree_cursor_goto_previous_sibling(&mut a),
                c_ref_tree_cursor_goto_previous_sibling(&mut b),
            );
            assert_eq!(x, y, "step back {back}");
            assert_eq!(view(&a, true), view(&b, false), "step back {back}");
            if !x {
                break;
            }
            back += 1;
        }
        eprintln!("walked {forward} forward, {back} back");
        assert_eq!(
            back, 599,
            "the walk must reach the first child (upstream fix 9a15b6afa)"
        );
        ts_tree_cursor_delete(&mut a);
        c_ref_tree_cursor_delete(&mut b);
        ts_tree_delete(tree);
        ts_subtree_pool_delete(&mut pool);
    }
}
