//! The tree port against the C original (src/tree.c compiled with renamed symbols), on trees
//! produced by the real parser: copy, edit, included ranges, root nodes, changed ranges and the
//! dot-graph output must agree.
mod common;
use common::*;
use core::ffi::{CStr, c_char, c_int, c_void};
use tree_sitter_runtime_rs::{api::*, tree::*};

unsafe extern "C" {
    fn c_ref_tree_copy(t: *const TSTree) -> *mut TSTree;
    fn c_ref_tree_delete(t: *mut TSTree);
    fn c_ref_tree_root_node(t: *const TSTree) -> TSNode;
    fn c_ref_tree_root_node_with_offset(t: *const TSTree, b: u32, p: TSPoint) -> TSNode;
    fn c_ref_tree_language(t: *const TSTree) -> *const TSLanguage;
    fn c_ref_tree_edit(t: *mut TSTree, e: *const TSInputEdit);
    fn c_ref_tree_included_ranges(t: *const TSTree, n: *mut u32) -> *mut TSRange;
    fn c_ref_tree_get_changed_ranges(
        a: *const TSTree,
        b: *const TSTree,
        n: *mut u32,
    ) -> *mut TSRange;
    fn c_ref_tree_print_dot_graph(t: *const TSTree, fd: c_int);

    fn ts_parser_new() -> *mut c_void;
    fn ts_parser_delete(p: *mut c_void);
    fn ts_parser_set_language(p: *mut c_void, l: *const TSLanguage) -> bool;
    fn ts_parser_set_included_ranges(p: *mut c_void, r: *const TSRange, n: u32) -> bool;
    fn ts_parser_parse_string(
        p: *mut c_void,
        old: *const TSTree,
        s: *const c_char,
        n: u32,
    ) -> *mut TSTree;
    fn ts_node_string(n: TSNode) -> *mut c_char;
    fn ts_node_start_byte(n: TSNode) -> u32;
    fn ts_node_end_byte(n: TSNode) -> u32;
    fn free(p: *mut c_void);
    fn tmpfile() -> *mut c_void;
    fn fileno(f: *mut c_void) -> c_int;
    fn lseek(fd: c_int, off: i64, whence: c_int) -> i64;
    fn read(fd: c_int, buf: *mut c_void, n: usize) -> isize;
    fn fclose(f: *mut c_void) -> c_int;
}

const SOURCES: [&str; 4] = [
    "function f(a, b) {\n  return a + b;\n}\nconst x = f(1, 2);\n",
    "class A extends B {\n  constructor() { super(); this.v = [1, 2, 3].map(x => x * 2); }\n  get n() { return `t${this.v.length}`; }\n}\n",
    "let s = 'hello';\nif (s) { while (true) { break; } } else { s += \"x\"; }\n// comment\n/* block */ var y = /re[a-z]+/g.test(s);\n",
    "const o = { a: 1, b: { c: [1,2,{d:3}] }, e() { return 1 } };\nfor (const k in o) { console.log(k, o[k]); }\n",
];

fn point_of(text: &[u8], byte: usize) -> TSPoint {
    let (mut row, mut col) = (0u32, 0u32);
    for &b in &text[..byte] {
        if b == b'\n' {
            row += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    TSPoint { row, column: col }
}

unsafe fn parse(parser: *mut c_void, old: *const TSTree, text: &[u8]) -> *mut TSTree {
    unsafe {
        ts_parser_parse_string(
            parser,
            old,
            text.as_ptr() as *const c_char,
            text.len() as u32,
        )
    }
}

unsafe fn sexp(n: TSNode) -> String {
    unsafe {
        let p = ts_node_string(n);
        let s = CStr::from_ptr(p).to_string_lossy().into_owned();
        free(p as *mut c_void);
        s
    }
}

unsafe fn ranges(p: *mut TSRange, n: u32) -> Vec<(u32, u32, u32, u32, u32, u32)> {
    unsafe {
        let v = (0..n)
            .map(|i| {
                let r = *p.add(i as usize);
                (
                    r.start_byte,
                    r.end_byte,
                    r.start_point.row,
                    r.start_point.column,
                    r.end_point.row,
                    r.end_point.column,
                )
            })
            .collect();
        free(p as *mut c_void);
        v
    }
}

unsafe fn dot_of(f: impl Fn(c_int)) -> String {
    unsafe {
        let file = tmpfile();
        let fd = fileno(file);
        f(fd);
        lseek(fd, 0, 0);
        let mut buf = vec![0u8; 1 << 16];
        let n = read(fd, buf.as_mut_ptr() as *mut c_void, buf.len());
        fclose(file);
        let s = String::from_utf8_lossy(&buf[..n.max(0) as usize]).into_owned();
        // `%p` values differ between runs: number them in order of first appearance.
        let mut seen: Vec<&str> = Vec::new();
        let mut out = String::new();
        let mut rest = s.as_str();
        while let Some(i) = rest.find("0x") {
            out.push_str(&rest[..i]);
            let tail = &rest[i..];
            let end = 2 + tail[2..]
                .find(|c: char| !c.is_ascii_hexdigit())
                .unwrap_or(tail.len() - 2);
            let tok = &tail[..end];
            let idx = seen.iter().position(|t| *t == tok).unwrap_or_else(|| {
                seen.push(tok);
                seen.len() - 1
            });
            out.push_str(&format!("<ptr{idx}>"));
            rest = &tail[end..];
        }
        out.push_str(rest);
        out
    }
}

fn root_dump(t: *const TSTree) -> Vec<u64> {
    dump_c(unsafe { (*t).root })
}

#[test]
fn tree_operations_match_c() {
    let lang = language();
    let mut rng = Rng(0x5eed_1234_abcd_ef01);
    let mut edits_done = 0;
    unsafe {
        let parser = ts_parser_new();
        assert!(ts_parser_set_language(parser, lang));
        for src in SOURCES {
            let text = src.as_bytes().to_vec();
            let base = parse(parser, std::ptr::null(), &text);
            assert!(!base.is_null());

            // Copy, language, root nodes.
            let (a, b) = (ts_tree_copy(base), c_ref_tree_copy(base));
            assert_eq!(root_dump(a), root_dump(b));
            assert_eq!((*a).language, (*b).language);
            assert_eq!(ts_tree_language(a), c_ref_tree_language(b));
            assert_eq!(sexp(ts_tree_root_node(a)), sexp(c_ref_tree_root_node(b)));
            let off = TSPoint { row: 2, column: 3 };
            let (na, nb) = (
                ts_tree_root_node_with_offset(a, 5, off),
                c_ref_tree_root_node_with_offset(b, 5, off),
            );
            assert_eq!(
                (ts_node_start_byte(na), ts_node_end_byte(na)),
                (ts_node_start_byte(nb), ts_node_end_byte(nb))
            );
            let (mut la, mut lb) = (0, 0);
            let (ra, rb) = (
                ts_tree_included_ranges(a, &mut la),
                c_ref_tree_included_ranges(b, &mut lb),
            );
            assert_eq!(ranges(ra, la), ranges(rb, lb));

            // Dot graph.
            let (da, db) = (
                dot_of(|fd| ts_tree_print_dot_graph(a, fd)),
                dot_of(|fd| c_ref_tree_print_dot_graph(b, fd)),
            );
            assert!(da.starts_with("digraph tree {"));
            assert_eq!(da, db);

            // Edits, with a reparse and the changed ranges.
            for _ in 0..150 {
                let start = rng.below(text.len() as u64) as usize;
                let old_len = rng.below((text.len() - start).min(12) as u64 + 1) as usize;
                let ins: &[u8] =
                    [&b""[..], b"x", b"\n", b"foo(1)", b" + 2", b"{}\n  "][rng.below(6) as usize];
                let mut new_text = text[..start].to_vec();
                new_text.extend_from_slice(ins);
                new_text.extend_from_slice(&text[start + old_len..]);
                let edit = TSInputEdit {
                    start_byte: start as u32,
                    old_end_byte: (start + old_len) as u32,
                    new_end_byte: (start + ins.len()) as u32,
                    start_point: point_of(&text, start),
                    old_end_point: point_of(&text, start + old_len),
                    new_end_point: point_of(&new_text, start + ins.len()),
                };
                let (ea, eb) = (ts_tree_copy(base), c_ref_tree_copy(base));
                ts_tree_edit(ea, &edit);
                c_ref_tree_edit(eb, &edit);
                assert_eq!(root_dump(ea), root_dump(eb), "edit {edit:?}");
                let (mut la, mut lb) = (0, 0);
                let (ra, rb) = (
                    ts_tree_included_ranges(ea, &mut la),
                    c_ref_tree_included_ranges(eb, &mut lb),
                );
                assert_eq!(ranges(ra, la), ranges(rb, lb));

                let (new_a, new_b) = (parse(parser, ea, &new_text), parse(parser, eb, &new_text));
                assert_eq!(
                    sexp(ts_tree_root_node(new_a)),
                    sexp(c_ref_tree_root_node(new_b))
                );
                let (mut la, mut lb) = (0, 0);
                let (ra, rb) = (
                    ts_tree_get_changed_ranges(ea, new_a, &mut la),
                    c_ref_tree_get_changed_ranges(eb, new_b, &mut lb),
                );
                assert_eq!(
                    ranges(ra, la),
                    ranges(rb, lb),
                    "changed ranges for {edit:?}"
                );
                edits_done += 1;
                for t in [ea, new_a] {
                    ts_tree_delete(t);
                }
                for t in [eb, new_b] {
                    c_ref_tree_delete(t);
                }
            }
            ts_tree_delete(a);
            c_ref_tree_delete(b);
            ts_tree_delete(base);

            // Included ranges.
            let inc = [
                TSRange {
                    start_point: TSPoint { row: 0, column: 0 },
                    end_point: TSPoint { row: 0, column: 8 },
                    start_byte: 0,
                    end_byte: 8,
                },
                TSRange {
                    start_point: TSPoint { row: 0, column: 12 },
                    end_point: TSPoint { row: 0, column: 30 },
                    start_byte: 12,
                    end_byte: 30,
                },
            ];
            assert!(ts_parser_set_included_ranges(parser, inc.as_ptr(), 2));
            let t = parse(parser, std::ptr::null(), &text);
            let (mut la, mut lb) = (0, 0);
            let (ra, rb) = (
                ts_tree_included_ranges(t, &mut la),
                c_ref_tree_included_ranges(t, &mut lb),
            );
            assert_eq!(la, 2);
            assert_eq!(ranges(ra, la), ranges(rb, lb));
            let (c1, c2) = (ts_tree_copy(t), c_ref_tree_copy(t));
            assert_eq!(root_dump(c1), root_dump(c2));
            // Editing a tree that has included ranges moves the ranges too.
            for _ in 0..10 {
                let start = rng.below(text.len() as u64) as u32;
                let edit = TSInputEdit {
                    start_byte: start,
                    old_end_byte: start + rng.below(6) as u32,
                    new_end_byte: start + rng.below(6) as u32,
                    start_point: TSPoint {
                        row: 0,
                        column: start,
                    },
                    old_end_point: TSPoint {
                        row: 0,
                        column: start + 2,
                    },
                    new_end_point: TSPoint {
                        row: rng.below(2) as u32,
                        column: start + 3,
                    },
                };
                ts_tree_edit(c1, &edit);
                c_ref_tree_edit(c2, &edit);
                let (mut la, mut lb) = (0, 0);
                let (ra, rb) = (
                    ts_tree_included_ranges(c1, &mut la),
                    c_ref_tree_included_ranges(c2, &mut lb),
                );
                assert_eq!(
                    ranges(ra, la),
                    ranges(rb, lb),
                    "included ranges after {edit:?}"
                );
                assert_eq!(root_dump(c1), root_dump(c2));
            }
            ts_tree_delete(c1);
            c_ref_tree_delete(c2);
            ts_tree_delete(t);
            assert!(ts_parser_set_included_ranges(parser, std::ptr::null(), 0));
        }
        ts_parser_delete(parser);
        // Deleting a null tree is allowed.
        ts_tree_delete(std::ptr::null_mut());
        c_ref_tree_delete(std::ptr::null_mut());
    }
    assert!(edits_done >= 500);
}
