#![feature(rustc_private)]

extern crate rustc_ast;
extern crate rustc_data_structures;
extern crate rustc_errors;
extern crate rustc_span;

use std::path::{Path, PathBuf};

use rustc_ast::{Crate, Inline, Item, ItemKind, ModKind};
use rustc_data_structures::fx::{FxIndexMap, FxIndexSet};
use rustc_errors::{Diag, DiagDecorator};
use rustc_lint::{EarlyContext, EarlyLintPass, LintContext};
use rustc_span::{BytePos, FileName, Span};
use serde::Deserialize;

dylint_linting::impl_early_lint! {
    /**
     * ### What it does
     *
     * Flags a directory below the crate root's directory with more members than `max_members`,
     * 15 by default. Its members are the crate's module files in it and its directories that
     * hold module files.
     *
     * ### Why is this bad?
     *
     * A reader takes in a directory by listing it. Past fifteen names a listing is searched
     * rather than read, and its files want directories of their own, by what they are about.
     *
     * ### Configuration
     *
     * ```toml
     * [wide_directory]
     * max_members = 15
     * ```
     *
     * ### Known problems
     *
     * Each crate counts only its own files, so a directory shared by two crates, such as
     * `tests/`, is counted once per crate. The warning points at the crate root, put
     * `#![allow(wide_directory)]` there to accept a wide directory
     **/
    pub WIDE_DIRECTORY,
    Warn,
    "a source directory with more members than the limit",
    WideDirectory::new()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    max_members: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self { max_members: 15 }
    }
}

struct WideDirectory {
    max_members: usize,
    files: Vec<PathBuf>,
}

impl WideDirectory {
    fn new() -> Self {
        let config: Config = dylint_linting::config_or_default(env!("CARGO_PKG_NAME"));
        Self {
            max_members: config.max_members,
            files: Vec::new(),
        }
    }

    fn add(&mut self, cx: &EarlyContext<'_>, inner: Span) {
        let file = cx.sess().source_map().lookup_source_file(inner.lo());
        if let FileName::Real(real) = &file.name
            && let Some(path) = real.local_path()
        {
            self.files.push(path.to_path_buf());
        }
    }
}

impl EarlyLintPass for WideDirectory {
    fn check_item(&mut self, cx: &EarlyContext<'_>, item: &Item) {
        if let ItemKind::Mod(_, _, ModKind::Loaded(_, Inline::No { .. }, spans, ..)) = &item.kind
            && !item.span.from_expansion()
        {
            self.add(cx, spans.inner_span);
        }
    }

    fn check_crate_post(&mut self, cx: &EarlyContext<'_>, krate: &Crate) {
        self.add(cx, krate.spans.inner_span);
        let Some(root) = self
            .files
            .last()
            .and_then(|file| file.parent())
            .map(Path::to_path_buf)
        else {
            return;
        };

        let mut directories: FxIndexMap<PathBuf, FxIndexSet<String>> = FxIndexMap::default();
        for file in &self.files {
            let Ok(relative) = file.strip_prefix(&root) else {
                continue;
            };

            let mut directory = root.clone();
            for part in relative.components() {
                let name = part.as_os_str().to_string_lossy().into_owned();
                directories
                    .entry(directory.clone())
                    .or_default()
                    .insert(name);
                directory.push(part);
            }
        }

        let source = cx
            .sess()
            .source_map()
            .lookup_source_file(krate.spans.inner_span.lo());
        let start = Span::with_root_ctxt(source.start_pos, source.start_pos + BytePos(1));
        for (directory, members) in &directories {
            if members.len() <= self.max_members {
                continue;
            }

            let count = members.len();
            let max_members = self.max_members;
            let path = directory.display().to_string();
            let decorate = DiagDecorator(|diag: &mut Diag<'_, ()>| {
                diag.primary_message(format!(
                    "directory `{path}` has {count} members, more than {max_members}"
                ));
                diag.help("group its files into directories by what they are about");
            });
            cx.emit_span_lint(WIDE_DIRECTORY, start, decorate);
        }
    }
}

#[test]
fn ui() {
    dylint_testing::ui::Test::src_base(env!("CARGO_PKG_NAME"), "ui")
        .dylint_toml("wide_directory.max_members = 3")
        .run();
}
