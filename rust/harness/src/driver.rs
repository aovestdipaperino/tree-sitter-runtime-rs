// Included into both `mod rt` (the Rust runtime) and `mod rf` (the renamed C reference), after the
// bindgen bindings, so everything here is compiled once against each runtime's own symbols.
use crate::{EditArgs, RangeArgs};
use core::ffi::{c_char, c_void, CStr};
use std::fmt::Write as _;

extern "C" {
    fn free(p: *mut c_void);
}

struct Reader {
    data: *const u8,
    len: usize,
    chunk: usize,
}

unsafe extern "C" fn read_cb(payload: *mut c_void, byte_index: u32, _position: TSPoint, bytes_read: *mut u32) -> *const c_char {
    let r = &*(payload as *const Reader);
    let i = byte_index as usize;
    if i >= r.len {
        *bytes_read = 0;
        return core::ptr::null();
    }
    let n = r.chunk.min(r.len - i);
    *bytes_read = n as u32;
    r.data.add(i) as *const c_char
}

pub struct Session {
    parser: *mut TSParser,
    tree: *mut TSTree,
}

/// What one parse produced: the S-expression and a node-by-node dump, and the changed ranges
/// against the previous tree (incremental parses only).
pub type Outcome = (String, Vec<[u32; 6]>);

impl Session {
    pub unsafe fn new(lang: *const c_void) -> Session {
        let parser = ts_parser_new();
        assert!(ts_parser_set_language(parser, lang as *const TSLanguage), "language rejected");
        Session { parser, tree: core::ptr::null_mut() }
    }

    pub unsafe fn set_ranges(&mut self, ranges: &[RangeArgs]) -> bool {
        let v: Vec<TSRange> = ranges
            .iter()
            .map(|r| TSRange {
                start_point: TSPoint { row: r.start.0, column: r.start.1 },
                end_point: TSPoint { row: r.end.0, column: r.end.1 },
                start_byte: r.start_byte,
                end_byte: r.end_byte,
            })
            .collect();
        ts_parser_set_included_ranges(self.parser, v.as_ptr(), v.len() as u32)
    }

    pub unsafe fn edit(&mut self, e: &EditArgs) {
        if self.tree.is_null() {
            return;
        }
        let edit = TSInputEdit {
            start_byte: e.start_byte,
            old_end_byte: e.old_end_byte,
            new_end_byte: e.new_end_byte,
            start_point: TSPoint { row: e.start.0, column: e.start.1 },
            old_end_point: TSPoint { row: e.old_end.0, column: e.old_end.1 },
            new_end_point: TSPoint { row: e.new_end.0, column: e.new_end.1 },
        };
        ts_tree_edit(self.tree, &edit);
    }

    pub unsafe fn parse(&mut self, data: &[u8], encoding: u32, chunk: usize, incremental: bool) -> Outcome {
        let reader = Reader { data: data.as_ptr(), len: data.len(), chunk: chunk.max(1) };
        let input = TSInput { payload: &reader as *const Reader as *mut c_void, read: Some(read_cb), encoding, decode: None };
        let old = if incremental { self.tree } else { core::ptr::null_mut() };
        let new = ts_parser_parse(self.parser, old, input);
        if new.is_null() {
            return ("<null tree>".to_string(), Vec::new());
        }
        let mut changed = Vec::new();
        if !old.is_null() {
            let mut n: u32 = 0;
            let p = ts_tree_get_changed_ranges(old, new, &mut n);
            for i in 0..n as usize {
                let r = &*p.add(i);
                changed.push([r.start_byte, r.end_byte, r.start_point.row, r.start_point.column, r.end_point.row, r.end_point.column]);
            }
            free(p as *mut c_void);
        }
        let dump = dump_tree(new);
        if !self.tree.is_null() {
            ts_tree_delete(self.tree);
        }
        self.tree = new;
        (dump, changed)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        unsafe {
            if !self.tree.is_null() {
                ts_tree_delete(self.tree);
            }
            ts_parser_delete(self.parser);
        }
    }
}

unsafe fn dump_tree(tree: *const TSTree) -> String {
    let root = ts_tree_root_node(tree);
    let mut out = String::new();
    let s = ts_node_string(root);
    out.push_str(&CStr::from_ptr(s).to_string_lossy());
    out.push('\n');
    free(s as *mut c_void);

    let mut cur = ts_tree_cursor_new(root);
    let mut depth = 0usize;
    'walk: loop {
        let n = ts_tree_cursor_current_node(&cur);
        let f = ts_tree_cursor_current_field_name(&cur);
        let field = if f.is_null() { String::new() } else { CStr::from_ptr(f).to_string_lossy().into_owned() };
        let ty = CStr::from_ptr(ts_node_type(n)).to_string_lossy().into_owned();
        let (sp, ep) = (ts_node_start_point(n), ts_node_end_point(n));
        let _ = writeln!(
            out,
            "{:indent$}{field}:{ty} sym={} gsym={} named={} missing={} extra={} error={} haserr={} chg={} bytes={}-{} pt={}:{}-{}:{} ps={} nps={} kids={} desc={}",
            "",
            ts_node_symbol(n),
            ts_node_grammar_symbol(n),
            ts_node_is_named(n) as u8,
            ts_node_is_missing(n) as u8,
            ts_node_is_extra(n) as u8,
            ts_node_is_error(n) as u8,
            ts_node_has_error(n) as u8,
            ts_node_has_changes(n) as u8,
            ts_node_start_byte(n),
            ts_node_end_byte(n),
            sp.row,
            sp.column,
            ep.row,
            ep.column,
            ts_node_parse_state(n),
            ts_node_next_parse_state(n),
            ts_node_child_count(n),
            ts_node_descendant_count(n),
            indent = depth * 2,
        );
        if ts_tree_cursor_goto_first_child(&mut cur) {
            depth += 1;
            continue;
        }
        loop {
            if ts_tree_cursor_goto_next_sibling(&mut cur) {
                break;
            }
            if !ts_tree_cursor_goto_parent(&mut cur) {
                break 'walk;
            }
            depth -= 1;
        }
    }
    ts_tree_cursor_delete(&mut cur);
    out
}
