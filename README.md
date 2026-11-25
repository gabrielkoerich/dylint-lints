# dylint-lints

Custom Rust lints for [dylint](https://github.com/trailofbits/dylint), for rules Clippy does not have. Each lint is a folder with its own crate, nightly toolchain and UI test.

| Lint | What it flags |
|------|---------------|
| [`use_in_fn`](use_in_fn/src/lib.rs) | A `use` item inside a function, method or closure body |
| [`long_file`](long_file/src/lib.rs) | A source file with more than `max_lines` lines, 2000 by default |
| [`wide_directory`](wide_directory/src/lib.rs) | A source directory with more than `max_members` files and directories, 15 by default |
| [`module_cycle`](module_cycle/src/lib.rs) | Sibling modules that use each other in a circle |
| [`tangle`](tangle/src/lib.rs) | Types and functions that use each other in a circle across modules, or more than seven of them |
| [`field_leak`](field_leak/src/lib.rs) | Code outside a struct that works on its fields, when the struct keeps some fields to itself |

`use_in_fn` covers the case Clippy leaves open. With `clippy::absolute_paths` (no `crate::a::b` or `std::a::b` in code), `clippy::arbitrary_source_item_ordering` (`mod`, then `use`, then everything else) and `clippy::items_after_statements` (no `use` after a statement), it keeps every import at the top of its module.

## Use it in a project

Dylint loads a lint library from a git or path source named in the workspace metadata. It does not load one from crates.io or as a Cargo dependency, so the crates here are not published.

```toml
[workspace.metadata.dylint]
libraries = [
    { git = "https://github.com/gabrielkoerich/dylint-lints", tag = "v0.1.0", pattern = "use_in_fn" },
]
```

Install the runner once, then run it beside Clippy:

```bash
cargo +stable install cargo-dylint dylint-link --version 6.3.2 --locked
DYLINT_RUSTFLAGS="-D warnings" cargo dylint --all -- --all-targets
```

The first run installs the nightly from the lint's `rust-toolchain.toml` and builds the lint with it. In CI, cache `~/.rustup/toolchains/` and `~/.dylint_drivers/`.

## Toolchain

A dylint lint builds against rustc's internal API, which changes between nightlies, so each lint pins the nightly that its `dylint_linting` version supports. Moving to a new nightly or `dylint_linting` version is one release here, then a tag bump in each project that loads it.

## Develop

```bash
cd use_in_fn
cargo test        # runs the UI test in ui/
```

A lint's UI test compiles `ui/main.rs` with the lint and compares the warnings with `ui/main.stderr`.

## License

MIT
