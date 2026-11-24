//! Differential harness for the Rust runtime: the same public-API scenarios run against the Rust
//! runtime (`rt`) and the unmodified C reference with renamed symbols (`rf`), on the lite grammars,
//! comparing S-expressions, a node-by-node dump, and changed ranges. See rust/DECISIONS.md ADR-007.
use core::ffi::c_void;
use std::{env, fs, path::PathBuf, process::ExitCode};

pub struct EditArgs {
    pub start_byte: u32,
    pub old_end_byte: u32,
    pub new_end_byte: u32,
    pub start: (u32, u32),
    pub old_end: (u32, u32),
    pub new_end: (u32, u32),
}
pub struct RangeArgs {
    pub start_byte: u32,
    pub end_byte: u32,
    pub start: (u32, u32),
    pub end: (u32, u32),
}

pub struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused,
    clippy::all
)]
mod rt {
    include!(env!("BINDINGS_RS"));
    include!("driver.rs");
}
#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused,
    clippy::all
)]
mod rf {
    include!(concat!(env!("OUT_DIR"), "/bindings_ref.rs"));
    include!("driver.rs");
}

use tree_sitter_runtime_rs as _; // link the Rust runtime (and the C units it still contains)

type Out = (String, Vec<[u32; 6]>);

#[derive(Default)]
struct Stats {
    compared: usize,
    mismatches: usize,
    by_kind: std::collections::BTreeMap<&'static str, (usize, usize)>,
}

impl Stats {
    fn check(&mut self, kind: &'static str, grammar: &str, idx: usize, a: &Out, b: &Out) {
        self.compared += 1;
        let e = self.by_kind.entry(kind).or_default();
        e.0 += 1;
        if a != b {
            self.mismatches += 1;
            e.1 += 1;
            if self.mismatches <= 6 {
                let (la, lb) = (
                    a.0.lines().collect::<Vec<_>>(),
                    b.0.lines().collect::<Vec<_>>(),
                );
                let at = (0..la.len().max(lb.len())).find(|&i| la.get(i) != lb.get(i));
                println!("MISMATCH [{kind}] {grammar} input #{idx}");
                if let Some(i) = at {
                    println!(
                        "  Rust runtime: {:?}\n  C reference : {:?}",
                        la.get(i),
                        lb.get(i)
                    );
                } else {
                    println!("  changed ranges differ: {:?} vs {:?}", a.1, b.1);
                }
            }
        }
    }
}

fn unhex(h: &str) -> Option<Vec<u8>> {
    (0..h.len() / 2)
        .map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).ok())
        .collect()
}

fn point_of(text: &[u8], off: usize) -> (u32, u32) {
    let pre = &text[..off];
    let row = pre.iter().filter(|&&b| b == b'\n').count() as u32;
    let col = off - pre.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    (row, col as u32)
}

fn char_boundaries(text: &[u8]) -> Vec<usize> {
    (0..=text.len())
        .filter(|&i| i == text.len() || (text[i] & 0xC0) != 0x80)
        .collect()
}

fn languages() -> Vec<(&'static str, *const c_void)> {
    use tree_sitter_lite_rs as g;
    let f = |l: tree_sitter_language::LanguageFn| unsafe { (l.into_raw())() as *const c_void };
    vec![
        ("javascript", f(g::javascript::LANGUAGE)),
        ("typescript", f(g::typescript::LANGUAGE_TYPESCRIPT)),
        ("tsx", f(g::tsx::LANGUAGE)),
        ("python", f(g::python::LANGUAGE)),
        ("rust", f(g::rust::LANGUAGE)),
        ("go", f(g::go::LANGUAGE)),
        ("java", f(g::java::LANGUAGE)),
        ("c", f(g::c::LANGUAGE)),
    ]
}

fn utf16(text: &[u8], big: bool) -> Vec<u8> {
    String::from_utf8_lossy(text)
        .encode_utf16()
        .flat_map(|u| {
            if big {
                u.to_be_bytes()
            } else {
                u.to_le_bytes()
            }
        })
        .collect()
}

fn main() -> ExitCode {
    let golden_dir = env::var("GOLDEN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../../tree-sitter-lite-rs/tests/golden")
        });
    let max_inputs: usize = env::var("MAX_INPUTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(80);
    let edits: usize = env::var("EDITS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    let only: Option<Vec<String>> = env::var("GRAMMARS")
        .ok()
        .map(|v| v.split(',').map(String::from).collect());
    let mut rng = Rng(env::var("SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0x9E3779B97F4A7C15));
    let snippets: [&str; 14] = [
        "x", "\n", "  ", "(", ")", "{", "}", "\"", "'", "`", "/*", "é", "😀", "\\",
    ];

    let mut stats = Stats::default();
    for (name, lang) in languages() {
        if only.as_ref().is_some_and(|o| !o.iter().any(|x| x == name)) {
            continue;
        }
        let path = golden_dir.join(format!("{name}.golden"));
        let inputs: Vec<Vec<u8>> = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
            .lines()
            .filter_map(|l| unhex(l.split_once('\t')?.0))
            .take(max_inputs)
            .collect();
        let before = stats.compared;
        for (idx, text) in inputs.iter().enumerate() {
            unsafe {
                // whole-buffer and chunked UTF-8 input
                for (kind, chunk) in [
                    ("utf8", text.len().max(1)),
                    ("utf8-chunk1", 1),
                    ("utf8-chunk7", 7),
                ] {
                    let a = rt::Session::new(lang).parse(text, 0, chunk, false);
                    let b = rf::Session::new(lang).parse(text, 0, chunk, false);
                    stats.check(kind, name, idx, &a, &b);
                }
                // cancelled by the progress callback, then resumed
                for limit in [1u32, 2, 5, 40] {
                    let a = rt::Session::new(lang).parse_cancelling(text, limit);
                    let b = rf::Session::new(lang).parse_cancelling(text, limit);
                    stats.check("cancel-resume", name, idx, &a, &b);
                }
                // UTF-16 input (little and big endian)
                for (kind, enc, big) in [("utf16le", 1u32, false), ("utf16be", 2u32, true)] {
                    let data = utf16(text, big);
                    let a = rt::Session::new(lang).parse(&data, enc, 64, false);
                    let b = rf::Session::new(lang).parse(&data, enc, 64, false);
                    stats.check(kind, name, idx, &a, &b);
                }
                // invalid UTF-8: overwrite a few bytes with arbitrary ones
                if !text.is_empty() {
                    let mut bad = text.clone();
                    for _ in 0..1 + rng.below(3) {
                        let at = rng.below(bad.len());
                        bad[at] = 0x80 | (rng.next() as u8);
                    }
                    let a = rt::Session::new(lang).parse(&bad, 0, bad.len(), false);
                    let b = rf::Session::new(lang).parse(&bad, 0, bad.len(), false);
                    stats.check("invalid-utf8", name, idx, &a, &b);
                }
                // included ranges
                let bounds = char_boundaries(text);
                if bounds.len() > 8 {
                    let mut cuts: Vec<usize> =
                        (0..4).map(|_| bounds[rng.below(bounds.len())]).collect();
                    cuts.sort();
                    let ranges = [
                        RangeArgs {
                            start_byte: cuts[0] as u32,
                            end_byte: cuts[1] as u32,
                            start: point_of(text, cuts[0]),
                            end: point_of(text, cuts[1]),
                        },
                        RangeArgs {
                            start_byte: cuts[2] as u32,
                            end_byte: cuts[3] as u32,
                            start: point_of(text, cuts[2]),
                            end: point_of(text, cuts[3]),
                        },
                    ];
                    let (mut sa, mut sb) = (rt::Session::new(lang), rf::Session::new(lang));
                    let (ra, rb) = (sa.set_ranges(&ranges), sb.set_ranges(&ranges));
                    assert_eq!(ra, rb, "set_included_ranges disagrees");
                    let a = sa.parse(text, 0, text.len().max(1), false);
                    let b = sb.parse(text, 0, text.len().max(1), false);
                    stats.check("included-ranges", name, idx, &a, &b);
                }
                // incremental edits: both runtimes see the same edit sequence and reparse with the old tree
                let (mut sa, mut sb) = (rt::Session::new(lang), rf::Session::new(lang));
                let mut cur = text.clone();
                sa.parse(&cur, 0, cur.len().max(1), false);
                sb.parse(&cur, 0, cur.len().max(1), false);
                for _ in 0..edits {
                    let bounds = char_boundaries(&cur);
                    let si = rng.below(bounds.len());
                    let start = bounds[si];
                    let end = bounds[(si + rng.below(4)).min(bounds.len() - 1)];
                    let ins = snippets[rng.below(snippets.len())].as_bytes();
                    let e = EditArgs {
                        start_byte: start as u32,
                        old_end_byte: end as u32,
                        new_end_byte: (start + ins.len()) as u32,
                        start: point_of(&cur, start),
                        old_end: point_of(&cur, end),
                        new_end: {
                            let mut t = cur.clone();
                            t.splice(start..end, ins.iter().copied());
                            point_of(&t, start + ins.len())
                        },
                    };
                    cur.splice(start..end, ins.iter().copied());
                    sa.edit(&e);
                    sb.edit(&e);
                    let a = sa.parse(&cur, 0, cur.len().max(1), true);
                    let b = sb.parse(&cur, 0, cur.len().max(1), true);
                    stats.check("incremental", name, idx, &a, &b);
                }
            }
        }
        println!(
            "{name:<11} {:>5} comparisons over {} inputs",
            stats.compared - before,
            inputs.len()
        );
    }
    println!("--");
    for (k, (n, bad)) in &stats.by_kind {
        println!("  {k:<16} {n:>6} compared, {bad} mismatches");
    }
    println!(
        "TOTAL: {} compared, {} mismatches",
        stats.compared, stats.mismatches
    );
    if stats.mismatches == 0 {
        println!("RESULT: PASS");
        ExitCode::SUCCESS
    } else {
        println!("RESULT: FAIL");
        ExitCode::FAILURE
    }
}
