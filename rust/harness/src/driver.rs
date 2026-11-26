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
    log: Box<String>,
}

unsafe extern "C" fn log_cb(payload: *mut c_void, ty: TSLogType, msg: *const c_char) {
    let log = &mut *(payload as *mut String);
    let _ = writeln!(log, "LOG {ty} {}", CStr::from_ptr(msg).to_string_lossy());
}

/// What one parse produced: the S-expression and a node-by-node dump, and the changed ranges
/// against the previous tree (incremental parses only).
pub type Outcome = (String, Vec<[u32; 6]>);

impl Session {
    pub unsafe fn new(lang: *const c_void) -> Session {
        let parser = ts_parser_new();
        assert!(ts_parser_set_language(parser, lang as *const TSLanguage), "language rejected");
        let mut log = Box::new(String::new());
        ts_parser_set_logger(parser, TSLogger { payload: &mut *log as *mut String as *mut c_void, log: Some(log_cb) });
        Session { parser, tree: core::ptr::null_mut(), log }
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
        self.log.clear();
        let new = ts_parser_parse(self.parser, old, input);
        if new.is_null() {
            return (format!("<null tree>\n{}", self.log), Vec::new());
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
        let mut dump = dump_tree(new);
        dump.push_str(&self.log);
        if !self.tree.is_null() {
            ts_tree_delete(self.tree);
        }
        self.tree = new;
        (dump, changed)
    }
}

struct Cancel {
    calls: u32,
    limit: u32,
    seen: Vec<(u32, bool)>,
}

unsafe extern "C" fn progress_cb(state: *mut TSParseState) -> bool {
    let st = &*state;
    let c = &mut *(st.payload as *mut Cancel);
    c.seen.push((st.current_byte_offset, st.has_error));
    c.calls += 1;
    c.calls >= c.limit
}

impl Session {
    /// Parse with a progress callback that cancels after `limit` calls, then resume the same
    /// parse without one. The callback's observations and both results are the outcome.
    pub unsafe fn parse_cancelling(&mut self, data: &[u8], limit: u32) -> Outcome {
        let reader = Reader { data: data.as_ptr(), len: data.len(), chunk: data.len().max(1) };
        let input = |r: &Reader| TSInput { payload: r as *const Reader as *mut c_void, read: Some(read_cb), encoding: 0, decode: None };
        let mut cancel = Cancel { calls: 0, limit, seen: Vec::new() };
        self.log.clear();
        let options = TSParseOptions { payload: &mut cancel as *mut Cancel as *mut c_void, progress_callback: Some(progress_cb) };
        let mut out = String::new();
        let mut tree = ts_parser_parse_with_options(self.parser, core::ptr::null(), input(&reader), options);
        let _ = writeln!(out, "CANCEL first_null={} seen={:?}", tree.is_null(), cancel.seen);
        if tree.is_null() {
            tree = ts_parser_parse(self.parser, core::ptr::null(), input(&reader));
        }
        if tree.is_null() {
            out.push_str("<null tree>\n");
        } else {
            out.push_str(&dump_tree(tree));
            ts_tree_delete(tree);
        }
        out.push_str(&self.log);
        (out, Vec::new())
    }
}

struct QCancel {
    calls: u32,
    seen: Vec<u32>,
}

unsafe extern "C" fn qprogress_cb(state: *mut TSQueryCursorState) -> bool {
    let c = &mut *((*state).payload as *mut QCancel);
    c.seen.push((*state).current_byte_offset);
    c.calls += 1;
    c.calls >= 3
}

/// Run one cursor to exhaustion, in match or capture order, returning a text record.
unsafe fn run_cursor(q: *const TSQuery, root: TSNode, mode: u32) -> String {
    let c = ts_query_cursor_new();
    let mut out = String::new();
    match mode {
        1 => ts_query_cursor_set_match_limit(c, 3),
        2 => {
            let (a, b) = (ts_node_start_byte(root) + 5, ts_node_end_byte(root).saturating_sub(5));
            let _ = writeln!(out, "byte_range={}", ts_query_cursor_set_byte_range(c, a, b) as u8);
        }
        3 => ts_query_cursor_set_max_start_depth(c, 2),
        7 => ts_query_cursor_set_match_limit(c, 1),
        14 => {
            let m = (ts_node_start_byte(root) + ts_node_end_byte(root)) / 2;
            let _ = writeln!(out, "empty_range={}", ts_query_cursor_set_byte_range(c, m, m) as u8);
        }
        15 => {
            let (a, b) = (ts_node_start_byte(root) + 9, ts_node_start_byte(root) + 2);
            let _ = writeln!(out, "reversed={}", ts_query_cursor_set_byte_range(c, a, b) as u8);
            let _ = writeln!(out, "reversed_contain={}", ts_query_cursor_set_containing_byte_range(c, a, b) as u8);
        }
        8 => ts_query_cursor_set_match_limit(c, 2),
        9 => ts_query_cursor_set_max_start_depth(c, 0),
        10 => ts_query_cursor_set_max_start_depth(c, 1),
        11 => ts_query_cursor_set_max_start_depth(c, 3),
        12 => {
            let (sp, ep) = (ts_node_start_point(root), ts_node_end_point(root));
            let r = ts_query_cursor_set_point_range(c, TSPoint { row: sp.row + 1, column: 0 }, TSPoint { row: ep.row.saturating_sub(1), column: 0 });
            let _ = writeln!(out, "point_range={}", r as u8);
        }
        13 => {
            let (sp, ep) = (ts_node_start_point(root), ts_node_end_point(root));
            let r = ts_query_cursor_set_containing_point_range(c, TSPoint { row: sp.row, column: 2 }, TSPoint { row: ep.row.saturating_sub(1), column: 1 });
            let _ = writeln!(out, "contain_point={}", r as u8);
        }
        4 => {
            let (a, b) = (ts_node_start_byte(root) + 3, ts_node_end_byte(root).saturating_sub(3));
            let _ = writeln!(out, "contain={}", ts_query_cursor_set_containing_byte_range(c, a, b) as u8);
        }
        _ => {}
    }
    if mode == 16 {
        let mut cancel = QCancel { calls: 0, seen: Vec::new() };
        let opts = TSQueryCursorOptions { payload: &mut cancel as *mut QCancel as *mut c_void, progress_callback: Some(qprogress_cb) };
        ts_query_cursor_exec_with_options(c, q, root, &opts);
        let mut m: TSQueryMatch = core::mem::zeroed();
        while ts_query_cursor_next_match(c, &mut m) {
            let _ = writeln!(out, "match id={} pat={} n={}", m.id, m.pattern_index, m.capture_count);
        }
        let _ = writeln!(out, "progress seen={:?}", cancel.seen);
        ts_query_cursor_delete(c);
        return out;
    }
    ts_query_cursor_exec(c, q, root);
    let mut m: TSQueryMatch = core::mem::zeroed();
    let mut idx = 0u32;
    if mode == 5 {
        let mut ci = 0u32;
        while ts_query_cursor_next_capture(c, &mut m, &mut ci) {
            let cap = &*m.captures.add(ci as usize);
            let _ = writeln!(out, "cap id={} pat={} n={} idx={} {}", m.id, m.pattern_index, m.capture_count, cap.index, nd(cap.node));
            if ci % 5 == 3 {
                ts_query_cursor_remove_match(c, m.id);
            }
        }
    } else {
        while ts_query_cursor_next_match(c, &mut m) {
            let _ = write!(out, "match id={} pat={}", m.id, m.pattern_index);
            for i in 0..m.capture_count as usize {
                let cap = &*m.captures.add(i);
                let _ = write!(out, " {}={}", cap.index, nd(cap.node));
            }
            out.push('\n');
            idx += 1;
            if mode == 6 && idx % 4 == 0 {
                ts_query_cursor_remove_match(c, m.id);
            }
        }
    }
    let _ = writeln!(out, "exceeded={}", ts_query_cursor_did_exceed_match_limit(c) as u8);
    ts_query_cursor_delete(c);
    out
}

impl Session {
    /// Compile `src` as a query for `lang`, describe the query and run it over the current tree.
    pub unsafe fn query_report(&self, lang: *const TSLanguage, src: &str) -> String {
        let mut out = String::new();
        let mut off: u32 = 0;
        let mut err: TSQueryError = 0;
        let q = ts_query_new(lang, src.as_ptr() as *const c_char, src.len() as u32, &mut off, &mut err);
        if q.is_null() {
            let _ = writeln!(out, "ERR type={} offset={}", err as u32, off);
            return out;
        }
        let (np, nc, ns) = (ts_query_pattern_count(q), ts_query_capture_count(q), ts_query_string_count(q));
        let _ = writeln!(out, "patterns={np} captures={nc} strings={ns}");
        for i in 0..nc {
            let mut len = 0u32;
            let name = ts_query_capture_name_for_id(q, i, &mut len);
            let _ = write!(out, " cap{i}={}", CStr::from_ptr(name).to_string_lossy());
        }
        for i in 0..ns {
            let mut len = 0u32;
            let v = ts_query_string_value_for_id(q, i, &mut len);
            let _ = write!(out, " str{i}={}", CStr::from_ptr(v).to_string_lossy());
        }
        out.push('\n');
        for p in 0..np {
            let mut n = 0u32;
            let steps = ts_query_predicates_for_pattern(q, p, &mut n);
            let _ = write!(out, "pat{p} bytes={}-{} rooted={} nonlocal={} preds=[", ts_query_start_byte_for_pattern(q, p), ts_query_end_byte_for_pattern(q, p), ts_query_is_pattern_rooted(q, p) as u8, ts_query_is_pattern_non_local(q, p) as u8);
            for i in 0..n as usize {
                let st = &*steps.add(i);
                let _ = write!(out, "{}:{},", st.type_ as u32, st.value_id);
            }
            out.push_str("] quant=");
            for c in 0..nc {
                let _ = write!(out, "{}", ts_query_capture_quantifier_for_id(q, p, c) as u32);
            }
            out.push_str(" guaranteed=");
            for st in 0..24 {
                let _ = write!(out, "{}", ts_query_is_pattern_guaranteed_at_step(q, st) as u8);
            }
            out.push('\n');
        }
        if !self.tree.is_null() {
            let root = ts_tree_root_node(self.tree);
            for mode in 0..17u32 {
                let _ = writeln!(out, "-- mode {mode}");
                out.push_str(&run_cursor(q, root, mode));
            }
            // disabled capture / pattern, then run again
            if nc > 0 {
                ts_query_disable_capture(q, c"zzz".as_ptr(), 3);
                let mut len = 0u32;
                let first = ts_query_capture_name_for_id(q, 0, &mut len);
                ts_query_disable_capture(q, first, len);
                out.push_str("-- disabled capture 0\n");
                out.push_str(&run_cursor(q, root, 0));
            }
            if np > 1 {
                ts_query_disable_pattern(q, 1);
                out.push_str("-- disabled pattern 1\n");
                out.push_str(&run_cursor(q, root, 0));
            }
        }
        // A copy keeps the disabled captures and patterns and must behave like the original.
        let qc = ts_query_copy(q);
        let _ = writeln!(
            out,
            "-- copy patterns={} captures={} strings={}",
            ts_query_pattern_count(qc),
            ts_query_capture_count(qc),
            ts_query_string_count(qc)
        );
        if !self.tree.is_null() {
            let root = ts_tree_root_node(self.tree);
            for mode in [0u32, 1, 5, 9] {
                let _ = writeln!(out, "-- copy mode {mode}");
                out.push_str(&run_cursor(qc, root, mode));
            }
        }
        ts_query_delete(qc);
        ts_query_delete(q);
        out
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

unsafe fn nd(n: TSNode) -> String {
    if ts_node_is_null(n) {
        return "-".to_string();
    }
    format!("{}@{}-{}", CStr::from_ptr(ts_node_type(n)).to_string_lossy(), ts_node_start_byte(n), ts_node_end_byte(n))
}

/// Navigation APIs on one node, as text.
unsafe fn nav_line(n: TSNode, lang: *const TSLanguage) -> String {
    let (sb, eb) = (ts_node_start_byte(n), ts_node_end_byte(n));
    let (sp, ep) = (ts_node_start_point(n), ts_node_end_point(n));
    let mut s = String::new();
    let _ = write!(
        s,
        "  nav par={} ns={} ps={} nns={} pns={} c0={} nc0={} cl={} fcb={} fncb={} dbr={} ndbr={} dpr={} ndpr={}",
        nd(ts_node_parent(n)),
        nd(ts_node_next_sibling(n)),
        nd(ts_node_prev_sibling(n)),
        nd(ts_node_next_named_sibling(n)),
        nd(ts_node_prev_named_sibling(n)),
        nd(ts_node_child(n, 0)),
        nd(ts_node_named_child(n, 0)),
        nd(ts_node_child(n, ts_node_child_count(n).saturating_sub(1))),
        nd(ts_node_first_child_for_byte(n, sb + 1)),
        nd(ts_node_first_named_child_for_byte(n, sb + 1)),
        nd(ts_node_descendant_for_byte_range(n, sb, eb)),
        nd(ts_node_named_descendant_for_byte_range(n, sb.saturating_add(1).min(eb), eb)),
        nd(ts_node_descendant_for_point_range(n, sp, ep)),
        nd(ts_node_named_descendant_for_point_range(n, sp, ep)),
    );
    let fields = ts_language_field_count(lang);
    for f in 1..=fields {
        let c = ts_node_child_by_field_id(n, f as TSFieldId);
        if !ts_node_is_null(c) {
            let _ = write!(s, " f{f}={}", nd(c));
        }
    }
    for i in 0..ts_node_child_count(n).min(4) {
        let nm = ts_node_field_name_for_child(n, i);
        if !nm.is_null() {
            let _ = write!(s, " fnc{i}={}", CStr::from_ptr(nm).to_string_lossy());
        }
    }
    for i in 0..ts_node_named_child_count(n).min(4) {
        let nm = ts_node_field_name_for_named_child(n, i);
        if !nm.is_null() {
            let _ = write!(s, " fnnc{i}={}", CStr::from_ptr(nm).to_string_lossy());
        }
    }
    let by_name = ts_node_child_by_field_name(n, c"body".as_ptr(), 4);
    let _ = write!(s, " body={}", nd(by_name));
    let root = ts_tree_root_node(n.tree);
    let _ = write!(s, " cwd={} eq={}", nd(ts_node_child_with_descendant(root, n)), ts_node_eq(n, n) as u8);
    let mut m = n;
    let edit = TSInputEdit { start_byte: sb, old_end_byte: sb, new_end_byte: sb + 3, start_point: sp, old_end_point: sp, new_end_point: TSPoint { row: sp.row, column: sp.column + 3 } };
    ts_node_edit(&mut m, &edit);
    let _ = write!(s, " edit={}-{}", ts_node_start_byte(m), ts_node_start_point(m).column);
    s
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
    let mut count = 0usize;
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
        count += 1;
        if count <= 150 {
            out.push_str(&nav_line(n, ts_tree_language(tree)));
            out.push('\n');
        }
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
