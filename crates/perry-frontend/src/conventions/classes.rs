//! Imported WIT resources as classes.
//!
//! The compiler models a resource as an opaque handle and its constructor, methods and
//! static functions as imported functions taking that handle. Sources written for
//! `jco componentize` use a class instead:
//!
//! ```ts
//! import { Example } from 'test:component/resources';
//! const example = new Example();
//! example.hello('world');
//! ```
//!
//! This module generates such a class around the handle, in TypeScript, for each resource
//! the entry module can reach, and converts values at the boundary: a handle an import
//! returns is wrapped, an instance passed to one is unwrapped, and a function declared to
//! return a WIT `result` returns its payload or throws its error.
use super::{ClearSpans, ImportedFunction, Role, Shape, Thrown, Typed, World};
use crate::hir::Foreign;
use anyhow::{Context as _, Result};
use perry_parser::swc_ecma_ast as ast;
use std::collections::HashMap;
use std::fmt::Write as _;
use swc_common::{DUMMY_SP, SyntaxContext};
use swc_ecma_visit::{VisitMut, VisitMutWith};

/// The method a class implements for `[Symbol.dispose]`, and `using` calls.
const DISPOSE: &str = "__dispose";

pub(super) struct Classes<'a> {
    world: &'a World,
    foreign: &'a HashMap<String, Foreign>,
    /// Whether any source mentions a member name: unused methods are not generated.
    used: Box<dyn Fn(&str) -> bool + 'a>,
    /// Generated helper functions, wrappers and exports.
    code: String,
    /// Aliases of the imported functions the generated code calls, by module and name.
    raw: HashMap<(String, String), String>,
    /// Conversion helpers by raw type and direction.
    conversions: HashMap<(String, bool), String>,
    /// Resources whose class is needed, and whether it is generated yet.
    needed: Vec<Option<bool>>,
    /// Source names of resources, renamed to their classes.
    renamed: HashMap<String, String>,
}

/// Whether the entry module imports a resource as a value, to construct it or call its
/// static functions.
pub(super) fn imports_resource(module: &ast::Module, world: &World) -> bool {
    module.body.iter().any(|item| {
        let ast::ModuleItem::ModuleDecl(ast::ModuleDecl::Import(import)) = item else {
            return false;
        };
        let source = import.src.value.to_string_lossy();
        !import.type_only
            && import.specifiers.iter().any(|specifier| match specifier {
                ast::ImportSpecifier::Named(named) if !named.is_type_only => {
                    let name = imported_name(named);
                    world.resources.iter().any(|resource| {
                        resource
                            .names
                            .iter()
                            .any(|(module, local)| *module == source && *local == name)
                    })
                }
                _ => false,
            })
    })
}

fn imported_name(named: &ast::ImportNamedSpecifier) -> String {
    named.imported.as_ref().map_or_else(
        || named.local.sym.to_string(),
        |name| name.atom().to_string(),
    )
}

impl<'a> Classes<'a> {
    pub(super) fn new(
        world: &'a World,
        foreign: &'a HashMap<String, Foreign>,
        used: impl Fn(&str) -> bool + 'a,
    ) -> Self {
        Self {
            world,
            foreign,
            used: Box::new(used),
            code: String::new(),
            raw: HashMap::new(),
            conversions: HashMap::new(),
            needed: vec![None; world.resources.len()],
            renamed: HashMap::new(),
        }
    }

    /// The type a source sees for a WIT type: classes for resources, tagged objects for
    /// results.
    pub(super) fn source_type(&mut self, ty: &Typed) -> String {
        match &ty.shape {
            Shape::Plain => ty.raw.clone(),
            Shape::Resource(index) => {
                self.needed[*index].get_or_insert(false);
                self.world.resources[*index].class.clone()
            }
            Shape::List(item) => format!("Array<{}>", self.source_type(item)),
            Shape::Option(item) => format!("{} | null", self.source_type(item)),
            Shape::Tuple(items) => {
                let items: Vec<_> = items.iter().map(|item| self.source_type(item)).collect();
                format!("[{}]", items.join(", "))
            }
            Shape::Record(fields) => {
                let fields: Vec<_> = fields
                    .iter()
                    .map(|(name, field)| format!("{name}: {}", self.source_type(field)))
                    .collect();
                format!("{{ {} }}", fields.join("; "))
            }
            Shape::Variant(cases) => {
                let cases: Vec<_> = cases
                    .iter()
                    .map(|(name, payload)| match payload {
                        Some(payload) => {
                            format!("{{ tag: \"{name}\"; val: {} }}", self.source_type(payload))
                        }
                        None => format!("{{ tag: \"{name}\" }}"),
                    })
                    .collect();
                cases.join(" | ")
            }
            Shape::Result { ok, err } => {
                let mut case = |tag: &str, payload: &Option<Box<Typed>>| match payload {
                    Some(payload) => {
                        format!("{{ tag: \"{tag}\"; val: {} }}", self.source_type(payload))
                    }
                    None => format!("{{ tag: \"{tag}\" }}"),
                };
                format!("{} | {}", case("ok", ok), case("err", err))
            }
        }
    }

    /// `expression` converted between the compiler's representation and the source's:
    /// `out` is from an import's result or an export's parameter towards the source.
    fn converted(&mut self, ty: &Typed, out: bool, expression: &str) -> String {
        match self.conversion(ty, out) {
            Some(function) => format!("{function}({expression})"),
            None => expression.to_owned(),
        }
    }

    /// The helper converting a value of `ty`, or `None` when both sides agree.
    fn conversion(&mut self, ty: &Typed, out: bool) -> Option<String> {
        if ty.shape == Shape::Plain {
            return None;
        }
        let key = (ty.raw.clone(), out);
        if let Some(name) = self.conversions.get(&key) {
            return Some(name.clone());
        }
        let direction = if out { "out" } else { "in" };
        let name = format!("__jco_{direction}{}", self.conversions.len());
        self.conversions.insert(key, name.clone());
        let source = self.source_type(ty);
        let (from, to) = if out {
            (ty.raw.clone(), source)
        } else {
            (source, ty.raw.clone())
        };
        let body = match &ty.shape {
            Shape::Plain => unreachable!("plain values need no conversion"),
            Shape::Resource(index) => {
                if out {
                    format!("return new {}(v);", self.world.resources[*index].class)
                } else {
                    "return v.__h!;".to_owned()
                }
            }
            Shape::List(item) => {
                let item_source = self.source_type(item);
                let (from, to) = if out {
                    (item.raw.clone(), item_source)
                } else {
                    (item_source, item.raw.clone())
                };
                format!(
                    "return v.map((x: {from}): {to} => {});",
                    self.converted(item, out, "x")
                )
            }
            Shape::Option(item) => {
                format!(
                    "return v === null ? null : {};",
                    self.converted(item, out, "v")
                )
            }
            Shape::Tuple(items) => {
                let items: Vec<_> = items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| self.converted(item, out, &format!("v[{index}]")))
                    .collect();
                format!("return [{}];", items.join(", "))
            }
            Shape::Record(fields) => {
                let fields: Vec<_> = fields
                    .iter()
                    .map(|(name, field)| {
                        format!(
                            "{name}: {}",
                            self.converted(field, out, &format!("v.{name}"))
                        )
                    })
                    .collect();
                format!("return {{ {} }};", fields.join(", "))
            }
            Shape::Variant(cases) => {
                let mut body = String::from("switch (v.tag) {");
                for (index, (name, payload)) in cases.iter().enumerate() {
                    let value = match payload {
                        Some(payload) => format!(
                            "{{ tag: \"{name}\", val: {} }}",
                            self.converted(payload, out, "v.val")
                        ),
                        None => format!("{{ tag: \"{name}\" }}"),
                    };
                    if index + 1 == cases.len() {
                        let _ = write!(body, " default: return {value};");
                    } else {
                        let _ = write!(body, " case \"{name}\": return {value};");
                    }
                }
                body.push_str(" }");
                body
            }
            Shape::Result { ok, err } => {
                // The source's `{ tag, val }` against the compiler's `{ ok, value | error }`.
                let (ok_field, err_field) = if out {
                    ("v.value", "v.error")
                } else {
                    ("v.val", "v.val")
                };
                let ok = ok.as_ref().map(|ty| self.converted(ty, out, ok_field));
                let err = err.as_ref().map(|ty| self.converted(ty, out, err_field));
                let field = |name: &str, value: Option<String>| {
                    value.map_or_else(String::new, |value| format!(", {name}: {value}"))
                };
                if out {
                    format!(
                        "if (v.ok) {{ return {{ tag: \"ok\"{} }}; }} return {{ tag: \"err\"{} }};",
                        field("val", ok),
                        field("val", err)
                    )
                } else {
                    format!(
                        "if (v.tag === \"ok\") {{ return {{ ok: true{} }}; }} return {{ ok: false{} }};",
                        field("value", ok),
                        field("error", err)
                    )
                }
            }
        };
        let _ = writeln!(self.code, "function {name}(v: {from}): {to} {{ {body} }}");
        Some(name)
    }

    /// The generated code's name for an imported function, when the compiler binds it.
    fn raw(&mut self, module: &str, binding: &str) -> Option<String> {
        if !self.foreign.contains_key(&format!("{module}#{binding}")) {
            return None;
        }
        let next = self.raw.len();
        Some(
            self.raw
                .entry((module.to_owned(), binding.to_owned()))
                .or_insert_with(|| format!("__jco_raw{next}"))
                .clone(),
        )
    }

    /// The parameter list, return type and body of a wrapper around `function`. A function
    /// declared to return a WIT `result` returns its payload and throws its error.
    fn wrapper(
        &mut self,
        function: &ImportedFunction,
        receiver: Option<&str>,
    ) -> Option<(String, String, String)> {
        let raw = self.raw(&function.module, &function.binding)?;
        let mut params = Vec::new();
        let mut arguments: Vec<String> = receiver.into_iter().map(str::to_owned).collect();
        for (index, param) in function.params.iter().enumerate() {
            params.push(format!("p{index}: {}", self.source_type(param)));
            arguments.push(self.converted(param, false, &format!("p{index}")));
        }
        let wait = if function.is_async { "await " } else { "" };
        let call = format!("{wait}{raw}({})", arguments.join(", "));
        let (result, body) = match &function.result {
            None => ("void".to_owned(), format!("{call};")),
            Some(Typed {
                shape: Shape::Result { ok, err },
                ..
            }) => {
                let error = match err.as_deref() {
                    Some(err) if err.thrown == Thrown::Value => {
                        self.converted(err, true, "r.error")
                    }
                    Some(err) if err.thrown == Thrown::Tag => "new Error(r.error.tag)".to_owned(),
                    _ => format!("new Error(\"{} failed\")", function.member),
                };
                let (result, value) = match ok {
                    Some(ok) => (
                        self.source_type(ok),
                        format!(" return {};", self.converted(ok, true, "r.value")),
                    ),
                    None => ("void".to_owned(), String::new()),
                };
                (
                    result,
                    format!("const r = {call}; if (!r.ok) {{ throw {error}; }}{value}"),
                )
            }
            Some(result) => (
                self.source_type(result),
                format!("return {};", self.converted(result, true, &call)),
            ),
        };
        let result = if function.is_async {
            format!("Promise<{result}>")
        } else {
            result
        };
        Some((params.join(", "), result, body))
    }

    fn keyword(function: &ImportedFunction) -> &'static str {
        if function.is_async { "async " } else { "" }
    }

    /// Take over the entry module's imports of resources and of functions whose values
    /// need conversion.
    pub(super) fn bind_imports(&mut self, module: &mut ast::Module) {
        let world = self.world;
        let mut functions = Vec::new();
        for item in &mut module.body {
            let ast::ModuleItem::ModuleDecl(ast::ModuleDecl::Import(import)) = item else {
                continue;
            };
            let source = import.src.value.to_string_lossy().into_owned();
            import.specifiers.retain(|specifier| {
                let ast::ImportSpecifier::Named(named) = specifier else {
                    return true;
                };
                let name = imported_name(named);
                let local = named.local.sym.to_string();
                if let Some(index) = world.resources.iter().position(|resource| {
                    resource
                        .names
                        .iter()
                        .any(|(module, exported)| *module == source && *exported == name)
                }) {
                    self.needed[index].get_or_insert(false);
                    self.renamed
                        .insert(local, world.resources[index].class.clone());
                    return false;
                }
                let function = world.imports.iter().find(|function| {
                    function.role == Role::Function
                        && function.module == source
                        && function.binding == name
                        && (function.params.iter().any(|ty| ty.shape != Shape::Plain)
                            || function
                                .result
                                .as_ref()
                                .is_some_and(|ty| ty.shape != Shape::Plain))
                });
                match function {
                    Some(function) => {
                        functions.push((local, function));
                        false
                    }
                    None => true,
                }
            });
        }
        module.body.retain(|item| {
            !matches!(
                item,
                ast::ModuleItem::ModuleDecl(ast::ModuleDecl::Import(import))
                    if import.specifiers.is_empty()
                        && world.imports.iter().any(|function| {
                            function.module == import.src.value.to_string_lossy()
                        })
            )
        });
        for (local, function) in functions {
            if let Some((params, result, body)) = self.wrapper(function, None) {
                let _ = writeln!(
                    self.code,
                    "{}function {local}({params}): {result} {{ {body} }}",
                    Self::keyword(function)
                );
            }
        }
    }

    /// An exported function that converts its arguments, calls the source's `inner`
    /// implementation and converts the result back.
    pub(super) fn export(
        &mut self,
        export: &super::ExportedFunction,
        inner: &str,
        params: usize,
        is_async: bool,
    ) {
        let mut declared = Vec::new();
        let mut arguments = Vec::new();
        for (index, param) in export.params.iter().enumerate() {
            declared.push(format!("p{index}: {}", param.raw));
            if index < params {
                arguments.push(self.converted(param, true, &format!("p{index}")));
            }
        }
        let wait = if is_async { "await " } else { "" };
        let keyword = if is_async { "async " } else { "" };
        let call = format!("{wait}{inner}({})", arguments.join(", "));
        // A `result` export's implementation returns the payload or throws the error.
        let converts = match export.result.as_ref() {
            Some(Typed {
                shape: Shape::Result { ok, .. },
                ..
            }) => ok.as_deref(),
            other => other,
        };
        let body = match converts {
            Some(ty) => format!("return {};", self.converted(ty, false, &call)),
            None => format!("{call};"),
        };
        let result = match &export.result {
            _ if export.open => String::new(),
            None if is_async => ": Promise<void>".to_owned(),
            None => ": void".to_owned(),
            Some(ty) if is_async => format!(": Promise<{}>", ty.raw),
            Some(ty) => format!(": {}", ty.raw),
        };
        let _ = writeln!(
            self.code,
            "export {keyword}function {}({}){result} {{ {body} }}",
            export.implementation,
            declared.join(", ")
        );
    }

    /// Generate the class of every needed resource. Generating one may need others.
    fn classes(&mut self) -> String {
        let world = self.world;
        let mut classes = String::new();
        while let Some(index) = self.needed.iter().position(|state| *state == Some(false)) {
            self.needed[index] = Some(true);
            let resource = &world.resources[index];
            let class = &resource.class;
            let mut body = format!(
                "class {class} {{\n  __h: {handle} | null = null;\n  constructor(__h: {handle}) {{ this.__h = __h; }}\n",
                handle = resource.handle
            );
            let mut constructor = String::new();
            for function in &world.imports {
                match function.role {
                    Role::Method(owner) if owner == index && (self.used)(&function.member) => {
                        if let Some((params, result, code)) =
                            self.wrapper(function, Some("this.__h!"))
                        {
                            let _ = writeln!(
                                body,
                                "  {}{}({params}): {result} {{ {code} }}",
                                Self::keyword(function),
                                function.member
                            );
                        }
                    }
                    Role::Static(owner) if owner == index && (self.used)(&function.member) => {
                        if let Some((params, result, code)) = self.wrapper(function, None) {
                            let _ = writeln!(
                                body,
                                "  static {}{}({params}): {result} {{ {code} }}",
                                Self::keyword(function),
                                function.member
                            );
                        }
                    }
                    Role::Constructor(owner) if owner == index => {
                        if let Some(raw) = self.raw(&function.module, &function.binding) {
                            let mut params = Vec::new();
                            let mut arguments = Vec::new();
                            for (index, param) in function.params.iter().enumerate() {
                                params.push(format!("p{index}: {}", self.source_type(param)));
                                arguments.push(self.converted(param, false, &format!("p{index}")));
                            }
                            constructor = format!(
                                "function __jco_new{class}({}): {class} {{ return new {class}({raw}({})); }}\n",
                                params.join(", "),
                                arguments.join(", ")
                            );
                        }
                    }
                    _ => {}
                }
            }
            if let Some(drop) = self.raw(&resource.module, &resource.drop) {
                let _ = writeln!(body, "  {DISPOSE}(): void {{ {drop}(this.__h!); }}");
            }
            body.push_str("}\n");
            classes.push_str(&body);
            classes.push_str(&constructor);
        }
        classes
    }

    /// Rewrite the source's uses of resources and append the generated declarations.
    pub(super) fn finish(mut self, module: &mut ast::Module) -> Result<()> {
        let classes = self.classes();
        let mut constructors = HashMap::new();
        for (index, resource) in self.world.resources.iter().enumerate() {
            if self.needed[index].is_some() {
                constructors.insert(
                    resource.class.clone(),
                    format!("__jco_new{}", resource.class),
                );
            }
        }
        module.visit_mut_with(&mut Uses {
            renamed: &self.renamed,
            constructors: &constructors,
        });
        let mut imports = String::new();
        let mut raw: Vec<_> = self.raw.iter().collect();
        raw.sort_by(|a, b| a.1.cmp(b.1));
        for ((module, binding), alias) in raw {
            let _ = writeln!(
                imports,
                "import {{ {binding} as {alias} }} from {};",
                serde_json::Value::String(module.clone())
            );
        }
        let code = format!("{imports}{}{classes}", self.code);
        if code.is_empty() {
            return Ok(());
        }
        let mut generated = crate::hir_compat::parse(&code, "guest-conventions.ts")
            .context("generating resource classes")?;
        generated.visit_mut_with(&mut ClearSpans);
        module.body.extend(generated.body);
        Ok(())
    }
}

fn is_symbol_dispose(expression: &ast::Expr) -> bool {
    matches!(
        expression,
        ast::Expr::Member(member)
            if matches!(member.obj.as_ref(), ast::Expr::Ident(object) if object.sym == "Symbol")
                && matches!(&member.prop, ast::MemberProp::Ident(name) if name.sym == "dispose")
    )
}

/// Rewrite resources named in the source: their type and value names, `new`, explicit
/// `[Symbol.dispose]()` calls and `using` declarations.
struct Uses<'a> {
    renamed: &'a HashMap<String, String>,
    constructors: &'a HashMap<String, String>,
}
impl VisitMut for Uses<'_> {
    fn visit_mut_ident(&mut self, id: &mut ast::Ident) {
        if let Some(class) = self.renamed.get(id.sym.as_ref()) {
            id.sym = class.as_str().into();
        }
    }

    fn visit_mut_prop_name(&mut self, name: &mut ast::PropName) {
        name.visit_mut_children_with(self);
        if let ast::PropName::Computed(computed) = name
            && is_symbol_dispose(&computed.expr)
        {
            *name = ast::PropName::Ident(ast::IdentName::new(DISPOSE.into(), computed.span));
        }
    }

    fn visit_mut_member_prop(&mut self, property: &mut ast::MemberProp) {
        property.visit_mut_children_with(self);
        if let ast::MemberProp::Computed(computed) = property
            && is_symbol_dispose(&computed.expr)
        {
            *property = ast::MemberProp::Ident(ast::IdentName::new(DISPOSE.into(), computed.span));
        }
    }

    fn visit_mut_expr(&mut self, expression: &mut ast::Expr) {
        expression.visit_mut_children_with(self);
        if let ast::Expr::New(new) = expression
            && let ast::Expr::Ident(class) = new.callee.as_ref()
            && let Some(constructor) = self.constructors.get(class.sym.as_ref())
        {
            *expression = ast::Expr::Call(ast::CallExpr {
                span: new.span,
                ctxt: SyntaxContext::empty(),
                callee: ast::Callee::Expr(Box::new(ast::Expr::Ident(ast::Ident::new(
                    constructor.as_str().into(),
                    class.span,
                    SyntaxContext::empty(),
                )))),
                args: new.args.take().unwrap_or_default(),
                type_args: None,
            });
        }
    }

    /// `using a = e; rest` becomes `const a = e; try { rest } finally { a.__dispose(); }`.
    fn visit_mut_stmts(&mut self, statements: &mut Vec<ast::Stmt>) {
        let position = statements
            .iter()
            .position(|statement| matches!(statement, ast::Stmt::Decl(ast::Decl::Using(_))));
        if let Some(position) = position {
            let rest = statements.split_off(position + 1);
            let Some(ast::Stmt::Decl(ast::Decl::Using(using))) = statements.pop() else {
                unreachable!("matched a using declaration above");
            };
            let span = using.span;
            let disposals = using
                .decls
                .iter()
                .rev()
                .filter_map(|declaration| match &declaration.name {
                    ast::Pat::Ident(binding) => Some(ast::Stmt::Expr(ast::ExprStmt {
                        span,
                        expr: Box::new(ast::Expr::Call(ast::CallExpr {
                            span,
                            ctxt: SyntaxContext::empty(),
                            callee: ast::Callee::Expr(Box::new(ast::Expr::Member(
                                ast::MemberExpr {
                                    span,
                                    obj: Box::new(ast::Expr::Ident(binding.id.clone())),
                                    prop: ast::MemberProp::Ident(ast::IdentName::new(
                                        DISPOSE.into(),
                                        DUMMY_SP,
                                    )),
                                },
                            ))),
                            args: vec![],
                            type_args: None,
                        })),
                    })),
                    _ => None,
                })
                .collect();
            statements.push(ast::Stmt::Decl(ast::Decl::Var(Box::new(ast::VarDecl {
                span,
                ctxt: SyntaxContext::empty(),
                kind: ast::VarDeclKind::Const,
                declare: false,
                decls: using.decls,
            }))));
            statements.push(ast::Stmt::Try(Box::new(ast::TryStmt {
                span,
                block: ast::BlockStmt {
                    span,
                    ctxt: SyntaxContext::empty(),
                    stmts: rest,
                },
                handler: None,
                finalizer: Some(ast::BlockStmt {
                    span,
                    ctxt: SyntaxContext::empty(),
                    stmts: disposals,
                }),
            })));
        }
        statements.visit_mut_children_with(self);
    }
}
