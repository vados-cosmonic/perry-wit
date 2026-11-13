//! The selected world as guest conventions name it: export objects, imported functions and
//! the resources they pass, with every type in its TypeScript spelling.
use crate::conventions::{
    ExportedFunction, ImportedFunction, ImportedResource, Role, Shape, Thrown, Typed, World,
};
use crate::hir::Foreign;
use crate::sdk::codegen::construct_text;
use perry_contract::{
    names::{NamedType, canonical_type, to_camel_case, to_pascal_case},
    types::{Contract, Input, convert},
};
use std::collections::HashMap;
use wit_parser::{Resolve, Type, TypeDefKind, TypeId, WorldId, WorldItem, WorldKey};

struct Types<'a> {
    resolve: &'a Resolve,
    foreign: &'a HashMap<String, Foreign>,
    /// Index of each imported resource in `World::resources`, by canonical type.
    resources: HashMap<TypeId, usize>,
}

impl Types<'_> {
    /// Named WIT types are registered under their qualified SDK names, which need no import.
    fn name(&self, id: TypeId) -> Option<String> {
        NamedType::get(self.resolve, id)
            .map(|named| named.qualified())
            .filter(|name| self.foreign.contains_key(name))
    }

    /// Whether a value of `ty` differs between the compiler's representation and the one
    /// sources written against classes expect: it holds an imported resource or a `result`.
    fn converts(&self, ty: &Type) -> bool {
        let Type::Id(id) = ty else {
            return false;
        };
        match &self.resolve.types[*id].kind {
            TypeDefKind::Resource => self
                .resources
                .contains_key(&canonical_type(self.resolve, *id)),
            TypeDefKind::Handle(wit_parser::Handle::Own(id) | wit_parser::Handle::Borrow(id)) => {
                self.resources
                    .contains_key(&canonical_type(self.resolve, *id))
            }
            TypeDefKind::Result(_) => true,
            kind => {
                let mut found = false;
                perry_contract::names::visit_type_children(kind, |ty| {
                    found = found || self.converts(ty);
                });
                found
            }
        }
    }

    fn typed(&self, ty: &Type) -> Typed {
        let name = |id| self.name(id);
        let plain = |raw: String| Typed {
            raw,
            shape: Shape::Plain,
            thrown: Thrown::Opaque,
        };
        let thrown = |ty: Typed, thrown| Typed { thrown, ..ty };
        convert(
            self.resolve,
            Input::Reference(*ty),
            // A named type that needs no conversion keeps its name.
            &|id| {
                (!self.converts(&Type::Id(id)))
                    .then(|| self.name(id))
                    .flatten()
                    .map(|name| thrown(plain(name), self.thrown(id)))
            },
            &|contract: Contract<'_, Typed>| match contract {
                Contract::Resource { id, owned } => {
                    let canonical = canonical_type(self.resolve, id);
                    match self.resources.get(&canonical) {
                        Some(index) => Typed {
                            raw: self.name(canonical).unwrap_or_else(|| "unknown".into()),
                            shape: Shape::Resource(*index),
                            thrown: Thrown::Opaque,
                        },
                        None => plain(construct_text(Contract::Resource { id, owned }, &name)),
                    }
                }
                Contract::Scalar(ty) => thrown(
                    plain(construct_text(Contract::Scalar(ty), &name)),
                    Thrown::Value,
                ),
                Contract::Bytes => plain(construct_text(Contract::Bytes, &name)),
                Contract::ByteStream => plain(construct_text(Contract::ByteStream, &name)),
                Contract::Enum(cases) => thrown(
                    plain(construct_text(Contract::Enum(cases), &name)),
                    Thrown::Value,
                ),
                Contract::Unsupported(kind) => {
                    plain(construct_text(Contract::Unsupported(kind), &name))
                }
                Contract::List(item) => {
                    let raw = construct_text(Contract::List(item.raw.clone()), &name);
                    if item.shape == Shape::Plain {
                        plain(raw)
                    } else {
                        Typed {
                            raw,
                            shape: Shape::List(Box::new(item)),
                            thrown: Thrown::Opaque,
                        }
                    }
                }
                Contract::Option(item) => {
                    let raw = construct_text(Contract::Option(item.raw.clone()), &name);
                    if item.shape == Shape::Plain {
                        plain(raw)
                    } else {
                        Typed {
                            raw,
                            shape: Shape::Option(Box::new(item)),
                            thrown: Thrown::Opaque,
                        }
                    }
                }
                Contract::Result { ok, err } => Typed {
                    raw: construct_text(
                        Contract::Result {
                            ok: ok.as_ref().map(|ty| ty.raw.clone()),
                            err: err.as_ref().map(|ty| ty.raw.clone()),
                        },
                        &name,
                    ),
                    shape: Shape::Result {
                        ok: ok.map(Box::new),
                        err: err.map(Box::new),
                    },
                    thrown: Thrown::Opaque,
                },
                Contract::Tuple(items) => {
                    let items: Vec<Typed> = items.collect();
                    let raw = construct_text(
                        Contract::Tuple(&mut items.iter().map(|item| item.raw.clone())),
                        &name,
                    );
                    if items.iter().all(|item| item.shape == Shape::Plain) {
                        plain(raw)
                    } else {
                        Typed {
                            raw,
                            shape: Shape::Tuple(items),
                            thrown: Thrown::Opaque,
                        }
                    }
                }
                Contract::Record(fields) => {
                    let fields: Vec<(String, Typed)> = fields.collect();
                    let raw = construct_text(
                        Contract::Record(
                            &mut fields
                                .iter()
                                .map(|(name, field)| (name.clone(), field.raw.clone())),
                        ),
                        &name,
                    );
                    if fields.iter().all(|(_, field)| field.shape == Shape::Plain) {
                        plain(raw)
                    } else {
                        Typed {
                            raw,
                            shape: Shape::Record(fields),
                            thrown: Thrown::Opaque,
                        }
                    }
                }
                Contract::Variant(cases) => {
                    let cases: Vec<(String, Option<Typed>)> = cases
                        .map(|(name, payload)| (name.to_owned(), payload))
                        .collect();
                    let raw = construct_text(
                        Contract::Variant(&mut cases.iter().map(|(name, payload)| {
                            (name.as_str(), payload.as_ref().map(|ty| ty.raw.clone()))
                        })),
                        &name,
                    );
                    if cases.iter().all(|(_, payload)| {
                        payload.as_ref().is_none_or(|ty| ty.shape == Shape::Plain)
                    }) {
                        thrown(plain(raw), Thrown::Tag)
                    } else {
                        Typed {
                            raw,
                            shape: Shape::Variant(cases),
                            thrown: Thrown::Tag,
                        }
                    }
                }
            },
        )
    }

    /// How a value of the named type `id` is thrown.
    fn thrown(&self, mut id: TypeId) -> Thrown {
        loop {
            match &self.resolve.types[id].kind {
                TypeDefKind::Type(Type::Id(inner)) => id = *inner,
                TypeDefKind::Type(_) | TypeDefKind::Enum(_) => return Thrown::Value,
                TypeDefKind::Variant(_) => return Thrown::Tag,
                _ => return Thrown::Opaque,
            }
        }
    }

    /// Whether the TypeScript spelling of a WIT result is the source's choice.
    fn open(&self, mut ty: Type) -> bool {
        while let Type::Id(id) = ty {
            match &self.resolve.types[id].kind {
                TypeDefKind::Type(inner) => ty = *inner,
                TypeDefKind::Result(_) | TypeDefKind::Option(_) => return true,
                _ => break,
            }
        }
        false
    }
}

/// The name of an interface's export object or of the module a source imports it from.
fn interface_name<'a>(
    resolve: &'a Resolve,
    key: &'a WorldKey,
    id: wit_parser::InterfaceId,
) -> &'a str {
    match key {
        WorldKey::Name(name) => name,
        WorldKey::Interface(_) => resolve.interfaces[id]
            .name
            .as_deref()
            .unwrap_or("interface"),
    }
}

pub(super) fn world(
    resolve: &Resolve,
    world: WorldId,
    foreign: &HashMap<String, Foreign>,
) -> World {
    let world = &resolve.worlds[world];
    let mut types = Types {
        resolve,
        foreign,
        resources: HashMap::new(),
    };

    // Every resource an imported interface declares, then every name it is imported under:
    // an interface that `use`s a resource exports it too.
    let mut resources = Vec::new();
    for (key, item) in &world.imports {
        let WorldItem::Interface { id, .. } = item else {
            continue;
        };
        let module = resolve.name_world_key(key);
        for (name, id) in &resolve.interfaces[*id].types {
            if !matches!(resolve.types[*id].kind, TypeDefKind::Resource) {
                continue;
            }
            let Some(handle) = types.name(*id) else {
                continue;
            };
            types.resources.insert(*id, resources.len());
            resources.push(ImportedResource {
                class: format!("__jco_{handle}"),
                handle,
                module: module.clone(),
                drop: format!("drop{}", to_pascal_case(name)),
                names: Vec::new(),
            });
        }
    }
    for (key, item) in &world.imports {
        let WorldItem::Interface { id, .. } = item else {
            continue;
        };
        let module = resolve.name_world_key(key);
        for (name, id) in &resolve.interfaces[*id].types {
            if let Some(index) = types.resources.get(&canonical_type(resolve, *id)) {
                resources[*index]
                    .names
                    .push((module.clone(), to_pascal_case(name)));
            }
        }
    }

    let mut imports = Vec::new();
    for (key, item) in &world.imports {
        let WorldItem::Interface { id, .. } = item else {
            continue;
        };
        let module = resolve.name_world_key(key);
        for function in resolve.interfaces[*id].functions.values() {
            let resource = function
                .kind
                .resource()
                .and_then(|id| types.resources.get(&canonical_type(resolve, id)).copied());
            let member = to_camel_case(function.item_name());
            let role = match (resource, function.name.split(']').next()) {
                (None, _) if function.kind.resource().is_none() => Role::Function,
                (Some(resource), Some("[constructor")) => Role::Constructor(resource),
                (Some(resource), Some("[method")) => Role::Method(resource),
                (Some(resource), Some("[static")) => Role::Static(resource),
                _ => continue,
            };
            // A method's receiver is its first WIT parameter.
            let skip = usize::from(matches!(role, Role::Method(_)));
            imports.push(ImportedFunction {
                module: module.clone(),
                binding: to_camel_case(&function.name),
                member,
                role,
                params: function
                    .params
                    .iter()
                    .skip(skip)
                    .map(|param| types.typed(&param.ty))
                    .collect(),
                result: function.result.as_ref().map(|ty| types.typed(ty)),
                is_async: function.kind.is_async(),
            });
        }
    }

    let exports = perry_contract::world_functions(resolve, world.exports.iter())
        .map(|entry| ExportedFunction {
            object: entry
                .interface
                .map(|id| to_camel_case(interface_name(resolve, entry.key, id))),
            member: to_camel_case(&entry.function.name),
            implementation: entry.implementation_name(resolve, world),
            params: entry
                .function
                .params
                .iter()
                .map(|param| types.typed(&param.ty))
                .collect(),
            result: entry.function.result.as_ref().map(|ty| types.typed(ty)),
            // A `result` export may return its payload or a tagged result, and an `option`
            // export `null` or `undefined`: the source decides.
            open: entry.function.result.is_some_and(|ty| types.open(ty)),
        })
        .collect();

    World {
        exports,
        imports,
        resources,
    }
}
