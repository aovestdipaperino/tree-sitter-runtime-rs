# tree-sitter-runtime-rs

A port of the [tree-sitter](https://github.com/tree-sitter/tree-sitter) C runtime (v0.27.2) to Rust,
exporting the same C ABI so the `tree-sitter` bindings and every grammar link unchanged.

Status: the runtime is fully ported (the only C left is the variadic `TSLexer.log` shim) and verified against the C original; not yet published. See [rust/MAP.md](rust/MAP.md) for the plan and state,
[rust/DECISIONS.md](rust/DECISIONS.md) for the decisions, and
[rust/GLOSSARY.md](rust/GLOSSARY.md) for the vocabulary.

`src/` and `include/` are the unmodified upstream v0.27.2 sources (`binding_rust/` is still 0.26.13 and gets forked later), kept as the
reference the Rust port is verified against (MIT, see `LICENSE.tree-sitter`).
