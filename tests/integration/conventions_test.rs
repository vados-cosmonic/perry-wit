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

const COUNTERS: &str = "package test:conventions;
interface counters {
  resource counter {
    constructor(start: u32);
    value: func() -> u32;
    next: func() -> counter;
    checked: func(fail: bool) -> result<u32, string>;
    make: static func(start: u32) -> counter;
  }
  all: func() -> list<counter>;
  take: func(c: counter) -> u32;
  settle: func(r: result<counter, string>) -> string;
}
world test { import counters; export run: func() -> result<string, string>; }";

struct Counter;

/// Run `body` as the export of a component importing `counters`; returns its result and
/// how many counters the guest dropped.
fn counters(file: &str, body: &str) -> Result<(String, u32)> {
    use wasmtime::component::{Component, Linker, Resource, ResourceType};
    let source = format!(
        "import {{ Counter, all, take, settle }} from 'test:conventions/counters';
         export function run() {{ {body} }}"
    );
    let engine = Engine::default();
    let component = Component::new(
        &engine,
        Compiler::Standard.component(COUNTERS, "test", file, &source)?,
    )?;
    let mut linker = Linker::<u32>::new(&engine);
    let mut host = linker.instance("test:conventions/counters")?;
    host.resource(
        "counter",
        ResourceType::host::<Counter>(),
        |mut store, _| {
            *store.data_mut() += 1;
            Ok(())
        },
    )?;
    host.func_wrap("[constructor]counter", |_, (start,): (u32,)| {
        Ok((Resource::<Counter>::new_own(start),))
    })?;
    host.func_wrap(
        "[method]counter.value",
        |_, (this,): (Resource<Counter>,)| Ok((this.rep(),)),
    )?;
    host.func_wrap(
        "[method]counter.next",
        |_, (this,): (Resource<Counter>,)| Ok((Resource::<Counter>::new_own(this.rep() + 1),)),
    )?;
    host.func_wrap(
        "[method]counter.checked",
        |_, (this, fail): (Resource<Counter>, bool)| {
            Ok((if fail {
                Err("bad".to_string())
            } else {
                Ok(this.rep())
            },))
        },
    )?;
    host.func_wrap("[static]counter.make", |_, (start,): (u32,)| {
        Ok((Resource::<Counter>::new_own(start * 10),))
    })?;
    host.func_wrap("all", |_, ()| {
        Ok((vec![
            Resource::<Counter>::new_own(1),
            Resource::<Counter>::new_own(2),
        ],))
    })?;
    host.func_wrap("take", |_, (counter,): (Resource<Counter>,)| {
        assert!(counter.owned());
        Ok((counter.rep(),))
    })?;
    host.func_wrap(
        "settle",
        |_, (result,): (std::result::Result<Resource<Counter>, String>,)| {
            Ok((match result {
                Ok(counter) => format!("ok{}", counter.rep()),
                Err(error) => format!("err{error}"),
            },))
        },
    )?;
    let mut store = Store::new(&engine, 0u32);
    let instance = linker.instantiate(&mut store, &component)?;
    let run =
        instance.get_typed_func::<(), (std::result::Result<String, String>,)>(&mut store, "run")?;
    let (result,) = run.call(&mut store, ())?;
    let text = result.map_err(|error| anyhow::anyhow!("{body}: threw {error}"))?;
    Ok((text, *store.data()))
}

/// An imported resource is a class: `new`, methods, static functions and disposal map to
/// the resource's WIT functions, and values holding resources convert at each call.
#[test]
fn imported_resources_are_classes() -> Result<()> {
    for (body, expected) in [
        ("const c = new Counter(3); return '' + c.value();", "3"),
        ("return '' + Counter.make(2).next().value();", "21"),
        (
            "const items = all(); return '' + items.length + items[1].value();",
            "22",
        ),
        ("const c = new Counter(5); return '' + take(c);", "5"),
        // A method declared to return a WIT result returns its payload or throws its error.
        (
            "const c = new Counter(6); return '' + c.checked(false);",
            "6",
        ),
        (
            "const c = new Counter(6);
             try { c.checked(true); return 'no'; }
             catch (error) { return error === 'bad' ? 'caught' : 'other'; }",
            "caught",
        ),
        // A result passed as a value is spelled `{ tag, val }`.
        (
            "return settle({ tag: 'ok', val: new Counter(8) }) + settle({ tag: 'err', val: 'no' });",
            "ok8errno",
        ),
    ] {
        assert_eq!(counters("counters.ts", body)?.0, expected, "{body}");
        // The same body needs no annotations.
        assert_eq!(counters("counters.js", body)?.0, expected, "{body}");
    }
    Ok(())
}

#[test]
fn resource_classes_are_disposed_by_using_and_symbol_dispose() -> Result<()> {
    for body in [
        "let text = ''; { using c = new Counter(11); text = '' + c.value(); } return text;",
        "const c = new Counter(11); const text = '' + c.value(); c[Symbol.dispose](); return text;",
    ] {
        assert_eq!(counters("counters.ts", body)?, ("11".into(), 1), "{body}");
    }
    Ok(())
}

/// A class named in an interface's export object implements the interface's resource.
#[test]
fn a_class_implements_an_exported_resource() -> Result<()> {
    use wasmtime::component::{Component, Linker, ResourceAny};
    let wit = "package test:conventions;
interface counters {
  resource counter {
    constructor(start: u32);
    get: func() -> u32;
    increment: func();
    twin: func() -> counter;
    make: static func(start: u32) -> counter;
  }
  take: func(c: counter) -> u32;
  peek: func(c: borrow<counter>) -> u32;
  live: func() -> u32;
}
world test { export counters; }";
    let source = "let live = 0;
class Counter {
  #value;
  constructor(start) { this.#value = start; live += 1; }
  get() { return this.#value; }
  increment() { this.#value += 1; }
  twin() { return new Counter(this.#value); }
  static make(start) { return new Counter(start * 10); }
  [Symbol.dispose]() { live -= 1; }
}
export const counters = {
  Counter,
  take(c) { return c.get(); },
  peek(c) { return c.get(); },
  live() { return live; },
};";
    let engine = Engine::default();
    let component = Component::new(
        &engine,
        Compiler::Standard.component(wit, "test", "counters.js", source)?,
    )?;
    let mut store = Store::new(&engine, ());
    let instance = Linker::<()>::new(&engine).instantiate(&mut store, &component)?;
    let (counter,): (ResourceAny,) = call(
        &mut store,
        &instance,
        "counters",
        "[constructor]counter",
        (3u32,),
    )?;
    call::<_, ()>(
        &mut store,
        &instance,
        "counters",
        "[method]counter.increment",
        (counter,),
    )?;
    let (value,): (u32,) = call(
        &mut store,
        &instance,
        "counters",
        "[method]counter.get",
        (counter,),
    )?;
    assert_eq!(value, 4);
    let (twin,): (ResourceAny,) = call(
        &mut store,
        &instance,
        "counters",
        "[method]counter.twin",
        (counter,),
    )?;
    let (made,): (ResourceAny,) = call(
        &mut store,
        &instance,
        "counters",
        "[static]counter.make",
        (2u32,),
    )?;
    let (peeked,): (u32,) = call(&mut store, &instance, "counters", "peek", (twin,))?;
    assert_eq!(peeked, 4);
    let live = |store: &mut Store<()>| -> Result<u32> {
        let (live,): (u32,) = call(store, &instance, "counters", "live", ())?;
        Ok(live)
    };
    assert_eq!(live(&mut store)?, 3);
    // Dropping a handle disposes its instance; an owned argument is consumed undisposed.
    twin.resource_drop(&mut store)?;
    assert_eq!(live(&mut store)?, 2);
    let (taken,): (u32,) = call(&mut store, &instance, "counters", "take", (made,))?;
    assert_eq!(taken, 20);
    assert_eq!(live(&mut store)?, 2);
    counter.resource_drop(&mut store)?;
    assert_eq!(live(&mut store)?, 1);
    Ok(())
}

/// Resources cross the export boundary as class instances too.
#[test]
fn exports_take_and_return_resource_classes() -> Result<()> {
    use wasmtime::component::{Component, Linker, Resource, ResourceType};
    let wit = "package test:conventions;
interface counters { resource counter { constructor(start: u32); value: func() -> u32; } }
interface api {
  use counters.{counter};
  describe: func(c: counter) -> string;
  create: func(start: u32) -> counter;
}
world test { import counters; export api; }";
    let source = "import { Counter } from 'test:conventions/counters';
export const api = {
  describe(c) { return 'got' + c.value(); },
  create(start) { return new Counter(start); },
};";
    let engine = Engine::default();
    let component = Component::new(
        &engine,
        Compiler::Standard.component(wit, "test", "api.js", source)?,
    )?;
    let mut linker = Linker::<()>::new(&engine);
    let mut host = linker.instance("test:conventions/counters")?;
    host.resource("counter", ResourceType::host::<Counter>(), |_, _| Ok(()))?;
    host.func_wrap("[constructor]counter", |_, (start,): (u32,)| {
        Ok((Resource::<Counter>::new_own(start),))
    })?;
    host.func_wrap(
        "[method]counter.value",
        |_, (this,): (Resource<Counter>,)| Ok((this.rep(),)),
    )?;
    let mut store = Store::new(&engine, ());
    let instance = linker.instantiate(&mut store, &component)?;
    let (_, api) = instance
        .get_export(&mut store, None, "test:conventions/api")
        .context("no exported api")?;
    let (_, describe) = instance
        .get_export(&mut store, Some(&api), "describe")
        .context("no describe")?;
    let (_, create) = instance
        .get_export(&mut store, Some(&api), "create")
        .context("no create")?;
    let describe =
        instance.get_typed_func::<(Resource<Counter>,), (String,)>(&mut store, describe)?;
    let create = instance.get_typed_func::<(u32,), (Resource<Counter>,)>(&mut store, create)?;
    let (text,) = describe.call(&mut store, (Resource::new_own(3),))?;
    assert_eq!(text, "got3");
    let (counter,) = create.call(&mut store, (5,))?;
    assert_eq!(counter.rep(), 5);
    Ok(())
}
