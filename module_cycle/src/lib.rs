#![feature(rustc_private)]

extern crate rustc_data_structures;
extern crate rustc_errors;
extern crate rustc_hir;
extern crate rustc_middle;
extern crate rustc_span;

use rustc_data_structures::fx::{FxIndexMap, FxIndexSet};
use rustc_errors::{Diag, DiagDecorator};
use rustc_hir::def::{DefKind, Res};
use rustc_hir::def_id::{CRATE_DEF_ID, DefId, LocalDefId};
use rustc_hir::intravisit::{self, Visitor};
use rustc_hir::{BodyId, Expr, ExprKind, HirId, Path, QPath};
use rustc_lint::{LateContext, LateLintPass, LintContext};
use rustc_middle::hir::nested_filter;
use rustc_middle::ty::{TyCtxt, TypeckResults};
use rustc_span::Span;

dylint_linting::declare_late_lint! {
    /**
     * ### What it does
     *
     * Flags sibling modules that use each other in a circle. A module uses a sibling when code
     * written in it, or in a module inside it, names an item of the sibling or of a module inside
     * it. Paths, method calls and field accesses all count.
     *
     * ### Why is this bad?
     *
     * Modules in a circle have no order to read them in, and none of them can change alone.
     * Clippy has no lint for it, and Cargo only keeps crates in order.
     *
     * ### Known problems
     *
     * Test builds are not checked, so a use from `#[cfg(test)]` code makes no circle. Put
     * `#[allow(module_cycle)]` on the parent module to accept a circle among its children.
     *
     * ### Example
     *
     * ```rust
     * mod kinds {
     *     pub const LIMIT: usize = super::socket::BACKLOG;
     *     pub struct Ready;
     * }
     *
     * mod socket {
     *     pub const BACKLOG: usize = 8;
     *     pub fn listen() -> super::kinds::Ready {
     *         super::kinds::Ready
     *     }
     * }
     * ```
     *
     * Use instead: move `BACKLOG` into `kinds`, so only `socket` uses `kinds`
     **/
    pub MODULE_CYCLE,
    Warn,
    "sibling modules that use each other in a circle"
}

// How many ties a report lists, past that it counts the rest
const MOST_NOTES: usize = 7;

impl<'tcx> LateLintPass<'tcx> for ModuleCycle {
    fn check_crate_post(&mut self, cx: &LateContext<'tcx>) {
        if cx.sess().opts.test {
            return;
        }

        let mut uses = Uses {
            cx,
            typeck: None,
            ties: FxIndexMap::default(),
        };
        cx.tcx.hir_walk_toplevel_module(&mut uses);
        for (parent, ties) in &uses.ties {
            report(cx, *parent, ties);
        }
    }
}

#[derive(Default)]
struct Tie {
    items: FxIndexSet<DefId>,
    first: Option<Span>,
}

type Ties = FxIndexMap<(LocalDefId, LocalDefId), Tie>;

struct Uses<'a, 'tcx> {
    cx: &'a LateContext<'tcx>,
    typeck: Option<&'tcx TypeckResults<'tcx>>,
    // By parent module, the uses between two of its children
    ties: FxIndexMap<LocalDefId, Ties>,
}

impl<'tcx> Uses<'_, 'tcx> {
    fn record(&mut self, from: HirId, span: Span, item: DefId) {
        let tcx = self.cx.tcx;
        if span.in_external_macro(tcx.sess.source_map()) {
            return;
        }

        let Some(local) = item.as_local() else {
            return;
        };

        let item = match tcx.def_kind(local) {
            DefKind::Mod => return,
            DefKind::Ctor(..) => tcx.parent(item),
            _ => item,
        };
        let user = tcx.parent_module(from).to_local_def_id();
        let used = tcx.parent_module_from_def_id(local).to_local_def_id();
        let Some((parent, user, used)) = siblings(tcx, user, used) else {
            return;
        };

        let tie = self
            .ties
            .entry(parent)
            .or_default()
            .entry((user, used))
            .or_default();
        tie.items.insert(item);
        tie.first.get_or_insert(span);
    }
}

impl<'tcx> Visitor<'tcx> for Uses<'_, 'tcx> {
    type NestedFilter = nested_filter::All;

    fn maybe_tcx(&mut self) -> TyCtxt<'tcx> {
        self.cx.tcx
    }

    fn visit_nested_body(&mut self, id: BodyId) {
        let outer = self.typeck.replace(self.cx.tcx.typeck_body(id));
        self.visit_body(self.cx.tcx.hir_body(id));
        self.typeck = outer;
    }

    fn visit_path(&mut self, path: &Path<'tcx>, hir_id: HirId) {
        if let Res::Def(_, item) = path.res {
            self.record(hir_id, path.span, item);
        }

        intravisit::walk_path(self, path);
    }

    fn visit_expr(&mut self, expr: &'tcx Expr<'tcx>) {
        if let Some(typeck) = self.typeck {
            let item = match &expr.kind {
                ExprKind::MethodCall(..) => typeck.type_dependent_def_id(expr.hir_id),
                ExprKind::Path(qpath @ QPath::TypeRelative(..)) => {
                    typeck.qpath_res(qpath, expr.hir_id).opt_def_id()
                }
                ExprKind::Field(base, _) => typeck
                    .expr_ty_adjusted(base)
                    .peel_refs()
                    .ty_adt_def()
                    .map(|adt| adt.did()),
                _ => None,
            };
            if let Some(item) = item {
                self.record(expr.hir_id, expr.span, item);
            }
        }

        intravisit::walk_expr(self, expr);
    }
}

// From the crate root down to `module`
fn ancestors(tcx: TyCtxt<'_>, mut module: LocalDefId) -> Vec<LocalDefId> {
    let mut chain = vec![module];
    while module != CRATE_DEF_ID {
        module = tcx.parent_module_from_def_id(module).to_local_def_id();
        chain.push(module);
    }

    chain.reverse();
    chain
}

// The module that holds both, and its child that holds each, unless one holds the other
fn siblings(
    tcx: TyCtxt<'_>,
    a: LocalDefId,
    b: LocalDefId,
) -> Option<(LocalDefId, LocalDefId, LocalDefId)> {
    let a = ancestors(tcx, a);
    let b = ancestors(tcx, b);
    let shared = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    if shared == a.len() || shared == b.len() {
        return None;
    }

    Some((a[shared - 1], a[shared], b[shared]))
}

fn report(cx: &LateContext<'_>, parent: LocalDefId, ties: &Ties) {
    let mut children: FxIndexSet<LocalDefId> = FxIndexSet::default();
    for &(user, used) in ties.keys() {
        children.insert(user);
        children.insert(used);
    }

    let index = |child: &LocalDefId| children.get_index_of(child).expect("a tied child");
    let mut edges = vec![Vec::new(); children.len()];
    for (user, used) in ties.keys() {
        edges[index(user)].push(index(used));
    }

    for circle in circles(&edges) {
        let members: Vec<LocalDefId> = circle.iter().map(|&at| children[at]).collect();
        let mut inside: Vec<(&(LocalDefId, LocalDefId), &Tie)> = ties
            .iter()
            .filter(|((user, used), _)| members.contains(user) && members.contains(used))
            .collect();
        inside.sort_by_key(|(_, tie)| tie.items.len());
        emit(cx, parent, &members, &inside);
    }
}

fn emit(
    cx: &LateContext<'_>,
    parent: LocalDefId,
    members: &[LocalDefId],
    ties: &[(&(LocalDefId, LocalDefId), &Tie)],
) {
    let tcx = cx.tcx;
    let name = |module: LocalDefId| tcx.item_name(module.to_def_id()).to_string();
    let mut names: Vec<String> = members.iter().map(|&m| name(m)).collect();
    names.sort();
    let names = names.join("`, `");
    let place = if parent == CRATE_DEF_ID {
        "the crate root".to_owned()
    } else {
        format!("`{}`", tcx.def_path_str(parent.to_def_id()))
    };
    let span = ties[0].1.first.expect("a tie has a site");
    let decorate = DiagDecorator(|diag: &mut Diag<'_, ()>| {
        diag.primary_message(format!(
            "modules `{names}` in {place} use each other in a circle"
        ));
        for ((user, used), tie) in ties.iter().take(MOST_NOTES) {
            let items: Vec<String> = tie
                .items
                .iter()
                .map(|&item| format!("`{}`", item_path(tcx, item)))
                .collect();
            let note = format!(
                "`{}` uses `{}` by {}",
                name(*user),
                name(*used),
                items.join(", ")
            );
            diag.span_note(tie.first.expect("a tie has a site"), note);
        }

        if ties.len() > MOST_NOTES {
            let more = ties.len() - MOST_NOTES;
            diag.note(format!(
                "and {more} more {}",
                if more == 1 { "tie" } else { "ties" }
            ));
        }

        diag.help("move what they share into one of them, or into a module both can use");
    });
    let hir_id = tcx.local_def_id_to_hir_id(parent);
    tcx.emit_node_span_lint(MODULE_CYCLE, hir_id, span, decorate);
}

// Each set of two or more nodes that reach each other, the cheapest way for a few dozen modules
fn circles(edges: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let reach: Vec<Vec<bool>> = (0..edges.len())
        .map(|start| {
            let mut seen = vec![false; edges.len()];
            let mut stack = vec![start];
            while let Some(at) = stack.pop() {
                for &next in &edges[at] {
                    if !seen[next] {
                        seen[next] = true;
                        stack.push(next);
                    }
                }
            }

            seen
        })
        .collect();
    let mut placed = vec![false; edges.len()];
    let mut circles = Vec::new();
    for start in 0..edges.len() {
        if placed[start] || !reach[start][start] {
            continue;
        }

        let circle: Vec<usize> = (start..edges.len())
            .filter(|&other| reach[start][other] && reach[other][start])
            .collect();
        for &member in &circle {
            placed[member] = true;
        }

        if circle.len() > 1 {
            circles.push(circle);
        }
    }

    circles
}

// `kinds::Stream::new`, a method by its self type's path instead of its `impl` block's
fn item_path(tcx: TyCtxt<'_>, item: DefId) -> String {
    let parent = tcx.parent(item);
    if let DefKind::Impl { of_trait: false } = tcx.def_kind(parent)
        && let Some(adt) = tcx
            .type_of(parent)
            .instantiate_identity()
            .skip_normalization()
            .ty_adt_def()
    {
        return format!("{}::{}", tcx.def_path_str(adt.did()), tcx.item_name(item));
    }

    tcx.def_path_str(item)
}

#[test]
fn ui() {
    dylint_testing::ui_test(env!("CARGO_PKG_NAME"), "ui");
}
