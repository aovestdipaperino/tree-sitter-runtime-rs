# Decisions

Architecture decision records for the port of the tree-sitter C runtime to Rust. Numbered in order,
never renumbered; a changed decision gets a new ADR that supersedes the old one.

## ADR-001: Port tree-sitter 0.26.13
Status: accepted (2026-11-24)
Context: tokensave's lock pins tree-sitter 0.26.9, the newest on crates.io here is 0.26.13, and a
fresh build resolves 0.26.13. The C differs by about 360 lines in six files (`api.h`, `parser.c`,
`query.c`, `subtree.c`, `tree_cursor.c`, `array.h`, `alloc.h`).
Decision: the reference is the C runtime of the `tree-sitter` 0.26.13 crate (`src/`, `include/`,
`binding_rust/`, kept unmodified in this repo). Revisit when a newer 0.26.x ships.
Consequences: the Rust binding in `binding_rust/` is the one that matches this `api.h`.

## ADR-002: Same C ABI, as a drop-in
Status: accepted (2026-11-24)
Context: the `tree-sitter` Rust binding, every grammar and every other host reach the runtime
through the `ts_*` functions of `include/tree_sitter/api.h`.
Decision: the Rust runtime exports exactly those symbols with the same signatures and struct
layouts (`extern "C"`, `#[repr(C)]`), so the binding and all grammars link unchanged. An idiomatic
Rust API is a later reshape, after the lift.
Consequences: public structs (`TSNode`, `TSPoint`, `TSRange`, `TSInput`, `TSTreeCursor`,
`TSQueryMatch`, ...) keep their layout forever; internal structs are free once nothing in C uses them.

## ADR-003: WASM is out of scope
Status: accepted (2026-11-24)
Context: `wasm_store.c` (1,908 lines) needs wasmtime, is an optional feature and is unused by tokensave.
Decision: the wasmtime-backed implementation is not ported. The runtime still needs the
`ts_wasm_store_*` symbols, which without the `wasm` feature are about 100 lines of dummy functions at
the end of `wasm_store.c` (they return false or 0). Those are built as C at first and become a small
Rust stub unit at the end.
Consequences: parity with the default C build (no `wasm` feature); `ts_language_is_wasm` is always false.

## ADR-004: Equivalence bar
Status: accepted (2026-11-24)
Decision: exact equality with the C runtime on: syntax trees (S-expressions including `ERROR` and
`MISSING` nodes), node byte and point ranges, field names, changed ranges after edits, query
captures and match order, error recovery results. Speed within 15% of C on real files.
Consequences: a mismatch is a bug in the port, never in the cases. Error recovery costs and
tie-breaking must be reproduced exactly (they decide which tree wins).

## ADR-005: Queries are ported last
Status: accepted (2026-11-24)
Decision: `query.c` (4,421 lines) stays C behind the front door until the parser, tree and cursor
are Rust; it is its own final wave.
Consequences: until then the Rust runtime links `query.c` as C, which only needs the public tree
and node API plus `ts_language_*`.

## ADR-006: Faithful translation first, contained unsafe
Status: accepted (2026-11-24)
Decision: translate with the original's structure, types and error paths. Raw pointers are
confined to a few modules (`subtree`, `stack`, `array`, `lexer` buffers) with a safe layer around
them; every `unsafe` block carries a `// SAFETY:` comment. No redesign of the memory model before
the differential tests pass; idiomatic cleanups only after the lift, one change at a time.
Consequences: subtrees stay tagged unions with inline small subtrees and reference counting.

## ADR-007: Oracle is a whole-library differential harness
Status: accepted (2026-11-24)
Context: rust-anything's `analyze` reports 0 bridgeable exports (every API takes `TSParser *`,
`TSTree *` or `TSNode` by value), so its per-function `record`, `diff` and `shadow` cannot wrap
the units. Its plan, waves, ADRs and `map` still apply.
Decision: the reference C runtime is built with every external symbol renamed, linked next to the
Rust runtime into one harness, and both are driven through the public C API on all eight lite
grammars: corpora, real files, mutation, prefixes, incremental edits, token soup, queries. Trees,
ranges, changed ranges and captures are compared. This is the same method that found the real
bugs in the grammar transpiler.
Consequences: every wave is verified by that harness; a unit is `done` when the harness passes
with it ported and `gate`-style checks (fmt, clippy `-D warnings`, `// SAFETY:` comments) hold.

## ADR-008: Delivery
Status: accepted (2026-11-24)
Decision: a new crate `tree-sitter-runtime-rs` exports the C ABI, plus a fork of the `tree-sitter`
Rust binding that depends on it instead of compiling C, wired in through `[patch.crates-io]`.
Tested first against tokensave's suite. The existing `tree-sitter-runtime` repo (types only) is
left alone.

## ADR-009: Targets and checkpoints
Status: accepted (2026-11-24)
Decision: Linux and macOS first (the CI platforms), Windows later. Stop for the user's go-ahead
after each wave: leaf modules, lexer and language, tangled core, node and cursor, queries.

## ADR-010: Finish the lite-tier fix list first
Status: accepted and done (2026-11-24)
Decision: the lite-tier release work (tests, Linux differential CI, upstream constants, crates.io
release 0.2.1) ran before this port started.

## ADR-011: Mixed build, one unit at a time
Status: proposed by the agent, not yet confirmed (2026-11-24)
Context: the runtime's internal structs (`Subtree`, `TSParser`, `Stack`, `Length`, ...) are shared
by every `.c` file, and `lib.c` includes them all as one translation unit.
Decision: build the C units as separate objects instead of `lib.c`. A ported unit's `.c` file is
dropped from the build and its Rust replacement exports the same symbols (`extern "C"`), over
`#[repr(C)]` mirrors of the shared structs, checked against the C layouts (`size_of`, `align_of`,
`offset_of`) by a generated layout test. Header-only `static inline` helpers stay in the C headers
until the last C unit that includes them is gone. The runtime is therefore runnable and testable
after every unit.
Consequences: layout drift is caught by the layout test, not by crashes. Confirm or veto.

## Known upstream quirks reproduced on purpose (ADR-004)
- `ts_assert` stays on in release builds (the C build never defines `NDEBUG`); `ts_assert!` is a
  plain `assert!`. A failed assertion aborts with a Rust panic message instead of glibc's.
- Bugs that upstream has since fixed are not reproduced; see ADR-013.

## ADR-013: oracle is 0.26.13 plus listed upstream fixes
Status: accepted (2026-11-24, on the instruction "fix evident bugs on rust side")
Amends ADR-004. Three evident bugs in 0.26.13 are fixed upstream (v0.27.x): UTF-16 surrogate trail
byte order (`unicode.h`), `(int8_t)child_index == -1` in the reverse cursor iterator, and the
reverse iterator dropping `descendant_index` (`tree_cursor.c`). The Rust port fixes them. The
reference C is pristine `src/` copied to `OUT_DIR/ref-src` and patched with `rust/patches/*.patch`
(`patch -p2`); pristine `src/` is never modified. Both `runtime/build.rs` and `harness/build.rs`
use the patched copy, so exact equality with the oracle still holds. Further post-0.26.13 fixes
(`_compare_int` overflow, `repeat_depth` width, `ts_language_symbol_for_name` prefix match, null
memcpy in `array_assign`, embedded-NUL symbol names, skipped-character error cost) are candidates
for the same mechanism or for rebasing to v0.27.2; undecided.

## ADR-012: variadic `TSLexer.log` stays C
Status: accepted (2026-11-24, by necessity)
Stable Rust cannot define a C-variadic function, and `TSLexer.log` is one. `runtime/c/lexer_log.c`
formats the message and calls back into Rust (`ts_lexer_log_emit`). Removing it waits for `c_variadic`.
