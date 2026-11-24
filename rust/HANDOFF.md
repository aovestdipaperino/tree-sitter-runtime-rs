# Handoff: runtime port

State at checkpoint 2 (waves 0 to 3, `lexer.c` and `language.c` done). Everything below is committed in this repo.

## Done
Ported to Rust, each with differential tests against the C original (`rust/runtime/tests/`):
`point.c` + `point.h`, `length.h`, `alloc.c`, `array.h`, `reduce_action.h`, the `unicode.h`
decoders, `error_costs.h`/`host.h`/`ts_assert.h`/`endian.h` (constants and `cfg`). Struct layouts
(`api.rs`, `Length`, `ReduceAction`, `Array`) are checked field by field against C `sizeof` and
`offsetof` (`tests/layout.rs`). The mixed build (`ported.txt` lists the C files dropped) passes the
whole-library harness: 7,039 comparisons against the renamed C reference, 0 mismatches.

## How to verify
    cd rust
    cargo test -p tree-sitter-runtime-rs
    cargo clippy --all-targets -- -D warnings
    cargo build --release -p runtime-diff-harness && ./target/release/runtime-diff-harness

Note the harness is weak on `ts_point_edit` boundary cases (a seeded off-by-one there did not change
any corpus result); the unit test in `tests/helpers.rs` is what pins it.

## Checkpoint 2 additions
`lexer.c` (Rust; `Lexer` keeps the C layout, shared with the C parser) and `language.c` plus all the
inline helpers of `language.h` (including the lookahead iterator). `TSLanguage` and the `parser.h`
types are in `parser_abi.rs`. Differential tests: `tests/lexer.rs` runs 3,000 random operation
scripts (all four encodings, chunking, included ranges, logging) against the renamed C lexer and
compares full state after every step; `tests/language.rs` compares every function on every state and
symbol of the 8 real grammars. Seeded bugs in both are caught.
The variadic `TSLexer.log` cannot be written in stable Rust, so `runtime/c/lexer_log.c` (15 lines)
stays C; remove it when `c_variadic` stabilises. Latent C hazard reproduced as is: advancing a lexer
left at EOF with a non-null chunk (possible after an empty included range) reads one range past
the array; the test avoids that state.

## Subtree (checkpoint 3, first unit)
`subtree.rs`: `Subtree`/`MutableSubtree` are unions of an inline `u64` (bitfield accessors, little-endian
only) and a heap pointer; `SubtreeHeapData` keeps the C layout with its 11 flags as a `u16`. Tests:
bit layout against the C compiler for 200k random inline values and every heap flag; setters vs C bitfield
assignment; 3,000 lockstep random operation scripts (leaves, nodes, error nodes, edits, make_mut,
compare, strings, array ops, external scanner state) with the full state of every live tree compared
after each step through the C accessors; compress on left-leaning chains. Eight seeded mutations,
including repeat-depth and edit-boundary ones, are caught (one equivalent mutant, `>` vs `>=` on
repeat depth, cannot be). `subtree.c` is dropped from the C build, so the C stack/parser/tree code now
runs on the Rust subtree: the whole-library harness still shows 0 mismatches.
Known layout difference (harmless): C `SubtreeInlineData` has alignment 2, the port 8; it only occurs
inside the 8-aligned unions.

## Next (rest of checkpoint 3)
`stack`, `tree`,
`tree_cursor`, `get_changed_ranges`, `parser`, then `node`. Risks to read first in `MAP.md`:
`Subtree` bitfields and low-bit pointer tagging need hand-written accessors with layout tests;
C and Rust units must share those structs while mixed. `query.c` stays C until the last wave.

## Open questions for the user
- ADR-011 (mixed build, one unit at a time) is proposed, not confirmed.
- ADR-001 ports tree-sitter 0.26.13, not tokensave's pinned 0.26.9: flagged for veto.
- No GitHub remote exists for this repo yet; creating a public repo needs a go-ahead.
