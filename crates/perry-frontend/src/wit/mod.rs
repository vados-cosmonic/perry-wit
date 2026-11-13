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
            exports: world.map_or_else(Vec::new, |world| exported_functions(resolve, world)),
        }
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

/// The export object of an interface is named after the interface, not its package.
fn exported_functions(resolve: &Resolve, world: WorldId) -> Vec<ExportedFunction> {
    let world = &resolve.worlds[world];
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
            }
        })
        .collect()
}

fn source_type(resolve: &Resolve, id: TypeId) -> Option<HirType> {
    Some(HirType::Named(
        NamedType::get(resolve, canonical_type(resolve, id))?.qualified(),
    ))
}
