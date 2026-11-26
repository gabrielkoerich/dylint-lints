#![feature(rustc_private)]

extern crate rustc_abi;
extern crate rustc_data_structures;
extern crate rustc_errors;
extern crate rustc_hir;
extern crate rustc_middle;
extern crate rustc_span;

use rustc_abi::FieldIdx;
use rustc_data_structures::fx::{FxIndexMap, FxIndexSet};
use rustc_errors::{Diag, DiagDecorator};
use rustc_hir::def::DefKind;
use rustc_hir::def_id::LocalDefId;
use rustc_hir::intravisit::{self, Visitor};
use rustc_hir::{BodyId, Expr, ExprKind, HirId, Mutability, Node, QPath, StmtKind};
use rustc_lint::{LateContext, LateLintPass, LintContext};
use rustc_middle::hir::nested_filter;
use rustc_middle::ty::{AdtDef, Ty, TyCtxt, TypeckResults};
use rustc_span::{Span, kw};

dylint_linting::declare_late_lint! {
    /**
     * ### What it does
     *
     * Flags code outside a struct that works on one of its fields: writes it, borrows it
     * mutably, indexes it, calls a method on it, moves it out, or sets it in a struct literal.
     * Copying a field or taking a shared borrow of it is a read, and reads are fine.
     *
     * Only a struct that guards its fields is checked. It guards them when it keeps at least one
     * field to itself and has a method with a `self` parameter, other than a getter or a setter,
     * that uses them. A `PhantomData` or `()` field plays no part. Code is inside the struct when
     * it is in one of its `impl` blocks, wherever that block is. Visibility keywords play no part.
     *
     * ### Why is this bad?
     *
     * The struct already has methods that work on its fields, and code elsewhere works on them
     * too. Each such place depends on how the struct keeps its data, so the struct cannot change
     * that alone. Clippy's `partial_pub_fields` only looks at the `pub` keyword.
     *
     * ### Known problems
     *
     * Test builds are not checked, and neither are patterns that destructure the struct. A use
     * through a getter that hands out a reference is not traced. Things that use each other in a
     * circle with the struct count as outside it. Put `#[allow(field_leak)]` on the struct or the
     * field to accept a leak.
     *
     * ### Example
     *
     * ```rust
     * pub struct RunQueue {
     *     pub items: Vec<u32>,
     *     limit: usize,
     * }
     *
     * impl RunQueue {
     *     pub fn full(&self) -> bool {
     *         self.items.len() >= self.limit
     *     }
     * }
     *
     * pub fn admit(queue: &mut RunQueue, task: u32) {
     *     queue.items.push(task);
     * }
     * ```
     *
     * Use instead: a `push` method on `RunQueue`, so only `RunQueue` works on `items`
     **/
    pub FIELD_LEAK,
    Warn,
    "code outside a struct that works on its fields"
}

// How many places a report lists, past that it counts the rest
const MOST_NOTES: usize = 7;

impl<'tcx> LateLintPass<'tcx> for FieldLeak {
    fn check_crate_post(&mut self, cx: &LateContext<'tcx>) {
        if cx.sess().opts.test {
            return;
        }

        let mut uses = Uses {
            cx,
            typeck: None,
            fields: FxIndexMap::default(),
        };
        cx.tcx.hir_walk_toplevel_module(&mut uses);
        let mut structs: FxIndexMap<LocalDefId, Vec<(FieldIdx, &Vec<Use>)>> = FxIndexMap::default();
        for ((owner, field), list) in &uses.fields {
            structs.entry(*owner).or_default().push((*field, list));
        }

        for (owner, used) in structs {
            report(cx, owner, &used);
        }
    }
}

struct Use {
    span: Span,
    from: HirId,
    inside: bool,
    works: bool,
}

struct Uses<'a, 'tcx> {
    cx: &'a LateContext<'tcx>,
    typeck: Option<&'tcx TypeckResults<'tcx>>,
    fields: FxIndexMap<(LocalDefId, FieldIdx), Vec<Use>>,
}

impl<'tcx> Uses<'_, 'tcx> {
    fn record(&mut self, adt: AdtDef<'tcx>, field: FieldIdx, from: HirId, span: Span, works: bool) {
        let tcx = self.cx.tcx;
        if !adt.is_struct() || span.in_external_macro(tcx.sess.source_map()) {
            return;
        }

        let Some(owner) = adt.did().as_local() else {
            return;
        };

        let inside = thing(tcx, from.owner.def_id) == Some(owner);
        let works = if inside {
            is_working_method(tcx, from.owner.def_id)
        } else {
            works
        };
        let found = Use {
            span,
            from,
            inside,
            works,
        };
        self.fields.entry((owner, field)).or_default().push(found);
    }

    fn is_copy(&self, ty: Ty<'tcx>) -> bool {
        self.cx.type_is_copy_modulo_regions(ty)
    }

    // Whether the field access `expr` works on the field, by what its parent does with it
    fn works_on(&self, expr: &Expr<'tcx>, ty: Ty<'tcx>) -> bool {
        let Node::Expr(parent) = self.cx.tcx.parent_hir_node(expr.hir_id) else {
            return !self.is_copy(ty);
        };

        match parent.kind {
            ExprKind::Assign(lhs, ..) | ExprKind::AssignOp(_, lhs, _) => lhs.hir_id == expr.hir_id,
            ExprKind::AddrOf(_, mutability, _) => mutability == Mutability::Mut,
            ExprKind::MethodCall(_, receiver, ..) if receiver.hir_id == expr.hir_id => true,
            ExprKind::Index(base, ..) => base.hir_id == expr.hir_id,
            ExprKind::Field(..) => true,
            ExprKind::Binary(..) | ExprKind::Unary(..) => false,
            _ => !self.is_copy(ty),
        }
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

    fn visit_expr(&mut self, expr: &'tcx Expr<'tcx>) {
        if let Some(typeck) = self.typeck {
            match &expr.kind {
                ExprKind::Field(base, _) => {
                    let adt = typeck.expr_ty_adjusted(base).peel_refs().ty_adt_def();
                    if let (Some(adt), Some(field)) = (adt, typeck.opt_field_index(expr.hir_id)) {
                        let works = self.works_on(expr, typeck.expr_ty(expr));
                        self.record(adt, field, expr.hir_id, expr.span, works);
                    }
                }
                ExprKind::Struct(_, fields, _) => {
                    if let Some(adt) = typeck.expr_ty(expr).ty_adt_def() {
                        for field in *fields {
                            if let Some(index) = typeck.opt_field_index(field.hir_id) {
                                self.record(adt, index, field.hir_id, field.ident.span, true);
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        intravisit::walk_expr(self, expr);
    }
}

// The type an item's code belongs to, an `impl` block's items are its self type's
fn thing(tcx: TyCtxt<'_>, mut def: LocalDefId) -> Option<LocalDefId> {
    loop {
        match tcx.def_kind(def) {
            DefKind::Struct | DefKind::Enum | DefKind::Union => return Some(def),
            DefKind::Impl { .. } => {
                let self_ty = tcx.type_of(def).instantiate_identity().skip_normalization();
                return self_ty.ty_adt_def()?.did().as_local();
            }
            DefKind::Mod | DefKind::ForeignMod | DefKind::Fn => return None,
            _ => def = tcx.opt_local_parent(def)?,
        }
    }
}

// A method with a `self` parameter whose body is more than a getter or a setter
fn is_working_method(tcx: TyCtxt<'_>, def: LocalDefId) -> bool {
    if tcx.def_kind(def) != DefKind::AssocFn || !tcx.associated_item(def).is_method() {
        return false;
    }

    let mut body = tcx.hir_body_owned_by(def).value;
    loop {
        body = match body.kind {
            ExprKind::Block(block, _) => match (block.stmts, block.expr) {
                ([], Some(tail)) => tail,
                ([stmt], None) => match stmt.kind {
                    StmtKind::Semi(expr) | StmtKind::Expr(expr) => expr,
                    _ => return true,
                },
                _ => return true,
            },
            ExprKind::AddrOf(_, _, inner) => inner,
            ExprKind::MethodCall(_, receiver, [], _) => receiver,
            ExprKind::Assign(lhs, ..) => lhs,
            ExprKind::Field(base, _) => return !is_self(base),
            _ => return true,
        };
    }
}

fn is_self(expr: &Expr<'_>) -> bool {
    matches!(expr.kind, ExprKind::Path(QPath::Resolved(None, path))
        if path.segments.len() == 1 && path.segments[0].ident.name == kw::SelfLower)
}

fn report(cx: &LateContext<'_>, owner: LocalDefId, used: &[(FieldIdx, &Vec<Use>)]) {
    let tcx = cx.tcx;
    let adt = tcx.adt_def(owner);
    let fields: Vec<FieldIdx> = adt
        .non_enum_variant()
        .fields
        .iter_enumerated()
        .filter(|(_, field)| {
            let ty = tcx
                .type_of(field.did)
                .instantiate_identity()
                .skip_normalization();
            !ty.is_unit() && !ty.ty_adt_def().is_some_and(|adt| adt.is_phantom_data())
        })
        .map(|(index, _)| index)
        .collect();
    let uses_of = |field: FieldIdx| {
        used.iter()
            .find(|(index, _)| *index == field)
            .map_or(&[][..], |(_, list)| list.as_slice())
    };
    let known_outside = |field: FieldIdx| uses_of(field).iter().any(|found| !found.inside);
    let kept: Vec<FieldIdx> = fields
        .iter()
        .copied()
        .filter(|&f| !known_outside(f))
        .collect();
    let working = fields.iter().any(|&field| {
        uses_of(field)
            .iter()
            .any(|found| found.inside && found.works)
    });
    if kept.is_empty() || !working {
        return;
    }

    let name = |field: FieldIdx| adt.non_enum_variant().fields[field].name.to_string();
    let owner_name = tcx.item_name(owner.to_def_id());
    for &field in &fields {
        let leaks: Vec<&Use> = uses_of(field)
            .iter()
            .filter(|found| !found.inside && found.works)
            .collect();
        if leaks.is_empty() {
            continue;
        }

        let modules: FxIndexSet<LocalDefId> = leaks
            .iter()
            .map(|found| tcx.parent_module(found.from).to_local_def_id())
            .collect();
        let others: Vec<String> = fields
            .iter()
            .filter(|&&other| other != field && known_outside(other))
            .map(|&other| name(other))
            .collect();
        let kept_names: Vec<String> = kept.iter().map(|&other| name(other)).collect();
        let places = count(leaks.len(), "place");
        let in_modules = count(modules.len(), "module");
        let message = format!(
            "`{owner_name}::{}` is worked on outside `{owner_name}`: {places} in {in_modules}",
            name(field)
        );
        let decorate = DiagDecorator(|diag: &mut Diag<'_, ()>| {
            diag.primary_message(message);
            for found in leaks.iter().skip(1).take(MOST_NOTES) {
                diag.span_note(found.span, "also here");
            }

            if leaks.len() > MOST_NOTES + 1 {
                diag.note(format!(
                    "and {}",
                    count(leaks.len() - MOST_NOTES - 1, "more place")
                ));
            }

            if !others.is_empty() {
                diag.note(format!("also known outside: `{}`", others.join("`, `")));
            }

            diag.note(format!("kept: `{}`", kept_names.join("`, `")));
            diag.help(format!("give `{owner_name}` a method that does this work"));
        });
        let def = adt.non_enum_variant().fields[field].did.expect_local();
        tcx.emit_node_span_lint(
            FIELD_LEAK,
            tcx.local_def_id_to_hir_id(def),
            leaks[0].span,
            decorate,
        );
    }
}

// `1 place`, `2 places`
fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

#[test]
fn ui() {
    dylint_testing::ui_test(env!("CARGO_PKG_NAME"), "ui");
}
