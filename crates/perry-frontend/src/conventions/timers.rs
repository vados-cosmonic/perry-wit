//! The callback-timer sleep idiom.
//!
//! The compiler has the promise-returning `setTimeout` of `node:timers/promises`, but no
//! callback timers and no `Promise` constructor. Portable sources sleep with both:
//!
//! ```js
//! await new Promise((resolve) => setTimeout(resolve, 1000));
//! ```
//!
//! This module rewrites exactly that expression into a call of the promise-returning timer.
use super::ident;
use perry_parser::swc_ecma_ast as ast;
use swc_common::{DUMMY_SP, SyntaxContext};
use swc_ecma_visit::{VisitMut, VisitMutWith};

/// The local name of the promise-returning timer.
const SLEEP: &str = "__jco_sleep";

fn unparenthesized(mut expression: &ast::Expr) -> &ast::Expr {
    while let ast::Expr::Paren(paren) = expression {
        expression = &paren.expr;
    }
    expression
}

/// The single parameter and single expression of a function: `(a) => e`, `(a) => { e; }`,
/// `function (a) { return e; }`. `None` for the parameter of a function without one.
fn simple_function(expression: &ast::Expr) -> Option<(Option<&ast::Ident>, &ast::Expr)> {
    fn single(statements: &[ast::Stmt]) -> Option<&ast::Expr> {
        match statements {
            [ast::Stmt::Expr(statement)] => Some(statement.expr.as_ref()),
            [
                ast::Stmt::Return(ast::ReturnStmt {
                    arg: Some(value), ..
                }),
            ] => Some(value.as_ref()),
            _ => None,
        }
    }
    fn parameter(patterns: Vec<&ast::Pat>) -> Option<Option<&ast::Ident>> {
        match patterns.as_slice() {
            [] => Some(None),
            [ast::Pat::Ident(binding)] => Some(Some(&binding.id)),
            _ => None,
        }
    }
    match unparenthesized(expression) {
        ast::Expr::Arrow(arrow) if !arrow.is_async && !arrow.is_generator => {
            let body = match arrow.body.as_ref() {
                ast::ArrowFunctionBody::Expr(value) => value.as_ref(),
                ast::ArrowFunctionBody::FunctionBody(block) => single(&block.stmts)?,
            };
            Some((parameter(arrow.params.iter().collect())?, body))
        }
        ast::Expr::Fn(function)
            if !function.function.is_async && !function.function.is_generator =>
        {
            let body = single(&function.function.body.as_ref()?.stmts)?;
            let params = function.function.params.iter().map(|param| &param.pat);
            Some((parameter(params.collect())?, body))
        }
        _ => None,
    }
}

/// The delay of `new Promise((resolve) => setTimeout(resolve, delay))`, where the timer's
/// callback may also be a function that only calls `resolve()`.
fn sleep(expression: &ast::Expr) -> Option<Option<&ast::Expr>> {
    let ast::Expr::New(new) = expression else {
        return None;
    };
    if !matches!(new.callee.as_ref(), ast::Expr::Ident(class) if class.sym == "Promise") {
        return None;
    }
    let [executor] = new.args.as_deref()? else {
        return None;
    };
    let (Some(resolve), body) = simple_function(&executor.expr)? else {
        return None;
    };
    let ast::Expr::Call(timer) = unparenthesized(body) else {
        return None;
    };
    let ast::Callee::Expr(callee) = &timer.callee else {
        return None;
    };
    if !matches!(callee.as_ref(), ast::Expr::Ident(name) if name.sym == "setTimeout")
        || timer.args.is_empty()
        || timer.args.len() > 2
        || executor.spread.is_some()
        || timer.args.iter().any(|argument| argument.spread.is_some())
    {
        return None;
    }
    let resolves = |expression: &ast::Expr| matches!(unparenthesized(expression), ast::Expr::Ident(name) if name.sym == resolve.sym);
    let callback = &timer.args[0].expr;
    let direct = resolves(callback);
    let indirect = simple_function(callback).is_some_and(|(parameter, body)| {
        parameter.is_none()
            && matches!(
                unparenthesized(body),
                ast::Expr::Call(call)
                    if call.args.is_empty()
                        && matches!(&call.callee, ast::Callee::Expr(callee) if resolves(callee))
            )
    });
    (direct || indirect).then(|| timer.args.get(1).map(|delay| delay.expr.as_ref()))
}

struct Sleeps {
    found: bool,
}
impl VisitMut for Sleeps {
    fn visit_mut_expr(&mut self, expression: &mut ast::Expr) {
        expression.visit_mut_children_with(self);
        let Some(delay) = sleep(expression) else {
            return;
        };
        let span = perry_parser::Spanned::span(&*expression);
        let delay = delay.cloned().unwrap_or_else(|| {
            ast::Expr::Lit(ast::Lit::Num(ast::Number {
                span,
                value: 0.0,
                raw: None,
            }))
        });
        *expression = ast::Expr::Call(ast::CallExpr {
            span,
            ctxt: SyntaxContext::empty(),
            callee: ast::Callee::Expr(Box::new(ast::Expr::Ident(ident(SLEEP, span)))),
            args: vec![ast::ExprOrSpread {
                spread: None,
                expr: Box::new(delay),
            }],
            type_args: None,
        });
        self.found = true;
    }
}

/// Rewrite every sleep idiom of the module, importing the timer it calls.
pub(super) fn bind(module: &mut ast::Module) {
    let mut sleeps = Sleeps { found: false };
    module.visit_mut_with(&mut sleeps);
    if !sleeps.found {
        return;
    }
    module.body.insert(
        0,
        ast::ModuleItem::ModuleDecl(ast::ModuleDecl::Import(ast::ImportDecl {
            span: DUMMY_SP,
            specifiers: vec![ast::ImportSpecifier::Named(ast::ImportNamedSpecifier {
                span: DUMMY_SP,
                local: ident(SLEEP, DUMMY_SP),
                imported: Some(ast::ModuleExportName::Ident(ident("setTimeout", DUMMY_SP))),
                is_type_only: false,
            })],
            src: Box::new(ast::Str {
                span: DUMMY_SP,
                value: "node:timers/promises".into(),
                raw: None,
            }),
            type_only: false,
            with: None,
            phase: ast::ImportPhase::Evaluation,
        })),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The delays the module sleeps for after the rewrite, and whether the timer is imported.
    fn rewritten(text: &str) -> anyhow::Result<(usize, bool)> {
        let mut module = crate::hir_compat::parse(text, "main.ts")?;
        bind(&mut module);
        struct Calls(usize);
        impl VisitMut for Calls {
            fn visit_mut_call_expr(&mut self, call: &mut ast::CallExpr) {
                call.visit_mut_children_with(self);
                if matches!(&call.callee, ast::Callee::Expr(callee) if matches!(callee.as_ref(), ast::Expr::Ident(name) if name.sym == SLEEP))
                    && call.args.len() == 1
                {
                    self.0 += 1;
                }
            }
        }
        let mut calls = Calls(0);
        module.visit_mut_with(&mut calls);
        let imported = matches!(
            module.body.first(),
            Some(ast::ModuleItem::ModuleDecl(ast::ModuleDecl::Import(import)))
                if import.src.value.as_str() == Some("node:timers/promises")
        );
        Ok((calls.0, imported))
    }

    #[test]
    fn a_promise_around_a_callback_timer_becomes_a_promise_timer() -> anyhow::Result<()> {
        for text in [
            "export async function run() { await new Promise((resolve) => setTimeout(resolve, 1000)); }",
            "export async function run() { await new Promise<void>((done) => { setTimeout(done, 5); }); }",
            "export async function run() { await new Promise(function (resolve) { setTimeout(() => resolve(), 5); }); }",
            "export async function run() { await new Promise((resolve) => setTimeout(resolve)); }",
        ] {
            assert_eq!(rewritten(text)?, (1, true), "{text}");
        }
        Ok(())
    }

    #[test]
    fn other_promises_are_left_alone() -> anyhow::Result<()> {
        for text in [
            "export function run() { return new Promise((resolve) => resolve(1)); }",
            "export function run() { return new Promise((resolve, reject) => setTimeout(resolve, 5)); }",
            "export function run() { return new Promise((resolve) => setTimeout(() => resolve(1), 5)); }",
            "export function run() { return new Promise((resolve) => setTimeout(other, 5)); }",
            "export function run() { return new Promise((resolve) => setTimeout(resolve, 5, 1)); }",
        ] {
            assert_eq!(rewritten(text)?, (0, false), "{text}");
        }
        Ok(())
    }
}
