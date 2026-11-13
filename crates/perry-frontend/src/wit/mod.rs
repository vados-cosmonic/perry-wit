//! Source types and ownership helpers for a resolved component contract.
mod types;

use crate::conventions::ExportedFunction;
use crate::hir::{Foreign, FunctionType, Type as HirType};
use anyhow::{Context, Result};
use perry_contract::{
    names::{NamedType, canonical_type, to_camel_case, to_pascal_case},
    types::Input,
};
use std::collections::HashMap;
use wit_parser::{Function, Resolve, TypeId, WorldId};

#[derive(Default)]
pub struct WorldTypes {
    foreign: HashMap<String, Foreign>,
    exports: Vec<ExportedFunction>,
}

pub enum ResourceOperation {
    Drop,
    New,
    Representation,
}

pub struct ResourceHelper {
    pub binding: String,
    pub name: String,
    pub operation: ResourceOperation,
}

impl WorldTypes {
    pub fn new(resolve: &Resolve, world: Option<WorldId>) -> Self {
        Self {
            foreign: types::foreign(resolve, world),
            exports: Vec::new(),
        }
        .with_exports(resolve, world)
    }

    fn with_exports(mut self, resolve: &Resolve, world: Option<WorldId>) -> Self {
        if let Some(world) = world {
            self.exports = exported_functions(resolve, world, &self.foreign);
        }
        self
    }

    /// The functions the selected world exports, with the names a source may give them.
    pub fn exports(&self) -> &[ExportedFunction] {
        &self.exports
    }

    pub fn foreign(&self) -> &HashMap<String, Foreign> {
        &self.foreign
    }

    pub fn import_function(
        &mut self,
        resolve: &Resolve,
        module: &str,
        function: &Function,
    ) -> Option<String> {
        let params = function
            .params
            .iter()
            .map(|parameter| {
                Some((
                    parameter.name.clone(),
                    HirType::Generic {
                        base: "WitInput".into(),
                        type_args: vec![types::ty(resolve, Input::Reference(parameter.ty))?],
                    },
                    false,
                ))
            })
            .collect::<Option<_>>()?;
        let result = match function.result {
            Some(ty) => types::ty(resolve, Input::Reference(ty))?,
            None => HirType::Void,
        };
        let binding = format!("{module}#{}", to_camel_case(&function.name));
        self.foreign.insert(
            binding.clone(),
            Foreign::Function(FunctionType {
                params,
                return_type: Box::new(result),
                is_async: function.kind.is_async(),
                is_generator: false,
            }),
        );
        Some(binding)
    }

    pub fn resource_helpers(
        &mut self,
        resolve: &Resolve,
        id: TypeId,
        module: &str,
        name: &str,
        exported: bool,
    ) -> Result<Vec<ResourceHelper>> {
        let resource = source_type(resolve, id).context("resource has no WIT declaration name")?;
        let suffix = to_pascal_case(name);
        let mut helpers = vec![(
            format!("drop{suffix}"),
            ResourceOperation::Drop,
            resource.clone(),
            HirType::Void,
        )];
        if exported {
            helpers.extend([
                (
                    format!("new{suffix}"),
                    ResourceOperation::New,
                    HirType::Number,
                    resource.clone(),
                ),
                (
                    format!("{}Rep", to_camel_case(name)),
                    ResourceOperation::Representation,
                    resource,
                    HirType::Number,
                ),
            ]);
        }
        Ok(helpers
            .into_iter()
            .map(|(name, operation, input, output)| {
                let binding = format!("{module}#{name}");
                self.foreign.insert(
                    binding.clone(),
                    Foreign::Function(FunctionType {
                        params: vec![("value".into(), input, false)],
                        return_type: Box::new(output),
                        is_async: false,
                        is_generator: false,
                    }),
                );
                ResourceHelper {
                    binding,
                    name,
                    operation,
                }
            })
            .collect())
    }
}

/// Whether a WIT result type has more than one TypeScript spelling.
fn open_result(resolve: &Resolve, mut ty: wit_parser::Type) -> bool {
    while let wit_parser::Type::Id(id) = ty {
        match &resolve.types[id].kind {
            wit_parser::TypeDefKind::Type(inner) => ty = *inner,
            wit_parser::TypeDefKind::Result(_) | wit_parser::TypeDefKind::Option(_) => return true,
            _ => break,
        }
    }
    false
}

/// The export object of an interface is named after the interface, not its package.
fn exported_functions(
    resolve: &Resolve,
    world: WorldId,
    foreign: &HashMap<String, Foreign>,
) -> Vec<ExportedFunction> {
    let world = &resolve.worlds[world];
    // Named WIT types are registered under their qualified SDK names, which need no import.
    let name = |id| {
        NamedType::get(resolve, id)
            .map(|named| named.qualified())
            .filter(|name| foreign.contains_key(name))
    };
    let text = |ty: &wit_parser::Type| {
        crate::sdk::codegen::type_text(resolve, Input::Reference(*ty), &name)
    };
    perry_contract::world_functions(resolve, world.exports.iter())
        .map(|entry| {
            let object = entry.interface.map(|id| {
                to_camel_case(match entry.key {
                    wit_parser::WorldKey::Name(name) => name,
                    wit_parser::WorldKey::Interface(_) => resolve.interfaces[id]
                        .name
                        .as_deref()
                        .unwrap_or("interface"),
                })
            });
            ExportedFunction {
                object,
                member: to_camel_case(&entry.function.name),
                implementation: entry.implementation_name(resolve, world),
                params: entry
                    .function
                    .params
                    .iter()
                    .map(|param| text(&param.ty))
                    .collect(),
                // A `result` export may return its payload or a tagged result, and an
                // `option` export `null` or `undefined`: the source decides.
                result: match &entry.function.result {
                    None => Some("void".into()),
                    Some(ty) if open_result(resolve, *ty) => None,
                    Some(ty) => Some(text(ty)),
                },
            }
        })
        .collect()
}

fn source_type(resolve: &Resolve, id: TypeId) -> Option<HirType> {
    Some(HirType::Named(
        NamedType::get(resolve, canonical_type(resolve, id))?.qualified(),
    ))
}
