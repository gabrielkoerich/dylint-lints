#![feature(rustc_private)]

extern crate rustc_ast;
extern crate rustc_errors;
extern crate rustc_span;

use rustc_ast::{Crate, Inline, Item, ItemKind, ModKind};
use rustc_errors::{Diag, DiagDecorator};
use rustc_lint::{EarlyContext, EarlyLintPass, LintContext};
use rustc_span::{BytePos, Span};
use serde::Deserialize;

dylint_linting::impl_early_lint! {
    /**
     * ### What it does
     *
     * Flags a source file with more lines than `max_lines`, 2000 by default. Every line counts,
     * blank and comment lines too.
     *
     * ### Why is this bad?
     *
     * A reader finds their way in a file by scrolling through it. Past a few thousand lines, a
     * file holds parts that want files of their own. Clippy's `too_many_lines` counts the lines
     * of one function, nothing counts the lines of a file.
     *
     * ### Configuration
     *
     * ```toml
     * [long_file]
     * max_lines = 2000
     * ```
     *
     * ### Known problems
     *
     * Only the crate root and out-of-line `mod` files count. A file read by `include!` does not
     **/
    pub LONG_FILE,
    Warn,
    "a source file with more lines than the limit",
    LongFile::new()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    max_lines: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self { max_lines: 2000 }
    }
}

struct LongFile {
    max_lines: usize,
}

impl LongFile {
    fn new() -> Self {
        let config: Config = dylint_linting::config_or_default(env!("CARGO_PKG_NAME"));
        Self {
            max_lines: config.max_lines,
        }
    }

    // `inner` is the body span of a crate root or of an out-of-line module
    fn check_file(&self, cx: &EarlyContext<'_>, inner: Span) {
        let file = cx.sess().source_map().lookup_source_file(inner.lo());
        let lines = file.count_lines();
        if lines <= self.max_lines {
            return;
        }

        let max_lines = self.max_lines;
        let start = Span::with_root_ctxt(file.start_pos, file.start_pos + BytePos(1));
        let decorate = DiagDecorator(|diag: &mut Diag<'_, ()>| {
            diag.primary_message(format!("file has {lines} lines, more than {max_lines}"));
            diag.help("split it into modules by what each part is about");
        });
        cx.emit_span_lint(LONG_FILE, start, decorate);
    }
}

impl EarlyLintPass for LongFile {
    fn check_crate(&mut self, cx: &EarlyContext<'_>, krate: &Crate) {
        self.check_file(cx, krate.spans.inner_span);
    }

    fn check_item(&mut self, cx: &EarlyContext<'_>, item: &Item) {
        if let ItemKind::Mod(_, _, ModKind::Loaded(_, Inline::No { .. }, spans, ..)) = &item.kind
            && !item.span.from_expansion()
        {
            self.check_file(cx, spans.inner_span);
        }
    }
}

#[test]
fn ui() {
    dylint_testing::ui::Test::src_base(env!("CARGO_PKG_NAME"), "ui")
        .dylint_toml("long_file.max_lines = 10")
        .run();
}
