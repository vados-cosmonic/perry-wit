//! Sources written for other JavaScript component toolchains bind the same WIT exports.
use crate::world::{Compiler, instantiate};
use anyhow::{Context, Result};
use wasmtime::{
    Engine, Store,
    component::{ComponentNamedList, Instance, Lift, Lower},
};

const MATH: &str = "package test:conventions;
interface math { add: func(x: u32, y: u32) -> u32; twice: func(x: u32) -> u32; }
world test { export math; }";

/// Call `function` of the exported interface `test:conventions/<interface>`.
fn call<P, R>(
    store: &mut Store<()>,
    instance: &Instance,
    interface: &str,
    function: &str,
    params: P,
) -> Result<R>
where
    P: ComponentNamedList + Lower,
    R: ComponentNamedList + Lift,
{
    let (_, interface) = instance
        .get_export(&mut *store, None, &format!("test:conventions/{interface}"))
        .with_context(|| format!("no exported interface {interface}"))?;
    let (_, export) = instance
        .get_export(&mut *store, Some(&interface), function)
        .with_context(|| format!("no exported function {function}"))?;
    let function = instance.get_typed_func::<P, R>(&mut *store, export)?;
    let result = function.call(&mut *store, params)?;
    function.post_return(&mut *store)?;
    Ok(result)
}

fn math(source: &str) -> Result<(u32, u32)> {
    let engine = Engine::default();
    let bytes = Compiler::Standard.component(MATH, "test", "math.ts", source)?;
    let (mut store, instance) = instantiate(&engine, &bytes)?;
    let (sum,): (u32,) = call(&mut store, &instance, "math", "add", (2u32, 3u32))?;
    let (twice,): (u32,) = call(&mut store, &instance, "math", "twice", (4u32,))?;
    Ok((sum, twice))
}

#[test]
fn an_interface_is_implemented_by_an_export_object() -> Result<()> {
    for source in [
        // Methods, with `this` and the object's own name reaching sibling members.
        "export const math = {
           add(x: number, y: number): number { return x + y; },
           twice(x: number): number { return this.add(x, x) + math.add(0, 0); },
         };",
        // Arrow functions and function expressions.
        "export const math = {
           add: (x: number, y: number): number => x + y,
           twice: function (x: number): number { return x * 2; },
         };",
        // Function declarations named by shorthand or by value.
        "function add(x: number, y: number): number { return x + y; }
         function double(x: number): number { return add(x, x); }
         export const math = { add, twice: double };",
        // An object declared first and exported under the interface's name.
        "const implementation = {
           add(x: number, y: number): number { return x + y; },
           twice(x: number): number { return x * 2; },
         };
         export { implementation as math };",
        // A declared type and a property outside the world.
        "interface Math { add(x: number, y: number): number; twice(x: number): number }
         export const math: Math = {
           add(x: number, y: number): number { return x + y; },
           twice(x: number): number { return math.helper(x); },
           helper(x: number): number { return x * 2; },
         } as Math;",
        // The flat functions the object is rewritten to.
        "export function mathAdd(x: number, y: number): number { return x + y; }
         export function mathTwice(x: number): number { return x * 2; }",
    ] {
        assert_eq!(math(source)?, (5, 8), "{source}");
    }
    Ok(())
}

#[test]
fn a_function_export_may_be_a_function_valued_binding() -> Result<()> {
    let wit = "package test:conventions; world test { export run: func(x: u32) -> u32; }";
    for source in [
        "export const run = (x: number): number => x + 1;",
        "const run = function (x: number): number { return x + 1; };\nexport { run };",
    ] {
        let engine = Engine::default();
        let bytes = Compiler::Standard.component(wit, "test", "run.ts", source)?;
        let (mut store, instance) = instantiate(&engine, &bytes)?;
        let run = instance.get_typed_func::<(u32,), (u32,)>(&mut store, "run")?;
        assert_eq!(run.call(&mut store, (1,))?, (2,), "{source}");
    }
    Ok(())
}

#[test]
fn an_export_object_reports_members_that_are_not_functions() {
    for (source, expected) in [
        (
            "export const math = { add: 1, twice(x: number): number { return x * 2; } };",
            "Component export 'math.add' needs a function",
        ),
        (
            "export const math = { add(x: number, y: number): number { return x + y; } };",
            "mathTwice",
        ),
    ] {
        let error = Compiler::Standard
            .component(MATH, "test", "math.ts", source)
            .unwrap_err();
        assert!(format!("{error:#}").contains(expected), "{error:#}");
    }
}
