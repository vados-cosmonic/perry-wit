//! Guest conventions shared with other JavaScript component toolchains.
//!
//! Perry-WIT binds each WIT export to one exported function: `add` of interface `math` is
//! `export function mathAdd(...)`. Components written for `jco componentize` export an
//! interface as one object whose properties are its functions:
//!
//! ```ts
//! export const math = { add(a, b) { return a + b; } };
//! ```
//!
//! This pass rewrites the entry module from the second form into the first, before module
//! binding, so the rest of the compiler sees only exported functions.
//!
//! Such components are often plain JavaScript. The world already states each export's
//! signature, so parameters and return types the source leaves out are taken from it.
//!
//! They also use imported resources as classes, which [classes] generates.
mod classes;

use anyhow::Result;
use perry_parser::swc_ecma_ast as ast;
use perry_wit_diagnostics::{DiagnosticCode, PerryDiagnostic, SourceSpan};
use std::collections::{BTreeMap, HashMap, HashSet};
use swc_common::{DUMMY_SP, Span, SyntaxContext};
use swc_ecma_visit::{VisitMut, VisitMutWith};

/// One function the selected world exports, and the names a source may give it.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportedFunction {
    /// The export object holding the function: the interface's camelCase name. `None` for
    /// a function the world exports directly.
    pub object: Option<String>,
    /// The function's camelCase name within `object`, or its exported name.
    pub member: String,
    /// The exported TypeScript function Perry-WIT binds.
    pub implementation: String,
    /// The type of each WIT parameter, in order.
    pub params: Vec<Typed>,
    /// The type of the WIT result.
    pub result: Option<Typed>,
    /// Whether the source's own return type decides how the result is produced, as for a
    /// WIT `result` or `option`.
    pub open: bool,
}

/// A WIT type in the compiler's TypeScript spelling, and where a source written against
/// classes spells it differently.
#[derive(Clone, Debug, PartialEq)]
pub struct Typed {
    /// The compiler's spelling: resources are opaque handles, results `{ ok, value | error }`.
    pub raw: String,
    pub shape: Shape,
    /// How a value of this type is thrown when it is the error of a `result`.
    pub thrown: Thrown,
}

/// The compiler throws strings, numbers, booleans and `Error` objects.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Thrown {
    /// The value itself: a string, number, boolean or enum case.
    Value,
    /// An `Error` whose message is a variant's case name.
    Tag,
    /// An `Error` naming the function that failed.
    Opaque,
}

/// The parts of a type that need conversion between the two spellings.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// Both spellings agree.
    Plain,
    /// An imported resource, by index into [World::resources].
    Resource(usize),
    List(Box<Typed>),
    Option(Box<Typed>),
    Tuple(Vec<Typed>),
    Record(Vec<(String, Typed)>),
    Variant(Vec<(String, Option<Typed>)>),
    /// A `result`, which such sources spell `{ tag: 'ok' | 'err', val }`.
    Result {
        ok: Option<Box<Typed>>,
        err: Option<Box<Typed>>,
    },
}

/// A resource an imported interface declares.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportedResource {
    /// The generated class.
    pub class: String,
    /// The opaque handle type.
    pub handle: String,
    /// The module declaring the resource, and its `drop` function there.
    pub module: String,
    pub drop: String,
    /// Each module and name a source may import the resource under.
    pub names: Vec<(String, String)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Role {
    Function,
    Constructor(usize),
    Method(usize),
    Static(usize),
}

/// A function of an imported interface.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportedFunction {
    pub module: String,
    /// The name the compiler binds the function under in `module`.
    pub binding: String,
    /// The function's or method's camelCase name.
    pub member: String,
    pub role: Role,
    /// Parameters, without a method's receiver.
    pub params: Vec<Typed>,
    pub result: Option<Typed>,
    pub is_async: bool,
}

/// The selected world as guest conventions name it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct World {
    pub exports: Vec<ExportedFunction>,
    pub imports: Vec<ImportedFunction>,
    pub resources: Vec<ImportedResource>,
}

/// Where an export object is declared in the entry module.
struct ObjectSite {
    /// Index of the module item declaring it.
    item: usize,
    /// Index of its declarator within that item.
    declarator: usize,
    /// The local binding, which differs from the export name under `export { a as b }`.
    local: String,
}

struct Source<'a> {
    text: &'a str,
    file: &'a str,
}
impl Source<'_> {
    fn error(&self, span: Span, message: String) -> anyhow::Error {
        let mut diagnostic = PerryDiagnostic::error(DiagnosticCode::UnsupportedExpression, message);
        if !span.is_dummy() {
            diagnostic = diagnostic.with_span(SourceSpan::new(
                self.file,
                self.text,
                span.lo.0.saturating_sub(1) as usize,
                span.hi.0.saturating_sub(1) as usize,
            ));
        }
        diagnostic.into()
    }
}

fn ident(name: &str, span: Span) -> ast::Ident {
    ast::Ident::new(name.into(), span, SyntaxContext::empty())
}

fn export(function: ast::FnDecl) -> ast::ModuleItem {
    ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDecl(ast::ExportDecl {
        span: DUMMY_SP,
        decl: ast::Decl::Fn(function),
    }))
}

/// The function a property or binding holds, when it is written as one.
fn function_of(expression: ast::Expr) -> std::result::Result<Box<ast::Function>, ast::Expr> {
    match expression {
        ast::Expr::Fn(function) => Ok(function.function),
        ast::Expr::Arrow(arrow) => {
            let body = match *arrow.body {
                ast::ArrowFunctionBody::FunctionBody(block) => block,
                ast::ArrowFunctionBody::Expr(value) => ast::FunctionBody {
                    span: arrow.span,
                    stmts: vec![ast::Stmt::Return(ast::ReturnStmt {
                        span: arrow.span,
                        arg: Some(value),
                    })],
                },
            };
            Ok(Box::new(ast::Function {
                params: arrow
                    .params
                    .into_iter()
                    .map(|pat| ast::Param {
                        span: DUMMY_SP,
                        decorators: vec![],
                        pat,
                    })
                    .collect(),
                decorators: vec![],
                this_param: None,
                span: arrow.span,
                ctxt: SyntaxContext::empty(),
                body: Some(body),
                is_generator: arrow.is_generator,
                is_async: arrow.is_async,
                type_params: arrow.type_params,
                return_type: arrow.return_type,
            }))
        }
        ast::Expr::Paren(paren) => function_of(*paren.expr),
        ast::Expr::TsAs(cast) => function_of(*cast.expr),
        ast::Expr::TsSatisfies(cast) => function_of(*cast.expr),
        other => Err(other),
    }
}

fn is_function(expression: &ast::Expr) -> bool {
    match expression {
        ast::Expr::Fn(_) | ast::Expr::Arrow(_) => true,
        ast::Expr::Paren(paren) => is_function(&paren.expr),
        ast::Expr::TsAs(cast) => is_function(&cast.expr),
        ast::Expr::TsSatisfies(cast) => is_function(&cast.expr),
        _ => false,
    }
}

/// The object literal an export object is initialized with.
fn object_literal(expression: &ast::Expr) -> Option<&ast::ObjectLit> {
    match expression {
        ast::Expr::Object(object) => Some(object),
        ast::Expr::Paren(paren) => object_literal(&paren.expr),
        ast::Expr::TsAs(cast) => object_literal(&cast.expr),
        ast::Expr::TsSatisfies(cast) => object_literal(&cast.expr),
        ast::Expr::TsConstAssertion(cast) => object_literal(&cast.expr),
        _ => None,
    }
}

fn into_object_literal(expression: ast::Expr) -> Option<ast::ObjectLit> {
    match expression {
        ast::Expr::Object(object) => Some(object),
        ast::Expr::Paren(paren) => into_object_literal(*paren.expr),
        ast::Expr::TsAs(cast) => into_object_literal(*cast.expr),
        ast::Expr::TsSatisfies(cast) => into_object_literal(*cast.expr),
        ast::Expr::TsConstAssertion(cast) => into_object_literal(*cast.expr),
        _ => None,
    }
}

fn variable(item: &ast::ModuleItem) -> Option<(&ast::VarDecl, bool)> {
    match item {
        ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDecl(ast::ExportDecl {
            decl: ast::Decl::Var(var),
            ..
        })) => Some((var, true)),
        ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Var(var))) => Some((var, false)),
        _ => None,
    }
}

fn variable_mut(item: &mut ast::ModuleItem) -> Option<&mut ast::VarDecl> {
    match item {
        ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDecl(ast::ExportDecl {
            decl: ast::Decl::Var(var),
            ..
        })) => Some(var),
        ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Var(var))) => Some(var),
        _ => None,
    }
}

/// Local bindings the module exports under another or the same name: `export { a as b }`.
fn named_exports(module: &ast::Module) -> HashMap<String, String> {
    let mut exports = HashMap::new();
    for item in &module.body {
        let ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportNamed(named)) = item else {
            continue;
        };
        if named.src.is_some() {
            continue;
        }
        for specifier in &named.specifiers {
            if let ast::ExportSpecifier::Named(specifier) = specifier
                && let ast::ModuleExportName::Ident(local) = &specifier.orig
            {
                let exported = specifier
                    .exported
                    .as_ref()
                    .map_or_else(|| local.sym.to_string(), |name| name.atom().to_string());
                exports.insert(exported, local.sym.to_string());
            }
        }
    }
    exports
}

/// Drop `export { local as exported }` once the binding it named is rewritten.
fn remove_named_export(module: &mut ast::Module, exported: &str) {
    for item in &mut module.body {
        if let ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportNamed(named)) = item
            && named.src.is_none()
        {
            named.specifiers.retain(|specifier| match specifier {
                ast::ExportSpecifier::Named(specifier) => {
                    specifier
                        .exported
                        .as_ref()
                        .unwrap_or(&specifier.orig)
                        .atom()
                        .as_ref()
                        != exported
                }
                _ => true,
            });
        }
    }
    module.body.retain(|item| {
        !matches!(
            item,
            ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportNamed(named))
                if named.src.is_none() && named.specifiers.is_empty()
        )
    });
}

fn find_object(module: &ast::Module, name: &str) -> Option<ObjectSite> {
    let named = named_exports(module);
    module.body.iter().enumerate().find_map(|(item, node)| {
        let (var, exported) = variable(node)?;
        var.decls.iter().enumerate().find_map(|(declarator, decl)| {
            let ast::Pat::Ident(binding) = &decl.name else {
                return None;
            };
            let local = binding.id.sym.as_ref();
            let is_export = if exported {
                local == name
            } else {
                named.get(name).is_some_and(|target| target == local)
            };
            (is_export && object_literal(decl.init.as_deref()?).is_some()).then(|| ObjectSite {
                item,
                declarator,
                local: local.to_owned(),
            })
        })
    })
}

/// Rewrite `object.member` and, inside the object's own methods, `this.member`.
struct Members<'a> {
    object: &'a str,
    names: &'a HashMap<String, String>,
    /// Whether `this` is the export object here.
    this: bool,
}
impl VisitMut for Members<'_> {
    fn visit_mut_expr(&mut self, expression: &mut ast::Expr) {
        expression.visit_mut_children_with(self);
        let ast::Expr::Member(member) = expression else {
            return;
        };
        let on_object = match member.obj.as_ref() {
            ast::Expr::Ident(object) => object.sym == self.object,
            ast::Expr::This(_) => self.this,
            _ => false,
        };
        if on_object
            && let ast::MemberProp::Ident(property) = &member.prop
            && let Some(name) = self.names.get(property.sym.as_ref())
        {
            *expression = ast::Expr::Ident(ident(name, member.span));
        }
    }
    // A nested function or class has its own `this`; an arrow function does not.
    fn visit_mut_function(&mut self, function: &mut ast::Function) {
        let this = std::mem::replace(&mut self.this, false);
        function.visit_mut_children_with(self);
        self.this = this;
    }
    fn visit_mut_class(&mut self, class: &mut ast::Class) {
        let this = std::mem::replace(&mut self.this, false);
        class.visit_mut_children_with(self);
        self.this = this;
    }
}

fn property_name(source: &Source<'_>, key: &ast::PropName) -> Result<String> {
    match key {
        ast::PropName::Ident(name) => Ok(name.sym.to_string()),
        ast::PropName::Str(name) => Ok(name.value.to_string_lossy().into_owned()),
        other => Err(source.error(
            perry_parser::Spanned::span(other),
            "A component export object needs plain property names".into(),
        )),
    }
}

/// Replace one export object with a function per property.
fn flatten_object(
    module: &mut ast::Module,
    source: &Source<'_>,
    object: &str,
    site: ObjectSite,
    implementations: &HashMap<&str, &str>,
) -> Result<()> {
    let ObjectSite {
        item,
        declarator,
        local,
    } = site;
    let var = variable_mut(&mut module.body[item]).expect("an export object is a variable");
    let declaration = var.decls.remove(declarator);
    let span = declaration.span;
    let literal = into_object_literal(*declaration.init.expect("an export object is initialized"))
        .expect("an export object is an object literal");
    let remaining = !var.decls.is_empty();

    // Properties outside the world stay reachable through `object.member` as internals.
    let mut names = HashMap::new();
    let mut properties = Vec::new();
    for property in literal.props {
        let ast::PropOrSpread::Prop(property) = property else {
            return Err(source.error(
                span,
                format!("Component export '{object}' cannot spread another object"),
            ));
        };
        let (name, value, key_span) = match *property {
            ast::Prop::Shorthand(id) => (id.sym.to_string(), ast::Expr::Ident(id.clone()), id.span),
            ast::Prop::KeyValue(pair) => {
                let key_span = perry_parser::Spanned::span(&pair.key);
                (property_name(source, &pair.key)?, *pair.value, key_span)
            }
            ast::Prop::Method(method) => {
                let key_span = perry_parser::Spanned::span(&method.key);
                (
                    property_name(source, &method.key)?,
                    ast::Expr::Fn(ast::FnExpr {
                        ident: None,
                        function: method.function,
                    }),
                    key_span,
                )
            }
            other => {
                return Err(source.error(
                    perry_parser::Spanned::span(&other),
                    format!("Component export '{object}' needs function properties"),
                ));
            }
        };
        let target = implementations.get(name.as_str()).map_or_else(
            || format!("__jco_{object}_{name}"),
            |name| (*name).to_owned(),
        );
        names.insert(name.clone(), target.clone());
        properties.push((name, target, value, key_span));
    }

    let mut generated = Vec::new();
    let mut aliases = Vec::new();
    for (name, target, value, key_span) in properties {
        let exported = implementations.contains_key(name.as_str());
        match function_of(value) {
            Ok(mut function) => {
                function.visit_mut_children_with(&mut Members {
                    object: &local,
                    names: &names,
                    this: true,
                });
                let function = ast::FnDecl {
                    ident: ident(&target, key_span),
                    declare: false,
                    function,
                };
                generated.push(if exported {
                    export(function)
                } else {
                    ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Fn(function)))
                });
            }
            // `{ run }` and `{ run: implementation }` name a function declared elsewhere.
            Err(ast::Expr::Ident(binding)) if exported => aliases.push((binding, target)),
            Err(_) if exported => {
                return Err(source.error(
                    key_span,
                    format!(
                        "Component export '{object}.{name}' needs a function: a method, a function expression or the name of a function declaration"
                    ),
                ));
            }
            Err(value) => generated.push(ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Var(
                Box::new(ast::VarDecl {
                    span: key_span,
                    ctxt: SyntaxContext::empty(),
                    kind: ast::VarDeclKind::Const,
                    declare: false,
                    decls: vec![ast::VarDeclarator {
                        span: key_span,
                        name: ast::Pat::Ident(ident(&target, key_span).into()),
                        init: Some(Box::new(value)),
                        definite: false,
                    }],
                }),
            )))),
        }
    }
    if !aliases.is_empty() {
        generated.push(ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportNamed(
            ast::NamedExport {
                span: DUMMY_SP,
                specifiers: aliases
                    .into_iter()
                    .map(|(binding, target)| {
                        ast::ExportSpecifier::Named(ast::ExportNamedSpecifier {
                            span: DUMMY_SP,
                            orig: ast::ModuleExportName::Ident(binding),
                            exported: Some(ast::ModuleExportName::Ident(ident(&target, DUMMY_SP))),
                            is_type_only: false,
                        })
                    })
                    .collect(),
                src: None,
                type_only: false,
                with: None,
            },
        )));
    }

    if remaining {
        module.body.splice(item + 1..item + 1, generated);
    } else {
        module.body.splice(item..=item, generated);
    }
    remove_named_export(module, object);
    module.visit_mut_with(&mut Members {
        object: &local,
        names: &names,
        this: false,
    });
    Ok(())
}

/// Rewrite `export const run = () => { ... }` into `export function run() { ... }`.
fn declare_functions(module: &mut ast::Module, names: &HashSet<&str>) {
    let named = named_exports(module);
    let mut index = 0;
    while index < module.body.len() {
        let Some((var, exported)) = variable(&module.body[index]) else {
            index += 1;
            continue;
        };
        let position = var.decls.iter().position(|decl| {
            let ast::Pat::Ident(binding) = &decl.name else {
                return false;
            };
            let local = binding.id.sym.as_ref();
            let bound = if exported {
                names.contains(local)
            } else {
                named
                    .iter()
                    .any(|(exported, target)| target == local && names.contains(exported.as_str()))
            };
            bound && decl.init.as_deref().is_some_and(is_function)
        });
        let Some(position) = position else {
            index += 1;
            continue;
        };
        let var = variable_mut(&mut module.body[index]).expect("matched a variable above");
        let declaration = var.decls.remove(position);
        let remaining = !var.decls.is_empty();
        let ast::Pat::Ident(binding) = declaration.name else {
            unreachable!("matched an identifier binding above");
        };
        let function = function_of(*declaration.init.expect("matched an initializer above"))
            .unwrap_or_else(|_| unreachable!("matched a function above"));
        let function = ast::FnDecl {
            ident: binding.id,
            declare: false,
            function,
        };
        let item = if exported {
            export(function)
        } else {
            ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Fn(function)))
        };
        if remaining {
            module.body.insert(index + 1, item);
        } else {
            module.body[index] = item;
        }
    }
}

struct ClearSpans;
impl VisitMut for ClearSpans {
    fn visit_mut_span(&mut self, span: &mut Span) {
        *span = DUMMY_SP;
    }
}

/// Type annotations parsed from the TypeScript spelling of each type.
fn annotations(types: &[String]) -> Option<Vec<Option<Box<ast::TsTypeAnn>>>> {
    let params: Vec<_> = types
        .iter()
        .enumerate()
        .map(|(index, ty)| format!("p{index}: {ty}"))
        .collect();
    let declaration = format!("function f({}) {{}}", params.join(", "));
    let mut module = crate::hir_compat::parse(&declaration, "wit-signature.ts").ok()?;
    module.visit_mut_with(&mut ClearSpans);
    let ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Fn(function))) = module.body.pop()? else {
        return None;
    };
    Some(
        function
            .function
            .params
            .into_iter()
            .map(|param| match param.pat {
                ast::Pat::Ident(binding) => binding.type_ann,
                _ => None,
            })
            .collect(),
    )
}

/// Give an export's implementation the types of its WIT signature where it declares none.
fn annotate(
    source: &Source<'_>,
    name: &ast::Ident,
    function: &mut ast::Function,
    export: &ExportedFunction,
    params: &[String],
    result: Option<String>,
) -> Result<()> {
    if function.params.len() > params.len() {
        let count =
            |count: usize| format!("{count} parameter{}", if count == 1 { "" } else { "s" });
        return Err(source.error(
            name.span,
            format!(
                "Component export '{}' declares {}, but its WIT function takes {}",
                export.implementation,
                count(function.params.len()),
                count(params.len()),
            ),
        ));
    }
    let result = result
        .filter(|_| function.return_type.is_none() && !function.is_generator)
        .map(|result| {
            if function.is_async {
                format!("Promise<{result}>")
            } else {
                result
            }
        });
    let mut types = params[..function.params.len()].to_vec();
    types.extend(result.clone());
    let Some(mut types) = annotations(&types) else {
        return Ok(());
    };
    if result.is_some() {
        function.return_type = types.pop().flatten();
    }
    for (param, ty) in function.params.iter_mut().zip(types) {
        let declared = match &mut param.pat {
            ast::Pat::Ident(binding) => &mut binding.type_ann,
            ast::Pat::Array(pattern) => &mut pattern.type_ann,
            ast::Pat::Object(pattern) => &mut pattern.type_ann,
            ast::Pat::Assign(pattern) => match pattern.left.as_mut() {
                ast::Pat::Ident(binding) => &mut binding.type_ann,
                _ => continue,
            },
            _ => continue,
        };
        if declared.is_none() {
            *declared = ty;
        }
    }
    Ok(())
}

/// Whether an export's values differ between the compiler's representation and the
/// source's. A `result` export returns its payload either way.
fn converts(export: &ExportedFunction) -> bool {
    let result = match export.result.as_ref() {
        Some(Typed {
            shape: Shape::Result { ok, .. },
            ..
        }) => ok.as_deref(),
        other => other,
    };
    export.params.iter().any(|ty| ty.shape != Shape::Plain)
        || result.is_some_and(|ty| ty.shape != Shape::Plain)
}

struct Rename<'a>(&'a HashMap<String, String>);
impl VisitMut for Rename<'_> {
    fn visit_mut_ident(&mut self, id: &mut ast::Ident) {
        if let Some(name) = self.0.get(id.sym.as_ref()) {
            id.sym = name.as_str().into();
        }
    }
}

/// Type every implementation of a world export from its WIT signature. With `classes`, an
/// export whose values need conversion becomes an internal function behind a generated
/// export.
fn annotate_exports(
    module: &mut ast::Module,
    source: &Source<'_>,
    world: &World,
    mut classes: Option<&mut classes::Classes<'_>>,
) -> Result<()> {
    let by_name: HashMap<&str, &ExportedFunction> = world
        .exports
        .iter()
        .map(|export| (export.implementation.as_str(), export))
        .collect();
    // A function declaration exported by name is typed as its first world export.
    let mut locals: HashMap<String, &ExportedFunction> = HashMap::new();
    for (exported, local) in named_exports(module) {
        if let Some(export) = by_name.get(exported.as_str()) {
            let current = locals.entry(local).or_insert(export);
            if current.implementation > export.implementation {
                *current = export;
            }
        }
    }
    let mut internal = HashMap::new();
    let mut unexported = Vec::new();
    for item in &mut module.body {
        let (function, export, exported) = match item {
            ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDecl(ast::ExportDecl {
                decl: ast::Decl::Fn(function),
                ..
            })) => match by_name.get(function.ident.sym.as_ref()) {
                Some(export) => (function, *export, true),
                None => continue,
            },
            ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Fn(function))) => {
                match locals.get(function.ident.sym.as_ref()) {
                    Some(export) => (function, *export, false),
                    None => continue,
                }
            }
            _ => continue,
        };
        let result = |text: Option<String>| {
            (!export.open).then(|| text.unwrap_or_else(|| "void".to_owned()))
        };
        match classes.as_deref_mut().filter(|_| converts(export)) {
            Some(classes) => {
                let params: Vec<_> = export
                    .params
                    .iter()
                    .map(|ty| classes.source_type(ty))
                    .collect();
                let returned = export.result.as_ref().map(|ty| classes.source_type(ty));
                annotate(
                    source,
                    &function.ident,
                    &mut function.function,
                    export,
                    &params,
                    result(returned),
                )?;
                let inner = if exported {
                    let inner = format!("__jco_impl_{}", export.implementation);
                    internal.insert(export.implementation.clone(), inner.clone());
                    inner
                } else {
                    unexported.push(export.implementation.clone());
                    function.ident.sym.to_string()
                };
                classes.export(
                    export,
                    &inner,
                    function.function.params.len(),
                    function.function.is_async,
                );
            }
            None => {
                let params: Vec<_> = export.params.iter().map(|ty| ty.raw.clone()).collect();
                let returned = export.result.as_ref().map(|ty| ty.raw.clone());
                annotate(
                    source,
                    &function.ident,
                    &mut function.function,
                    export,
                    &params,
                    result(returned),
                )?;
            }
        }
    }
    // The source's implementation, and its own calls to it, move behind the generated export.
    for item in &mut module.body {
        if let ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDecl(ast::ExportDecl {
            decl: ast::Decl::Fn(function),
            ..
        })) = item
            && internal.contains_key(function.ident.sym.as_ref())
        {
            *item = ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Fn(function.clone())));
        }
    }
    if !internal.is_empty() {
        module.visit_mut_with(&mut Rename(&internal));
    }
    for exported in unexported {
        remove_named_export(module, &exported);
    }
    Ok(())
}

/// Rewrite the entry module's export objects and function-valued bindings into the
/// exported functions the world's contract binds, typed from the contract, and its
/// imported resources into classes.
pub(crate) fn apply(
    module: &mut ast::Module,
    world: &World,
    foreign: &HashMap<String, crate::hir::Foreign>,
    sources: &BTreeMap<String, String>,
    text: &str,
    file: &str,
) -> Result<()> {
    let source = Source { text, file };
    let exports = &world.exports;
    let mut objects: Vec<&str> = Vec::new();
    for export in exports {
        if let Some(object) = export.object.as_deref()
            && !objects.contains(&object)
        {
            objects.push(object);
        }
    }
    let mut flattened = false;
    for object in objects {
        let Some(site) = find_object(module, object) else {
            continue;
        };
        let implementations = exports
            .iter()
            .filter(|export| export.object.as_deref() == Some(object))
            .map(|export| (export.member.as_str(), export.implementation.as_str()))
            .collect();
        flatten_object(module, &source, object, site, &implementations)?;
        flattened = true;
    }
    let names = exports
        .iter()
        .map(|export| export.implementation.as_str())
        .collect();
    declare_functions(module, &names);
    // A source that exports objects or constructs resources is written against classes.
    if !flattened && !classes::imports_resource(module, world) {
        return annotate_exports(module, &source, world, None);
    }
    let used = |name: &str| text.contains(name) || sources.values().any(|text| text.contains(name));
    let mut classes = classes::Classes::new(world, foreign, used);
    classes.bind_imports(module);
    annotate_exports(module, &source, world, Some(&mut classes))?;
    classes.finish(module)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(raw: &str) -> Typed {
        Typed {
            raw: raw.to_owned(),
            shape: Shape::Plain,
            thrown: Thrown::Value,
        }
    }

    fn exports(entries: &[(Option<&str>, &str, &str)]) -> World {
        World {
            exports: entries
                .iter()
                .map(|(object, member, implementation)| ExportedFunction {
                    object: object.map(str::to_owned),
                    member: (*member).to_owned(),
                    implementation: (*implementation).to_owned(),
                    params: vec![plain("number"), plain("Array<string>")],
                    result: Some(plain("string")),
                    open: false,
                })
                .collect(),
            ..World::default()
        }
    }

    fn run(module: &mut ast::Module, world: &World, source: &str) -> Result<()> {
        apply(
            module,
            world,
            &HashMap::new(),
            &BTreeMap::new(),
            source,
            "main.ts",
        )
    }

    /// Whether each parameter of the first function named `name` is typed, and its return.
    fn typed(source: &str, exports: &World, name: &str) -> Result<(Vec<bool>, bool)> {
        let mut module = crate::hir_compat::parse(source, "main.ts")?;
        run(&mut module, exports, source)?;
        let function = module
            .body
            .iter()
            .find_map(|item| match item {
                ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDecl(ast::ExportDecl {
                    decl: ast::Decl::Fn(function),
                    ..
                }))
                | ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Fn(function)))
                    if function.ident.sym == name =>
                {
                    Some(&function.function)
                }
                _ => None,
            })
            .expect("the function is declared");
        let params = function
            .params
            .iter()
            .map(|param| match &param.pat {
                ast::Pat::Ident(binding) => binding.type_ann.is_some(),
                _ => false,
            })
            .collect();
        Ok((params, function.return_type.is_some()))
    }

    #[test]
    fn untyped_exports_take_their_types_from_the_world() -> Result<()> {
        let world = exports(&[
            (Some("math"), "add", "mathAdd"),
            (None, "run", "run"),
            (None, "stop", "stop"),
        ]);
        let source = "export const math = { add(a, b) { return helper(a); } };
            function helper(value) { return ''; }
            export function run(a) { return ''; }
            function stopped(a, b) { return ''; }
            export { stopped as stop };";
        assert_eq!(typed(source, &world, "mathAdd")?, (vec![true, true], true));
        assert_eq!(typed(source, &world, "run")?, (vec![true], true));
        assert_eq!(typed(source, &world, "stopped")?, (vec![true, true], true));
        // Functions the world does not export keep their own types.
        assert_eq!(typed(source, &world, "helper")?, (vec![false], false));
        Ok(())
    }

    #[test]
    fn an_export_with_more_parameters_than_the_world_is_rejected() {
        let error = typed(
            "export function run(a, b, c) {}",
            &exports(&[(None, "run", "run")]),
            "run",
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("declares 3 parameters, but its WIT function takes 2 parameters"),
            "{error}"
        );
    }

    /// The module's exported function names, then every other top-level declaration.
    fn shape(source: &str, exports: &World) -> Result<(Vec<String>, Vec<String>)> {
        let mut module = crate::hir_compat::parse(source, "main.ts")?;
        run(&mut module, exports, source)?;
        let mut exported = Vec::new();
        let mut internal = Vec::new();
        for item in &module.body {
            match item {
                ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDecl(export)) => {
                    match &export.decl {
                        ast::Decl::Fn(function) => exported.push(function.ident.sym.to_string()),
                        ast::Decl::Var(var) => {
                            for decl in &var.decls {
                                if let ast::Pat::Ident(name) = &decl.name {
                                    internal.push(format!("export const {}", name.id.sym));
                                }
                            }
                        }
                        _ => {}
                    }
                }
                ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportNamed(named)) => {
                    for specifier in &named.specifiers {
                        if let ast::ExportSpecifier::Named(specifier) = specifier {
                            let exported_name =
                                specifier.exported.as_ref().unwrap_or(&specifier.orig);
                            exported.push(format!(
                                "{} as {}",
                                specifier.orig.atom(),
                                exported_name.atom()
                            ));
                        }
                    }
                }
                ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Fn(function))) => {
                    internal.push(function.ident.sym.to_string());
                }
                ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Var(var))) => {
                    for decl in &var.decls {
                        if let ast::Pat::Ident(name) = &decl.name {
                            internal.push(format!("const {}", name.id.sym));
                        }
                    }
                }
                _ => {}
            }
        }
        Ok((exported, internal))
    }

    const MATH: [(Option<&str>, &str, &str); 2] = [
        (Some("math"), "add", "mathAdd"),
        (Some("math"), "twice", "mathTwice"),
    ];

    #[test]
    fn an_export_object_becomes_one_exported_function_per_world_member() -> Result<()> {
        let (exported, internal) = shape(
            "export const math = { add(a: number, b: number) { return a + b; }, twice: (a: number) => this.add(a, a), helper() { return 1; }, limit: 3 };",
            &exports(&MATH),
        )?;
        assert_eq!(exported, ["mathAdd", "mathTwice"]);
        assert_eq!(internal, ["__jco_math_helper", "const __jco_math_limit"]);
        Ok(())
    }

    #[test]
    fn an_object_exported_by_name_and_function_names_are_bound() -> Result<()> {
        let (exported, internal) = shape(
            "function add(a: number, b: number) { return a + b; }\nconst implementation = { add, twice: add };\nexport { implementation as math };",
            &exports(&MATH),
        )?;
        assert_eq!(exported, ["add as mathAdd", "add as mathTwice"]);
        assert_eq!(internal, ["add"]);
        Ok(())
    }

    #[test]
    fn a_function_valued_binding_becomes_a_function_declaration() -> Result<()> {
        let world = exports(&[(None, "run", "run"), (None, "stop", "stop")]);
        let (exported, internal) = shape(
            "export const run = async (): Promise<number> => 1, other = 2;\nconst stop = function () {};\nexport { stop };",
            &world,
        )?;
        assert_eq!(exported, ["run", "stop as stop"]);
        assert_eq!(internal, ["export const other", "stop"]);
        Ok(())
    }

    #[test]
    fn sources_outside_the_convention_are_left_alone() -> Result<()> {
        let (exported, internal) = shape(
            "export function mathAdd(a: number, b: number) { return a + b; }\nconst math = { add: 1 };\nexport const config = { add() {} };",
            &exports(&MATH),
        )?;
        assert_eq!(exported, ["mathAdd"]);
        assert_eq!(internal, ["const math", "export const config"]);
        Ok(())
    }

    #[test]
    fn a_world_member_that_is_not_a_function_is_rejected() {
        let error = shape(
            "export const math = { add: 1, twice() {} };",
            &exports(&MATH),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Component export 'math.add' needs a function"),
            "{error}"
        );
    }

    /// A world importing `t:t/res` with one resource: a constructor, `hello` and `make`.
    fn resource_world() -> (World, HashMap<String, crate::hir::Foreign>) {
        let handle = Typed {
            raw: "TTResExample".into(),
            shape: Shape::Resource(0),
            thrown: Thrown::Opaque,
        };
        let function =
            |binding: &str, member: &str, role, result: Option<Typed>| ImportedFunction {
                module: "t:t/res".into(),
                binding: binding.into(),
                member: member.into(),
                role,
                params: vec![],
                result,
                is_async: false,
            };
        let world = World {
            exports: vec![ExportedFunction {
                object: None,
                member: "run".into(),
                implementation: "run".into(),
                params: vec![],
                result: Some(plain("string")),
                open: false,
            }],
            imports: vec![
                function(
                    "ConstructorExample",
                    "constructor",
                    Role::Constructor(0),
                    None,
                ),
                function(
                    "MethodExampleHello",
                    "hello",
                    Role::Method(0),
                    Some(plain("string")),
                ),
                function("MethodExampleUnused", "unused", Role::Method(0), None),
                function(
                    "StaticExampleMake",
                    "make",
                    Role::Static(0),
                    Some(handle.clone()),
                ),
                function("find", "find", Role::Function, Some(handle)),
                function("count", "count", Role::Function, Some(plain("number"))),
            ],
            resources: vec![ImportedResource {
                class: "__jco_TTResExample".into(),
                handle: "TTResExample".into(),
                module: "t:t/res".into(),
                drop: "dropExample".into(),
                names: vec![("t:t/res".into(), "Example".into())],
            }],
        };
        let foreign = [
            "ConstructorExample",
            "MethodExampleHello",
            "MethodExampleUnused",
            "StaticExampleMake",
            "find",
            "count",
            "dropExample",
        ]
        .into_iter()
        .map(|name| (format!("t:t/res#{name}"), crate::hir::Foreign::Resource(0)))
        .collect();
        (world, foreign)
    }

    /// Top-level names the pass leaves in the module, and the names its imports bind.
    fn declared(source: &str) -> Result<(Vec<String>, Vec<String>)> {
        let (world, foreign) = resource_world();
        let mut module = crate::hir_compat::parse(source, "main.ts")?;
        apply(
            &mut module,
            &world,
            &foreign,
            &BTreeMap::new(),
            source,
            "main.ts",
        )?;
        let mut names = Vec::new();
        let mut imports = Vec::new();
        for item in &module.body {
            match item {
                ast::ModuleItem::ModuleDecl(ast::ModuleDecl::Import(import)) => {
                    for specifier in &import.specifiers {
                        if let ast::ImportSpecifier::Named(named) = specifier {
                            let imported = named.imported.as_ref().map(|name| name.atom());
                            imports.push(imported.map_or_else(
                                || named.local.sym.to_string(),
                                |name| name.to_string(),
                            ));
                        }
                    }
                }
                ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Class(class))) => {
                    names.push(format!("class {}", class.ident.sym));
                    for member in &class.class.body {
                        if let ast::ClassMember::Method(method) = member
                            && let ast::PropName::Ident(name) = &method.key
                        {
                            names.push(format!("  {}", name.sym));
                        }
                    }
                }
                ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Fn(function)))
                | ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDecl(ast::ExportDecl {
                    decl: ast::Decl::Fn(function),
                    ..
                })) => names.push(function.ident.sym.to_string()),
                _ => {}
            }
        }
        imports.sort();
        Ok((names, imports))
    }

    #[test]
    fn an_imported_resource_becomes_a_class_with_the_methods_the_source_uses() -> Result<()> {
        let (names, imports) = declared(
            "import { Example, find, count } from 't:t/res';
             export function run(): string {
               using made = Example.make();
               return new Example().hello() + find().hello() + count();
             }",
        )?;
        assert_eq!(
            names,
            [
                "run",
                // `find` returns a resource, so a converting function replaces its import.
                "__jco_out0",
                "find",
                "class __jco_TTResExample",
                "  hello",
                "  make",
                "  __dispose",
                "__jco_new__jco_TTResExample",
            ]
        );
        // `count` needs no conversion and stays imported; the rest are the bindings the
        // generated code calls. `unused` is never mentioned, so it is not bound.
        assert_eq!(
            imports,
            [
                "ConstructorExample",
                "MethodExampleHello",
                "StaticExampleMake",
                "count",
                "dropExample",
                "find",
            ]
        );
        Ok(())
    }

    #[test]
    fn a_source_using_opaque_handles_keeps_its_imports() -> Result<()> {
        let (names, imports) = declared(
            "import { find, count, dropExample } from 't:t/res';
             export function run(): string { dropExample(find()); return '' + count(); }",
        )?;
        assert_eq!(names, ["run"]);
        assert_eq!(imports, ["count", "dropExample", "find"]);
        Ok(())
    }
}
