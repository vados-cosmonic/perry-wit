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

const LIBRARY: &str = "package test:conventions;
interface library {
  record book { title: string, pages: u32 }
  enum genre { fiction, fact }
  variant shape { circle(f64), square(f64), none }
  longest: func(books: list<book>) -> option<book>;
  describe: func(b: book, g: genre) -> string;
  area: func(s: shape) -> f64;
  parse: func(text: string) -> result<u32, string>;
  rest: func(data: list<u8>) -> list<u8>;
  swap: func(t: tuple<string, u32>) -> tuple<u32, string>;
}
world test { export library; }";

#[derive(wasmtime::component::ComponentType, Lift, Lower, Clone, Debug, PartialEq)]
#[component(record)]
struct Book {
    title: String,
    pages: u32,
}

#[derive(wasmtime::component::ComponentType, Lift, Lower, Clone, Copy, Debug, PartialEq)]
#[component(enum)]
#[repr(u8)]
enum Genre {
    #[component(name = "fiction")]
    Fiction,
    #[component(name = "fact")]
    Fact,
}

#[derive(wasmtime::component::ComponentType, Lift, Lower, Clone, Debug, PartialEq)]
#[component(variant)]
enum Shape {
    #[component(name = "circle")]
    Circle(f64),
    #[component(name = "square")]
    Square(f64),
    #[component(name = "none")]
    None,
}

/// JavaScript has no annotations; the world's signature types each export.
#[test]
fn untyped_exports_take_parameter_and_return_types_from_the_world() -> Result<()> {
    let source = "export const library = {
  longest(books) {
    if (books.length === 0) return null;
    let best = books[0];
    for (const book of books) { if (book.pages > best.pages) best = book; }
    return best;
  },
  describe(b, g) { return b.title + ':' + b.pages + ':' + g; },
  area(s) {
    switch (s.tag) {
      case 'circle': return 3 * s.val * s.val;
      case 'square': return s.val * s.val;
      default: return 0;
    }
  },
  parse(text) {
    if (text === '') throw 'empty';
    return text.length;
  },
  rest(data) { return data.slice(1); },
  swap(t) { return [t[1], t[0]]; },
};";
    let engine = Engine::default();
    let bytes = Compiler::Standard.component(LIBRARY, "test", "library.js", source)?;
    let (mut store, instance) = instantiate(&engine, &bytes)?;
    let store = &mut store;
    let book = |title: &str, pages| Book {
        title: title.into(),
        pages,
    };
    let (longest,): (Option<Book>,) = call(
        store,
        &instance,
        "library",
        "longest",
        (vec![book("a", 3), book("b", 9)],),
    )?;
    assert_eq!(longest, Some(book("b", 9)));
    let (none,): (Option<Book>,) = call(
        store,
        &instance,
        "library",
        "longest",
        (Vec::<Book>::new(),),
    )?;
    assert_eq!(none, None);
    let (text,): (String,) = call(
        store,
        &instance,
        "library",
        "describe",
        (book("t", 2), Genre::Fact),
    )?;
    assert_eq!(text, "t:2:fact");
    let (area,): (f64,) = call(store, &instance, "library", "area", (Shape::Square(3.0),))?;
    assert_eq!(area, 9.0);
    let (area,): (f64,) = call(store, &instance, "library", "area", (Shape::None,))?;
    assert_eq!(area, 0.0);
    let (parsed,): (std::result::Result<u32, String>,) =
        call(store, &instance, "library", "parse", ("abcd",))?;
    assert_eq!(parsed, Ok(4));
    let (parsed,): (std::result::Result<u32, String>,) =
        call(store, &instance, "library", "parse", ("",))?;
    assert_eq!(parsed, Err("empty".into()));
    let (rest,): (Vec<u8>,) = call(store, &instance, "library", "rest", (vec![1u8, 2, 3],))?;
    assert_eq!(rest, [2, 3]);
    let (swapped,): ((u32, String),) = call(
        store,
        &instance,
        "library",
        "swap",
        (("x".to_string(), 4u32),),
    )?;
    assert_eq!(swapped, (4, "x".into()));
    Ok(())
}

#[test]
fn declared_types_win_and_extra_parameters_are_rejected() -> Result<()> {
    let wit =
        "package test:conventions; world test { export join: func(a: string, n: u32) -> string; }";
    let engine = Engine::default();
    let bytes = Compiler::Standard.component(
        wit,
        "test",
        "join.ts",
        "export function join(a: string, n) { return a + n; }",
    )?;
    let (mut store, instance) = instantiate(&engine, &bytes)?;
    let join = instance.get_typed_func::<(&str, u32), (String,)>(&mut store, "join")?;
    assert_eq!(join.call(&mut store, ("x", 3))?.0, "x3");

    let error = Compiler::Standard
        .component(
            wit,
            "test",
            "join.js",
            "export function join(a, n, extra) { return a; }",
        )
        .unwrap_err();
    assert!(
        format!("{error:#}")
            .contains("declares 3 parameters, but its WIT function takes 2 parameters"),
        "{error:#}"
    );
    Ok(())
}
