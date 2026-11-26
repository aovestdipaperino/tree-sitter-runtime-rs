# tree-sitter-runtime-rs

The [tree-sitter](https://github.com/tree-sitter/tree-sitter) C runtime (v0.27.2), ported to Rust.
It exports the same C ABI (`ts_parser_new`, `ts_parser_parse`, `ts_query_new`, ...), so the
`tree-sitter` Rust bindings and every generated grammar link against it unchanged, with no C
library in between.

The only C left is a 15-line file for `TSLexer.log`, which is a variadic function that stable Rust
cannot define. The crate therefore needs a C compiler at build time.

Using it from the `tree-sitter` bindings: depend on the fork in the repository (`rust/binding`,
the upstream 0.27.2 bindings pointed at this runtime) and patch it in:

```toml
[patch.crates-io]
tree-sitter = { git = "https://github.com/aovestdipaperino/tree-sitter-runtime-rs" }
```

## How it is verified

Every C unit was ported one at a time and compared with the upstream C, built with its symbols
renamed and linked next to the Rust runtime. A harness drives both through the public C API over
eight real grammars (trees, changed ranges, incremental edits, included ranges, UTF-16, cancel and
resume, node navigation, queries) and requires identical results. Building that reference needs the
`reference` feature and a checkout of the repository; the published crate does not include it.

Out of scope: the wasm store (the no-wasm stubs are ported, the real thing is not).

MIT licensed. Upstream copyright is in `LICENSE`.
