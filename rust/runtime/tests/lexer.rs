//! The Rust lexer against the C lexer (src/lexer.c compiled with renamed symbols), driven by the
//! same random scripts. After every step the complete lexer state, the values the operations
//! return, and every log message must be identical.
#![cfg(feature = "reference")]
use core::ffi::{CStr, c_char, c_void};
use std::mem::MaybeUninit;
use tree_sitter_runtime_rs::{api::*, length::Length, lexer::*};

unsafe extern "C" {
    fn c_ref_lexer_init(l: *mut Lexer);
    fn c_ref_lexer_delete(l: *mut Lexer);
    fn c_ref_lexer_set_input(l: *mut Lexer, input: TSInput);
    fn c_ref_lexer_reset(l: *mut Lexer, p: Length);
    fn c_ref_lexer_start(l: *mut Lexer);
    fn c_ref_lexer_finish(l: *mut Lexer, end: *mut u32);
    fn c_ref_lexer_mark_end(l: *mut Lexer);
    fn c_ref_lexer_set_included_ranges(l: *mut Lexer, r: *const TSRange, n: u32) -> bool;
    fn c_ref_lexer_included_ranges(l: *const Lexer, n: *mut u32) -> *mut TSRange;
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

/// What a lexer reads from, and what it logged.
struct Source {
    bytes: Vec<u8>,
    chunk: usize,
    logs: Vec<String>,
}

unsafe extern "C" fn read(
    payload: *mut c_void,
    byte: u32,
    _p: TSPoint,
    n: *mut u32,
) -> *const c_char {
    let s = unsafe { &*(payload as *const Source) };
    let start = byte as usize;
    if start >= s.bytes.len() {
        unsafe { *n = 0 };
        return core::ptr::null();
    }
    let end = (start + s.chunk).min(s.bytes.len());
    unsafe { *n = (end - start) as u32 };
    unsafe { s.bytes.as_ptr().add(start) as *const c_char }
}

unsafe extern "C" fn log(payload: *mut c_void, _t: TSLogType, msg: *const c_char) {
    let s = unsafe { &mut *(payload as *mut Source) };
    s.logs.push(
        unsafe { CStr::from_ptr(msg) }
            .to_string_lossy()
            .into_owned(),
    );
}

/// A custom decoder: two bytes make one code point (big endian); one trailing byte is an error.
unsafe extern "C" fn custom_decode(s: *const u8, n: u32, cp: *mut i32) -> u32 {
    if n < 2 {
        unsafe { *cp = -1 };
        return n.min(1).max(if n == 0 { 0 } else { 1 });
    }
    unsafe { *cp = (i32::from(*s) << 8) | i32::from(*s.add(1)) };
    2
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Snapshot {
    pos: (u32, u32, u32),
    start: (u32, u32, u32),
    end: (u32, u32, u32),
    chunk_start: u32,
    chunk_size: u32,
    chunk_null: bool,
    lookahead_size: u32,
    did_get_column: bool,
    column: (u32, bool),
    range_count: u32,
    range_index: u32,
    lookahead: i32,
    result_symbol: u16,
}

fn len3(l: Length) -> (u32, u32, u32) {
    (l.bytes, l.extent.row, l.extent.column)
}

fn snap(l: &Lexer) -> Snapshot {
    Snapshot {
        pos: len3(l.current_position),
        start: len3(l.token_start_position),
        end: len3(l.token_end_position),
        chunk_start: l.chunk_start,
        chunk_size: l.chunk_size,
        chunk_null: l.chunk.is_null(),
        lookahead_size: l.lookahead_size,
        did_get_column: l.did_get_column,
        column: (l.column_data.value, l.column_data.valid),
        range_count: l.included_range_count,
        range_index: l.current_included_range_index,
        lookahead: l.data.lookahead,
        result_symbol: l.data.result_symbol,
    }
}

fn ranges_of(l: &Lexer) -> Vec<(u32, u32, u32, u32, u32, u32)> {
    (0..l.included_range_count)
        .map(|i| {
            let r = unsafe { *l.included_ranges.add(i as usize) };
            (
                r.start_byte,
                r.end_byte,
                r.start_point.row,
                r.start_point.column,
                r.end_point.row,
                r.end_point.column,
            )
        })
        .collect()
}

fn text(rng: &mut Rng) -> Vec<u8> {
    const PIECES: [&[u8]; 14] = [
        b"a",
        b"b",
        b" ",
        b"\n",
        b"\n",
        b"\r\n",
        "é".as_bytes(),
        "€".as_bytes(),
        "😀".as_bytes(),
        b"\xef\xbb\xbf",
        b"\xe2\x82",
        b"\xff",
        b"\xf0\x9f",
        b"x",
    ];
    let mut v = Vec::new();
    if rng.below(5) == 0 {
        v.extend_from_slice(b"\xef\xbb\xbf");
    }
    for _ in 0..rng.below(40) {
        v.extend_from_slice(PIECES[rng.below(PIECES.len() as u64) as usize]);
    }
    v
}

fn random_ranges(rng: &mut Rng, len: u32) -> Vec<TSRange> {
    let mut v = Vec::new();
    let mut at = 0u32;
    for _ in 0..rng.below(4) + 1 {
        let start = (at + rng.below(6) as u32).min(len + 2);
        let end = (start + rng.below(12) as u32).min(len + 3);
        // Occasionally out of order or inverted, which must be rejected identically.
        let (start, end) = if rng.below(12) == 0 {
            (end, start)
        } else {
            (start, end)
        };
        v.push(TSRange {
            start_byte: start,
            end_byte: end,
            start_point: TSPoint {
                row: rng.below(3) as u32,
                column: start,
            },
            end_point: TSPoint {
                row: rng.below(3) as u32 + 1,
                column: end,
            },
        });
        at = end;
    }
    v
}

#[test]
fn random_scripts_match_c() {
    let encodings = [
        TSInputEncodingUTF8,
        TSInputEncodingUTF16LE,
        TSInputEncodingUTF16BE,
        TSInputEncodingCustom,
    ];
    let mut steps = 0u64;
    for seed in 1..=3000u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let bytes = text(&mut rng);
        let encoding = encodings[rng.below(4) as usize];
        let chunk = [1usize, 2, 3, 5, 7, 4096][rng.below(6) as usize];
        let mut src_r = Source {
            bytes: bytes.clone(),
            chunk,
            logs: vec![],
        };
        let mut src_c = Source {
            bytes,
            chunk,
            logs: vec![],
        };
        let with_logger = rng.below(3) != 0;

        let mut lr: Box<MaybeUninit<Lexer>> = Box::new(MaybeUninit::uninit());
        let mut lc: Box<MaybeUninit<Lexer>> = Box::new(MaybeUninit::uninit());
        let (pr, pc) = (lr.as_mut_ptr(), lc.as_mut_ptr());
        unsafe {
            ts_lexer_init(pr);
            c_ref_lexer_init(pc);
            let mk = |s: &mut Source| TSInput {
                payload: s as *mut Source as *mut c_void,
                read: Some(read),
                encoding,
                decode: Some(custom_decode),
            };
            ts_lexer_set_input(pr, mk(&mut src_r));
            c_ref_lexer_set_input(pc, mk(&mut src_c));
            if with_logger {
                (*pr).logger = TSLogger {
                    payload: &mut src_r as *mut Source as *mut c_void,
                    log: Some(log),
                };
                (*pc).logger = TSLogger {
                    payload: &mut src_c as *mut Source as *mut c_void,
                    log: Some(log),
                };
            }
            if rng.below(3) == 0 {
                let rs = random_ranges(&mut rng, src_r.bytes.len() as u32);
                let a = ts_lexer_set_included_ranges(pr, rs.as_ptr(), rs.len() as u32);
                let b = c_ref_lexer_set_included_ranges(pc, rs.as_ptr(), rs.len() as u32);
                assert_eq!(a, b, "seed {seed} set_included_ranges");
            }
            ts_lexer_start(pr);
            c_ref_lexer_start(pc);
        }
        let (r, c) = unsafe { (&mut *pr, &mut *pc) };
        assert_eq!(snap(r), snap(c), "seed {seed} after start");

        for step in 0..120 {
            let mut op = rng.below(12);
            // A lexer can be left at EOF (range index == count) with a non-null chunk, e.g. after
            // an empty included range. Advancing then reads one range past the array in the C and
            // in the port alike (undefined behaviour, so the results are garbage), so skip it.
            if op <= 3
                && r.current_included_range_index == r.included_range_count
                && !r.chunk.is_null()
            {
                op = 6;
            }
            let ctx = format!(
                "seed {seed} enc {encoding} chunk {chunk} step {step} op {op} before {:?}",
                snap(c)
            );
            unsafe {
                match op {
                    0..=3 => {
                        let skip = op == 3;
                        (r.data.advance)(&mut r.data, skip);
                        (c.data.advance)(&mut c.data, skip);
                    }
                    4 => {
                        if rng.below(2) == 0 {
                            (r.data.mark_end)(&mut r.data);
                            (c.data.mark_end)(&mut c.data);
                        } else {
                            ts_lexer_mark_end(pr);
                            c_ref_lexer_mark_end(pc);
                        }
                    }
                    5 => {
                        let (a, b) = (
                            (r.data.get_column)(&mut r.data),
                            (c.data.get_column)(&mut c.data),
                        );
                        assert_eq!(a, b, "{ctx} get_column");
                    }
                    6 => {
                        let a = (r.data.eof)(&r.data);
                        let b = (c.data.eof)(&c.data);
                        let a2 = (r.data.is_at_included_range_start)(&r.data);
                        let b2 = (c.data.is_at_included_range_start)(&c.data);
                        assert_eq!((a, a2), (b, b2), "{ctx} predicates");
                    }
                    7 => {
                        let p = Length {
                            bytes: rng.below(src_r.bytes.len() as u64 + 3) as u32,
                            extent: TSPoint {
                                row: rng.below(4) as u32,
                                column: rng.below(20) as u32,
                            },
                        };
                        ts_lexer_reset(pr, p);
                        c_ref_lexer_reset(pc, p);
                    }
                    8 => {
                        ts_lexer_start(pr);
                        c_ref_lexer_start(pc);
                    }
                    9 => {
                        let (mut ea, mut eb) = (rng.below(50) as u32, 0);
                        eb = ea.max(eb);
                        ea = eb;
                        let mut ec = ea;
                        ts_lexer_finish(pr, &mut ea);
                        c_ref_lexer_finish(pc, &mut ec);
                        assert_eq!(ea, ec, "{ctx} finish");
                    }
                    10 => {
                        let rs = random_ranges(&mut rng, src_r.bytes.len() as u32);
                        let a = ts_lexer_set_included_ranges(pr, rs.as_ptr(), rs.len() as u32);
                        let b = c_ref_lexer_set_included_ranges(pc, rs.as_ptr(), rs.len() as u32);
                        assert_eq!(a, b, "{ctx} set_included_ranges");
                    }
                    _ => {
                        let log_fn = r.data.log.unwrap();
                        log_fn(&r.data, c"scan %d %s".as_ptr(), 42i32, c"ok".as_ptr());
                        let log_fn = c.data.log.unwrap();
                        log_fn(&c.data, c"scan %d %s".as_ptr(), 42i32, c"ok".as_ptr());
                    }
                }
            }
            assert_eq!(snap(r), snap(c), "{ctx}");
            assert_eq!(ranges_of(r), ranges_of(c), "{ctx} ranges");
            assert_eq!(src_r.logs, src_c.logs, "{ctx} logs");
            steps += 1;
        }
        unsafe {
            let (mut na, mut nb) = (0u32, 0u32);
            let ra = ts_lexer_included_ranges(pr, &mut na);
            let rb = c_ref_lexer_included_ranges(pc, &mut nb);
            assert_eq!(na, nb);
            assert_eq!(
                std::slice::from_raw_parts(ra, na as usize).len(),
                std::slice::from_raw_parts(rb, nb as usize).len()
            );
            ts_lexer_delete(pr);
            c_ref_lexer_delete(pc);
        }
    }
    assert!(steps > 300_000);
}
