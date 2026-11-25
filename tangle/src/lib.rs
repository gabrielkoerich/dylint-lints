#![feature(rustc_private)]

extern crate rustc_data_structures;
extern crate rustc_errors;
extern crate rustc_hir;
extern crate rustc_middle;
extern crate rustc_span;

use rustc_data_structures::fx::{FxIndexMap, FxIndexSet};
use rustc_errors::{Diag, DiagDecorator};
use rustc_hir::def::{DefKind, Res};
use rustc_hir::def_id::{DefId, LocalDefId};
use rustc_hir::intravisit::{self, Visitor};
use rustc_hir::{BodyId, Expr, ExprKind, HirId, Path, QPath};
use rustc_lint::{LateContext, LateLintPass, Level, LintContext};
use rustc_middle::hir::nested_filter;
use rustc_middle::ty::{TyCtxt, TypeckResults};
use rustc_span::Span;

dylint_linting::declare_late_lint! {
    /**
     * ### What it does
     *
     * Flags things that use each other in a circle, when the circle is written in more than one
     * module or has more than seven members. A thing is a type with its fields, variants,
     * methods and `impl` blocks wherever they are, a trait with its items, or a free function,
     * constant or static. Paths, method calls and field accesses all count as uses.
     *
     * ### Why is this bad?
     *
     * Things in a circle can only be learned and changed together. When the circle crosses
     * modules, no module holds what a reader needs, and past seven members it is too much to
     * hold in mind anywhere. `module_cycle` sees circles between sibling modules, this lint
     * also sees them between a module and its child.
     *
     * ### Known problems
     *
     * Test builds are not checked. A call through a function that only forwards to another
     * still counts as a use of the forwarder. Put `#[allow(tangle)]` on any member of the
     * circle, or on a module that holds one, to accept it.
     *
     * ### Example
     *
     * ```rust
     * mod kernel {
     *     pub struct Tasks;
     *     impl Tasks {
     *         pub fn wake(&self) {}
     *     }
     *
     *     pub mod timers {
     *         pub struct Timers;
     *         impl Timers {
     *             pub fn fire(&self, tasks: &super::Tasks) {
     *                 tasks.wake();
     *             }
     *         }
     *     }
     *
     *     impl Tasks {
     *         pub fn sleep(&self, timers: &timers::Timers) {
     *             timers.fire(self);
     *         }
     *     }
     * }
     * ```
     *
     * Use instead: a `tick` function in `kernel` that calls both, so `Timers` no longer knows
     * `Tasks`
     **/
    pub TANGLE,
    Warn,
    "things that use each other in a circle across modules"
}

// What a reader holds in mind at once, a circle of more members is too large in one module too
const HELD_IN_MIND: usize = 7;

impl<'tcx> LateLintPass<'tcx> for Tangle {
    fn check_crate_post(&mut self, cx: &LateContext<'tcx>) {
        if cx.sess().opts.test {
            return;
        }

        let mut uses = Uses {
            cx,
            typeck: None,
            things: FxIndexSet::default(),
            ties: FxIndexMap::default(),
        };
        cx.tcx.hir_walk_toplevel_module(&mut uses);
        let mut edges = vec![FxIndexSet::default(); uses.things.len()];
        for &(user, used) in uses.ties.keys() {
            edges[user].insert(used);
        }

        for members in circles(&edges) {
            report(cx, &uses, &members);
        }
    }
}

struct Tie {
    items: FxIndexSet<DefId>,
    first: (Span, HirId),
    modules: FxIndexSet<LocalDefId>,
}

struct Uses<'a, 'tcx> {
    cx: &'a LateContext<'tcx>,
    typeck: Option<&'tcx TypeckResults<'tcx>>,
    things: FxIndexSet<LocalDefId>,
    // By user and used thing, as indexes in `things`
    ties: FxIndexMap<(usize, usize), Tie>,
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

        let (Some(user), Some(used)) = (thing(tcx, from.owner.def_id), thing(tcx, local)) else {
            return;
        };

        if user == used {
            return;
        }

        let item = match tcx.def_kind(local) {
            DefKind::Ctor(..) => tcx.parent(item),
            _ => item,
        };
        let user = self.things.insert_full(user).0;
        let used = self.things.insert_full(used).0;
        let tie = self.ties.entry((user, used)).or_insert_with(|| Tie {
            items: FxIndexSet::default(),
            first: (span, from),
            modules: FxIndexSet::default(),
        });
        tie.items.insert(item);
        tie.modules
            .insert(tcx.parent_module(from).to_local_def_id());
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

// The thing `def` is part of, an `impl` block's items are its self type's
fn thing(tcx: TyCtxt<'_>, mut def: LocalDefId) -> Option<LocalDefId> {
    loop {
        match tcx.def_kind(def) {
            DefKind::Struct
            | DefKind::Enum
            | DefKind::Union
            | DefKind::Trait
            | DefKind::TraitAlias
            | DefKind::TyAlias
            | DefKind::Fn
            | DefKind::Const { .. }
            | DefKind::Static { .. } => return Some(def),
            DefKind::Impl { .. } => {
                let self_ty = tcx.type_of(def).instantiate_identity().skip_normalization();
                return self_ty.ty_adt_def()?.did().as_local();
            }
            DefKind::Mod | DefKind::ForeignMod => return None,
            _ => def = tcx.opt_local_parent(def)?,
        }
    }
}

fn report(cx: &LateContext<'_>, uses: &Uses<'_, '_>, members: &[usize]) {
    let tcx = cx.tcx;
    let mut ties: Vec<(&(usize, usize), &Tie)> = uses
        .ties
        .iter()
        .filter(|((user, used), _)| members.contains(user) && members.contains(used))
        .collect();
    let mut modules: FxIndexSet<LocalDefId> = members
        .iter()
        .map(|&member| {
            tcx.parent_module_from_def_id(uses.things[member])
                .to_local_def_id()
        })
        .collect();
    for (_, tie) in &ties {
        modules.extend(tie.modules.iter().copied());
    }

    if modules.len() < 2 && members.len() <= HELD_IN_MIND {
        return;
    }

    ties.sort_by_key(|(_, tie)| tie.items.len());
    let name = |member: usize| tcx.def_path_str(uses.things[member].to_def_id());
    let mut names: Vec<String> = members.iter().map(|&member| name(member)).collect();
    names.sort();
    let mut places: Vec<String> = modules
        .iter()
        .map(|&module| match tcx.def_path_str(module.to_def_id()) {
            path if path.is_empty() => "crate".to_owned(),
            path => path,
        })
        .collect();
    places.sort();
    let message = if names.len() <= HELD_IN_MIND {
        format!("`{}` use each other in a circle", names.join("`, `"))
    } else {
        format!("{} things use each other in a circle", names.len())
    };
    let (span, mut hir_id) = ties[0].1.first;
    // The lint takes the level of the node it is emitted at, so one member's allow or expect covers the circle
    let marked = members
        .iter()
        .map(|&member| tcx.local_def_id_to_hir_id(uses.things[member]))
        .find(|&member| {
            matches!(
                tcx.lint_level_spec_at_node(TANGLE, member).level(),
                Level::Allow | Level::Expect
            )
        });
    if let Some(member) = marked {
        hir_id = member;
    }

    let decorate = DiagDecorator(|diag: &mut Diag<'_, ()>| {
        diag.primary_message(message);
        if places.len() > 1 {
            diag.note(format!("written in `{}`", places.join("`, `")));
        }

        for ((user, used), tie) in ties.iter().take(HELD_IN_MIND) {
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
            diag.span_note(tie.first.0, note);
        }

        if ties.len() > HELD_IN_MIND {
            let more = ties.len() - HELD_IN_MIND;
            diag.note(format!(
                "and {more} more {}",
                if more == 1 { "tie" } else { "ties" }
            ));
        }

        diag.help("give the circle one owner that calls the others, so no member knows its caller");
    });
    tcx.emit_node_span_lint(TANGLE, hir_id, span, decorate);
}

// Tarjan's strongly connected components with two or more members, without recursion
fn circles(edges: &[FxIndexSet<usize>]) -> Vec<Vec<usize>> {
    const UNSEEN: usize = usize::MAX;
    let mut order = vec![UNSEEN; edges.len()];
    let mut low = vec![0; edges.len()];
    let mut on_stack = vec![false; edges.len()];
    let mut stack = Vec::new();
    let mut next = 0;
    let mut circles = Vec::new();
    for root in 0..edges.len() {
        if order[root] != UNSEEN {
            continue;
        }

        let mut work = vec![(root, 0)];
        while let Some(&(at, edge)) = work.last() {
            if edge == 0 && order[at] == UNSEEN {
                order[at] = next;
                low[at] = next;
                next += 1;
                stack.push(at);
                on_stack[at] = true;
            }

            if let Some(&to) = edges[at].get_index(edge) {
                work.last_mut().expect("a frame").1 += 1;
                if order[to] == UNSEEN {
                    work.push((to, 0));
                } else if on_stack[to] {
                    low[at] = low[at].min(order[to]);
                }

                continue;
            }

            work.pop();
            if let Some(&(parent, _)) = work.last() {
                low[parent] = low[parent].min(low[at]);
            }

            if low[at] == order[at] {
                let mut members = Vec::new();
                while let Some(member) = stack.pop() {
                    on_stack[member] = false;
                    members.push(member);
                    if member == at {
                        break;
                    }
                }

                if members.len() > 1 {
                    members.sort_unstable();
                    circles.push(members);
                }
            }
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
