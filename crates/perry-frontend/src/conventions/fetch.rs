//! Fetch-event HTTP handlers.
//!
//! A component exporting `wasi:http/incoming-handler@0.2` implements `handle` over the
//! request and response resources of `wasi:http/types`. Components written for
//! `jco componentize` may register a service-worker listener instead:
//!
//! ```js
//! addEventListener('fetch', (event) => event.respondWith(new Response('Hello')));
//! ```
//!
//! This module turns the listener into an asynchronous function that returns the response,
//! and appends an `incomingHandler` export object, written in TypeScript against the
//! resource classes, that builds the `Request`, calls the listener and sends its `Response`.
//! The rest of the pass then binds that object like one the source had written.
use super::{ClearSpans, Source, World, annotations, find_object, function_of, ident};
use anyhow::{Context as _, Result};
use perry_parser::swc_ecma_ast as ast;
use swc_common::Span;
use swc_ecma_visit::VisitMutWith;

/// The export object of `wasi:http/incoming-handler`, and the module declaring its resources.
const OBJECT: &str = "incomingHandler";
const TYPES: &str = "wasi:http/types";

/// The generated handler. It names the resource module without its version, and the
/// listener as `__jco_fetch_listener`.
const SHIM: &str = include_str!("fetch.ts");
const LISTENER: &str = "__jco_fetch_listener";

/// The listener of a top-level `addEventListener('fetch', listener)` call.
fn registration(item: &ast::ModuleItem) -> Option<&ast::Expr> {
    let ast::ModuleItem::Stmt(ast::Stmt::Expr(statement)) = item else {
        return None;
    };
    let ast::Expr::Call(call) = statement.expr.as_ref() else {
        return None;
    };
    let ast::Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let named = match callee.as_ref() {
        ast::Expr::Ident(name) => name.sym == "addEventListener",
        ast::Expr::Member(member) => {
            matches!(member.obj.as_ref(), ast::Expr::Ident(object) if object.sym == "self" || object.sym == "globalThis")
                && matches!(&member.prop, ast::MemberProp::Ident(name) if name.sym == "addEventListener")
        }
        _ => false,
    };
    let [event, listener] = call.args.as_slice() else {
        return None;
    };
    let fetch = matches!(event.expr.as_ref(), ast::Expr::Lit(ast::Lit::Str(name)) if name.value.as_str() == Some("fetch"));
    (named && fetch && event.spread.is_none() && listener.spread.is_none())
        .then_some(listener.expr.as_ref())
}

/// The argument of `event.respondWith(argument)`.
fn responded(expression: &mut ast::Expr, event: Option<&str>) -> Option<Box<ast::Expr>> {
    let ast::Expr::Call(call) = expression else {
        return None;
    };
    let ast::Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let ast::Expr::Member(member) = callee.as_ref() else {
        return None;
    };
    let on_event = matches!(member.obj.as_ref(), ast::Expr::Ident(object) if Some(object.sym.as_ref()) == event);
    let responds =
        matches!(&member.prop, ast::MemberProp::Ident(name) if name.sym == "respondWith");
    if !on_event || !responds || call.args.len() != 1 || call.args[0].spread.is_some() {
        return None;
    }
    Some(call.args.remove(0).expr)
}

fn unparenthesized(mut expression: ast::Expr) -> ast::Expr {
    while let ast::Expr::Paren(paren) = expression {
        expression = *paren.expr;
    }
    expression
}

/// The statements producing a response: those of an immediately called function without
/// parameters, as in `respondWith((async () => { ... })())`, or the returned expression.
fn response(expression: ast::Expr, span: Span) -> Vec<ast::Stmt> {
    let returned = |expression: ast::Expr| {
        vec![ast::Stmt::Return(ast::ReturnStmt {
            span,
            arg: Some(Box::new(expression)),
        })]
    };
    let ast::Expr::Call(call) = expression else {
        return returned(expression);
    };
    let immediate = call.args.is_empty()
        && matches!(&call.callee, ast::Callee::Expr(callee) if {
            let mut callee = callee.as_ref();
            while let ast::Expr::Paren(paren) = callee {
                callee = &paren.expr;
            }
            match callee {
                ast::Expr::Arrow(arrow) => arrow.params.is_empty() && !arrow.is_generator,
                ast::Expr::Fn(function) => {
                    function.function.params.is_empty() && !function.function.is_generator
                }
                _ => false,
            }
        });
    if !immediate {
        return returned(ast::Expr::Call(call));
    }
    let ast::Callee::Expr(callee) = call.callee else {
        unreachable!("matched an expression callee above");
    };
    match function_of(unparenthesized(*callee)) {
        Ok(function) => function.body.map(|body| body.stmts).unwrap_or_default(),
        Err(_) => unreachable!("matched a function above"),
    }
}

/// Make a listener an asynchronous function that returns what it responds with.
fn respond(source: &Source<'_>, function: &mut ast::Function, span: Span) -> Result<()> {
    let unsupported = |span: Span| {
        source.error(
            span,
            "A fetch listener needs to end by calling event.respondWith(response)".into(),
        )
    };
    if function.params.len() > 1 || function.is_generator {
        return Err(unsupported(span));
    }
    let event = match function.params.first().map(|param| &param.pat) {
        None => None,
        Some(ast::Pat::Ident(binding)) => Some(binding.id.sym.to_string()),
        Some(_) => return Err(unsupported(span)),
    };
    let body = function.body.as_mut().ok_or_else(|| unsupported(span))?;
    let argument = match body.stmts.last_mut() {
        Some(ast::Stmt::Expr(statement)) => responded(&mut statement.expr, event.as_deref()),
        Some(ast::Stmt::Return(ast::ReturnStmt {
            arg: Some(value), ..
        })) => responded(value, event.as_deref()),
        _ => None,
    };
    let argument = argument.ok_or_else(|| unsupported(span))?;
    body.stmts.pop();
    body.stmts
        .extend(response(unparenthesized(*argument), span));

    let mut types = annotations(&["__jco_FetchEvent".into(), "Promise<Response>".into()])
        .context("typing a fetch listener")?;
    function.return_type = types.pop().flatten();
    if let Some(ast::Pat::Ident(binding)) = function.params.first_mut().map(|param| &mut param.pat)
        && binding.type_ann.is_none()
    {
        binding.type_ann = types.pop().flatten();
    }
    function.is_async = true;
    Ok(())
}

/// Implement `wasi:http/incoming-handler` from a `fetch` listener, when the world exports
/// it and the source does not. Returns the text of the generated handler.
pub(super) fn bind(
    module: &mut ast::Module,
    source: &Source<'_>,
    world: &World,
) -> Result<Option<&'static str>> {
    let exported = world
        .exports
        .iter()
        .any(|export| export.object.as_deref() == Some(OBJECT) && export.member == "handle");
    let types = world
        .resources
        .iter()
        .map(|resource| resource.module.as_str())
        .find(|module| {
            module
                .strip_prefix(TYPES)
                .is_some_and(|version| version.starts_with("@0.2"))
        });
    let (true, Some(types)) = (exported, types) else {
        return Ok(None);
    };
    if find_object(module, OBJECT).is_some() {
        return Ok(None);
    }
    let Some(position) = module
        .body
        .iter()
        .position(|item| registration(item).is_some())
    else {
        return Ok(None);
    };
    let span = perry_parser::Spanned::span(&module.body[position]);
    let ast::ModuleItem::Stmt(ast::Stmt::Expr(statement)) = module.body.remove(position) else {
        unreachable!("matched a registration above");
    };
    let ast::Expr::Call(mut call) = *statement.expr else {
        unreachable!("matched a registration above");
    };
    let listener = unparenthesized(*call.args.remove(1).expr);
    let name = match listener {
        // A listener declared elsewhere responds in place.
        ast::Expr::Ident(name) => {
            let declaration = module.body.iter_mut().find_map(|item| match item {
                ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Fn(function)))
                    if function.ident.sym == name.sym =>
                {
                    Some(function)
                }
                _ => None,
            });
            let Some(declaration) = declaration else {
                return Err(source.error(
                    name.span,
                    format!(
                        "The fetch listener '{}' needs to be a function declared in the entry module",
                        name.sym
                    ),
                ));
            };
            respond(source, &mut declaration.function, name.span)?;
            name.sym.to_string()
        }
        listener => {
            let mut function = function_of(listener).map_err(|_| {
                source.error(span, "A fetch listener needs to be a function".into())
            })?;
            respond(source, &mut function, span)?;
            module.body.insert(
                position,
                ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Fn(ast::FnDecl {
                    ident: ident(LISTENER, span),
                    declare: false,
                    function,
                }))),
            );
            LISTENER.to_owned()
        }
    };
    let shim = SHIM
        .replace(&format!("'{TYPES}'"), &format!("'{types}'"))
        .replace(LISTENER, &name);
    let mut generated = crate::hir_compat::parse(&shim, "fetch-event.ts")
        .context("generating the fetch-event handler")?;
    generated.visit_mut_with(&mut ClearSpans);
    // Imports lead the module; the handler follows the source.
    let (imports, rest): (Vec<_>, Vec<_>) = generated.body.into_iter().partition(|item| {
        matches!(
            item,
            ast::ModuleItem::ModuleDecl(ast::ModuleDecl::Import(_))
        )
    });
    module.body.splice(0..0, imports);
    module.body.extend(rest);
    Ok(Some(SHIM))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conventions::{ExportedFunction, ImportedResource, Role};

    fn world() -> World {
        World {
            exports: vec![ExportedFunction {
                object: Some(OBJECT.into()),
                member: "handle".into(),
                implementation: "incomingHandlerHandle".into(),
                params: vec![],
                result: None,
                open: false,
                role: Role::Function,
                receiver: None,
            }],
            resources: vec![ImportedResource {
                class: "__jco_WasiHttpTypesFields".into(),
                handle: "WasiHttpTypesFields".into(),
                module: "wasi:http/types@0.2.3".into(),
                drop: "dropFields".into(),
                names: vec![],
            }],
            ..World::default()
        }
    }

    /// The listener the pass declares, as `(async, parameter typed, statements)`, and
    /// whether the handler was generated against the world's version of the resources.
    fn bound(text: &str, listener: &str) -> Result<Option<(bool, bool, usize, bool)>> {
        let mut module = crate::hir_compat::parse(text, "main.ts")?;
        let source = Source {
            text,
            file: "main.ts",
        };
        if bind(&mut module, &source, &world())?.is_none() {
            return Ok(None);
        }
        let versioned = matches!(
            &module.body[0],
            ast::ModuleItem::ModuleDecl(ast::ModuleDecl::Import(import))
                if import.src.value.as_str() == Some("wasi:http/types@0.2.3")
        );
        let handler = find_object(&module, OBJECT).is_some();
        Ok(module.body.iter().find_map(|item| match item {
            ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Fn(function)))
                if function.ident.sym == *listener =>
            {
                let typed = matches!(
                    function.function.params.first().map(|param| &param.pat),
                    Some(ast::Pat::Ident(binding)) if binding.type_ann.is_some()
                );
                Some((
                    function.function.is_async && function.function.return_type.is_some(),
                    typed,
                    function
                        .function
                        .body
                        .as_ref()
                        .map_or(0, |body| body.stmts.len()),
                    versioned && handler,
                ))
            }
            _ => None,
        }))
    }

    #[test]
    fn a_fetch_listener_becomes_a_function_returning_its_response() -> Result<()> {
        for (text, statements) in [
            // The statements of an immediately called function produce the response.
            (
                "addEventListener('fetch', (event) => event.respondWith((async () => { const text = 'Hello'; return new Response(text); })()));",
                2,
            ),
            (
                "self.addEventListener('fetch', function (event) { const request = event.request; event.respondWith(reply(request)); });
                 async function reply(request: Request): Promise<Response> { return new Response(request.url); }",
                2,
            ),
            (
                "addEventListener('fetch', (event) => { return event.respondWith(new Response('Hello')); });",
                1,
            ),
        ] {
            assert_eq!(
                bound(text, LISTENER)?,
                Some((true, true, statements, true)),
                "{text}"
            );
        }
        // A listener declared by name responds in place.
        assert_eq!(
            bound(
                "function onFetch(event) { event.respondWith(new Response('Hello')); }
                 addEventListener('fetch', onFetch);",
                "onFetch"
            )?,
            Some((true, true, 1, true))
        );
        Ok(())
    }

    #[test]
    fn sources_without_a_fetch_listener_or_with_their_own_handler_are_left_alone() -> Result<()> {
        for text in [
            "export function run(): void {}",
            "addEventListener('message', (event) => {});",
            "addEventListener('fetch', (event) => event.respondWith(new Response('Hello')));
             export const incomingHandler = { handle(request, out) {} };",
        ] {
            assert_eq!(bound(text, LISTENER)?, None, "{text}");
        }
        Ok(())
    }

    #[test]
    fn a_listener_that_does_not_end_by_responding_is_rejected() {
        for text in [
            "addEventListener('fetch', (event) => { event.respondWith(new Response('Hello')); done(); });",
            "addEventListener('fetch', (event) => {});",
            "addEventListener('fetch', (event, extra) => event.respondWith(new Response('Hello')));",
        ] {
            let error = bound(text, LISTENER).expect_err(text);
            assert!(
                error
                    .to_string()
                    .contains("needs to end by calling event.respondWith(response)"),
                "{error}"
            );
        }
        let error = bound("addEventListener('fetch', missing);", LISTENER).expect_err("missing");
        assert!(
            error
                .to_string()
                .contains("needs to be a function declared in the entry module"),
            "{error}"
        );
    }
}
