//! Exported WIT resources as classes.
//!
//! The compiler binds an exported resource's constructor, methods and static functions to
//! exported functions that take and return opaque handles, and has the source keep its own
//! table from a handle's representation to its state. Sources written for
//! `jco componentize` name a class in the interface's export object instead:
//!
//! ```ts
//! class Counter {
//!     constructor(start) { this.value = start; }
//!     get() { return this.value; }
//! }
//! export const counters = { Counter };
//! ```
//!
//! This module types such a class from the resource's WIT functions and generates, in
//! TypeScript, the table and the exported functions around it. Each handle has its own
//! representation: dropping the handle removes the instance from the table and calls its
//! `[Symbol.dispose]` method, when it has one.
use super::classes::Classes;
use super::{ExportedFunction, Role, Source, annotate, annotate_params, annotations};
use anyhow::Result;
use perry_parser::swc_ecma_ast as ast;
use std::collections::HashSet;
use std::fmt::Write as _;
use swc_common::{DUMMY_SP, Span};
use swc_ecma_visit::{VisitMut, VisitMutWith};

/// The field a private name becomes: the compiler has no private fields.
fn private(name: &ast::PrivateName) -> ast::IdentName {
    ast::IdentName::new(format!("__private_{}", name.name).into(), name.span)
}

/// Rewrite `this.#field` and `#method()` into ordinary members.
struct Private;
impl VisitMut for Private {
    fn visit_mut_member_prop(&mut self, property: &mut ast::MemberProp) {
        property.visit_mut_children_with(self);
        if let ast::MemberProp::PrivateName(name) = property {
            *property = ast::MemberProp::Ident(private(name));
        }
    }

    fn visit_mut_class_member(&mut self, member: &mut ast::ClassMember) {
        member.visit_mut_children_with(self);
        match member {
            ast::ClassMember::PrivateProp(property) => {
                *member = ast::ClassMember::ClassProp(ast::ClassProp {
                    span: property.span,
                    key: ast::PropName::Ident(private(&property.key)),
                    value: property.value.take(),
                    type_ann: property.type_ann.take(),
                    is_static: property.is_static,
                    decorators: std::mem::take(&mut property.decorators),
                    accessibility: None,
                    is_abstract: false,
                    is_optional: property.is_optional,
                    is_override: property.is_override,
                    readonly: property.readonly,
                    declare: false,
                    definite: property.definite,
                });
            }
            ast::ClassMember::PrivateMethod(method) => {
                *member = ast::ClassMember::Method(ast::ClassMethod {
                    span: method.span,
                    key: ast::PropName::Ident(private(&method.key)),
                    function: method.function.clone(),
                    kind: method.kind,
                    is_static: method.is_static,
                    accessibility: None,
                    is_abstract: method.is_abstract,
                    is_optional: method.is_optional,
                    is_override: method.is_override,
                });
            }
            _ => {}
        }
    }
}

fn key_name(key: &ast::PropName) -> Option<&str> {
    match key {
        ast::PropName::Ident(name) => Some(name.sym.as_ref()),
        ast::PropName::Str(name) => name.value.as_str(),
        _ => None,
    }
}

fn is_dispose(key: &ast::PropName) -> bool {
    match key {
        ast::PropName::Computed(computed) => matches!(
            computed.expr.as_ref(),
            ast::Expr::Member(member)
                if matches!(member.obj.as_ref(), ast::Expr::Ident(object) if object.sym == "Symbol")
                    && matches!(&member.prop, ast::MemberProp::Ident(name) if name.sym == "dispose")
        ),
        other => key_name(other) == Some("__dispose"),
    }
}

fn method<'a>(
    class: &'a mut ast::Class,
    name: &str,
    is_static: bool,
) -> Option<&'a mut ast::ClassMethod> {
    class.body.iter_mut().find_map(|member| match member {
        ast::ClassMember::Method(method)
            if method.is_static == is_static
                && method.kind == ast::MethodKind::Method
                && key_name(&method.key) == Some(name) =>
        {
            Some(method)
        }
        _ => None,
    })
}

fn constructor(class: &mut ast::Class) -> Option<&mut ast::Constructor> {
    class.body.iter_mut().find_map(|member| match member {
        ast::ClassMember::Constructor(constructor) => Some(constructor),
        _ => None,
    })
}

fn keyword(name: &str) -> Box<ast::TsTypeAnn> {
    let kind = match name {
        "string" => ast::TsKeywordTypeKind::TsStringKeyword,
        "number" => ast::TsKeywordTypeKind::TsNumberKeyword,
        _ => ast::TsKeywordTypeKind::TsBooleanKeyword,
    };
    Box::new(ast::TsTypeAnn {
        span: DUMMY_SP,
        type_ann: Box::new(ast::TsType::TsKeywordType(ast::TsKeywordType {
            span: DUMMY_SP,
            kind,
        })),
    })
}

/// Declare the fields a constructor assigns from its parameters or from literals: the
/// compiler needs every field declared with its type.
fn declare_fields(class: &mut ast::Class) {
    let mut assigned: Vec<(String, Span, Box<ast::TsTypeAnn>)> = Vec::new();
    if let Some(constructor) = constructor(class) {
        let params: Vec<(String, Box<ast::TsTypeAnn>)> = constructor
            .params
            .iter()
            .filter_map(|param| match param {
                ast::ParamOrTsParamProp::Param(ast::Param {
                    pat: ast::Pat::Ident(binding),
                    ..
                }) => Some((binding.id.sym.to_string(), binding.type_ann.clone()?)),
                _ => None,
            })
            .collect();
        for statement in constructor.body.iter().flat_map(|body| &body.stmts) {
            let ast::Stmt::Expr(statement) = statement else {
                continue;
            };
            let ast::Expr::Assign(assign) = statement.expr.as_ref() else {
                continue;
            };
            let ast::AssignTarget::Simple(ast::SimpleAssignTarget::Member(target)) = &assign.left
            else {
                continue;
            };
            let (ast::Expr::This(_), ast::MemberProp::Ident(field)) =
                (target.obj.as_ref(), &target.prop)
            else {
                continue;
            };
            let ty = match assign.right.as_ref() {
                ast::Expr::Ident(value) => params
                    .iter()
                    .find(|(name, _)| value.sym == *name.as_str())
                    .map(|(_, ty)| ty.clone()),
                ast::Expr::Lit(ast::Lit::Str(_)) | ast::Expr::Tpl(_) => Some(keyword("string")),
                ast::Expr::Lit(ast::Lit::Num(_)) => Some(keyword("number")),
                ast::Expr::Lit(ast::Lit::Bool(_)) => Some(keyword("boolean")),
                _ => None,
            };
            if let Some(ty) = ty
                && assign.op == ast::AssignOp::Assign
                && !assigned
                    .iter()
                    .any(|(name, ..)| field.sym == *name.as_str())
            {
                assigned.push((field.sym.to_string(), field.span, ty));
            }
        }
    }
    let mut declared = HashSet::new();
    for member in &mut class.body {
        if let ast::ClassMember::ClassProp(property) = member
            && let Some(name) = key_name(&property.key).map(str::to_owned)
        {
            if property.type_ann.is_none()
                && property.value.is_none()
                && let Some((.., ty)) = assigned.iter().find(|(field, ..)| *field == name)
            {
                property.type_ann = Some(ty.clone());
            }
            declared.insert(name);
        }
    }
    let fields: Vec<_> = assigned
        .into_iter()
        .filter(|(name, ..)| !declared.contains(name))
        .map(|(name, span, ty)| {
            ast::ClassMember::ClassProp(ast::ClassProp {
                span,
                key: ast::PropName::Ident(ast::IdentName::new(name.into(), span)),
                value: None,
                type_ann: Some(ty),
                is_static: false,
                decorators: vec![],
                accessibility: None,
                is_abstract: false,
                is_optional: false,
                is_override: false,
                readonly: false,
                declare: false,
                definite: false,
            })
        })
        .collect();
    class.body.splice(0..0, fields);
}

fn class_of<'a>(module: &'a mut ast::Module, name: &str) -> Option<&'a mut ast::ClassDecl> {
    module.body.iter_mut().find_map(|item| match item {
        ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Class(class)))
        | ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDecl(ast::ExportDecl {
            decl: ast::Decl::Class(class),
            ..
        })) if class.ident.sym == *name => Some(class),
        _ => None,
    })
}

impl Classes<'_> {
    /// Bind each class the source implements an exported resource with: `implemented` pairs
    /// a resource of the world with the name of its class.
    pub(super) fn bind_exported(
        &mut self,
        module: &mut ast::Module,
        source: &Source<'_>,
        implemented: &[(usize, String)],
    ) -> Result<()> {
        let world = self.world;
        // Every class is known before any is typed: one resource's functions pass others.
        for (resource, class) in implemented {
            self.implemented[*resource] = Some(class.clone());
        }
        // The resource's own name, imported as a type, names the class.
        for (resource, class) in implemented {
            let exported = &world.exported[*resource];
            for item in &mut module.body {
                let ast::ModuleItem::ModuleDecl(ast::ModuleDecl::Import(import)) = item else {
                    continue;
                };
                if import.src.value.to_string_lossy() != exported.module.as_str() {
                    continue;
                }
                import.specifiers.retain(|specifier| {
                    let ast::ImportSpecifier::Named(named) = specifier else {
                        return true;
                    };
                    let name = named.imported.as_ref().map_or_else(
                        || named.local.sym.to_string(),
                        |name| name.atom().to_string(),
                    );
                    if name != exported.name {
                        return true;
                    }
                    let local = named.local.sym.to_string();
                    if local != *class {
                        self.renamed.insert(local, class.clone());
                    }
                    false
                });
            }
        }
        let modules: HashSet<&str> = implemented
            .iter()
            .map(|(resource, _)| world.exported[*resource].module.as_str())
            .collect();
        module.body.retain(|item| {
            !matches!(
                item,
                ast::ModuleItem::ModuleDecl(ast::ModuleDecl::Import(import))
                    if import.specifiers.is_empty()
                        && modules.contains(import.src.value.to_string_lossy().as_ref())
            )
        });

        for (resource, class) in implemented {
            self.bind_class(module, source, *resource, class)?;
        }
        Ok(())
    }

    fn bind_class(
        &mut self,
        module: &mut ast::Module,
        source: &Source<'_>,
        resource: usize,
        name: &str,
    ) -> Result<()> {
        let world = self.world;
        let exported = &world.exported[resource];
        let Some(declaration) = class_of(module, name) else {
            return Err(source.error(
                DUMMY_SP,
                format!(
                    "Component export '{}.{}' is a resource: '{name}' needs to be a class declared in the entry module",
                    exported.object, exported.name
                ),
            ));
        };
        let span = declaration.ident.span;
        let class = declaration.class.as_mut();
        class.implements.clear();
        class.visit_mut_with(&mut Private);

        let helpers = (
            self.raw(&exported.module, &exported.create),
            self.raw(&exported.module, &exported.representation),
        );
        let (Some(create), Some(representation)) = helpers else {
            return Ok(());
        };
        let table = format!("__jco_tbl{resource}");
        let mut code = format!(
            "const {table} = new Map<number, {name}>();\nlet __jco_next{resource} = 0;\nfunction __jco_reg{resource}(v: {name}): number {{ __jco_next{resource} = __jco_next{resource} + 1; {table}.set(__jco_next{resource}, v); return __jco_next{resource}; }}\n"
        );

        // Type the class's members from the resource's WIT functions, then generate the
        // exported function of each.
        let functions: Vec<&ExportedFunction> = world
            .exports
            .iter()
            .filter(|export| {
                matches!(
                    export.role,
                    Role::Constructor(owner) | Role::Method(owner) | Role::Static(owner)
                        if owner == resource
                )
            })
            .collect();
        for export in functions {
            let params: Vec<String> = export
                .params
                .iter()
                .map(|ty| self.source_type(ty))
                .collect();
            if matches!(export.role, Role::Constructor(_)) {
                let mut declared = 0;
                if let Some(constructor) = constructor(class) {
                    declared = constructor.params.len();
                    if declared > params.len() {
                        return Err(source.error(
                            constructor.span,
                            format!(
                                "The constructor of '{name}' declares {declared} parameters, but its WIT constructor takes {}",
                                params.len()
                            ),
                        ));
                    }
                    if let Some(types) = annotations(&params[..declared]) {
                        annotate_params(
                            constructor
                                .params
                                .iter_mut()
                                .filter_map(|param| match param {
                                    ast::ParamOrTsParamProp::Param(param) => Some(&mut param.pat),
                                    ast::ParamOrTsParamProp::TsParamProp(_) => None,
                                }),
                            types,
                        );
                    }
                }
                let mut parameters = Vec::new();
                let mut arguments = Vec::new();
                for (index, param) in export.params.iter().enumerate() {
                    parameters.push(format!("p{index}: {}", param.raw));
                    if index < declared {
                        arguments.push(self.converted(param, true, &format!("p{index}")));
                    }
                }
                let _ = writeln!(
                    code,
                    "export function {}({}): {} {{ return {create}(__jco_reg{resource}(new {name}({}))); }}",
                    export.implementation,
                    parameters.join(", "),
                    exported.handle,
                    arguments.join(", ")
                );
                continue;
            }
            let is_static = matches!(export.role, Role::Static(_));
            let Some(implementation) = method(class, &export.member, is_static) else {
                return Err(source.error(
                    span,
                    format!(
                        "Class '{name}' needs a {}method '{}' to implement the resource '{}'",
                        if is_static { "static " } else { "" },
                        export.member,
                        exported.name
                    ),
                ));
            };
            let returned = export.result.as_ref().map(|ty| self.source_type(ty));
            let result = (!export.open).then(|| returned.unwrap_or_else(|| "void".to_owned()));
            annotate(
                source,
                implementation.span,
                &mut implementation.function,
                export,
                &params,
                result,
            )?;
            let declared = implementation.function.params.len();
            let is_async = implementation.function.is_async;
            if is_static {
                let inner = format!("{name}.{}", export.member);
                self.export(export, None, &inner, declared, is_async);
            } else {
                let inner = format!("{table}.get({representation}(self))!.{}", export.member);
                self.export(export, Some(&exported.handle), &inner, declared, is_async);
            }
        }
        declare_fields(class);

        // Dropping a handle forgets its instance, after disposing it.
        let dispose = class.body.iter().any(
            |member| matches!(member, ast::ClassMember::Method(method) if is_dispose(&method.key)),
        );
        let dispose = if dispose { " v.__dispose();" } else { "" };
        let _ = writeln!(
            code,
            "export function {}(rep: number): void {{ if ({table}.has(rep)) {{ const v = {table}.get(rep)!; {table}.delete(rep);{dispose} }} }}",
            exported.destructor
        );
        self.code.push_str(&code);
        Ok(())
    }
}
