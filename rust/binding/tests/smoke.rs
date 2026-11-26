//! The upstream bindings (v0.27.2) running on the pure Rust runtime.
use streaming_iterator::StreamingIterator;
use tree_sitter::{InputEdit, Language, Parser, Point, Query, QueryCursor};

fn js() -> Language {
    tree_sitter_lite_rs::language_javascript().into()
}

#[test]
fn parse_and_edit() {
    let mut parser = Parser::new();
    parser.set_language(&js()).unwrap();
    let mut src = String::from("function f(a) { return a + 1; }\n");
    let mut tree = parser.parse(&src, None).unwrap();
    assert_eq!(tree.root_node().kind(), "program");
    assert!(!tree.root_node().has_error());
    let at = src.find("1;").unwrap();
    src.replace_range(at..at + 1, "b");
    tree.edit(&InputEdit {
        start_byte: at,
        old_end_byte: at + 1,
        new_end_byte: at + 1,
        start_position: Point::new(0, at),
        old_end_position: Point::new(0, at + 1),
        new_end_position: Point::new(0, at + 1),
    });
    let tree2 = parser.parse(&src, Some(&tree)).unwrap();
    assert_eq!(
        tree2.root_node().to_sexp(),
        parser.parse(&src, None).unwrap().root_node().to_sexp()
    );
    assert!(!tree.changed_ranges(&tree2).collect::<Vec<_>>().is_empty());
}

#[test]
fn query_and_copy() {
    let mut parser = Parser::new();
    parser.set_language(&js()).unwrap();
    let src = "function f(a) {} function g(b) {}";
    let tree = parser.parse(src, None).unwrap();
    let query = Query::new(&js(), "(function_declaration name: (identifier) @n)").unwrap();
    let mut cursor = QueryCursor::new();
    let mut names = Vec::new();
    let mut it = cursor.matches(&query, tree.root_node(), src.as_bytes());
    while let Some(m) = it.next() {
        names.push(src[m.nodes_for_capture_index(0).next().unwrap().byte_range()].to_string());
    }
    assert_eq!(names, ["f", "g"]);
    assert!(Query::new(&js(), "(nope)").is_err());
}

#[test]
fn version_and_abi() {
    assert!(js().abi_version() >= 13);
}
