# Authoring

A component is a TypeScript entry module plus a WIT world.
Perry-WIT builds typed components that export WIT functions, CLI commands that run top-level code, and HTTP handlers.

## Typed Components

The WIT world declares the exports.
Each `kebab-case` WIT export binds to the matching `camelCase` TypeScript export; remaining module functions stay internal.
The starter template contains this pair:

```wit check=task
{{#include ../../template/wit/world.wit}}
```

```ts check=task
{{#include ../../template/src/index.ts}}
```

```sh
perry-wit src/index.ts --wit wit --world task -o dist/task.wasm
wasmtime run -C cache=n -S p3=y -W component-model-async=y --invoke 'run-task("hello")' dist/task.wasm
```

- An `async func` in WIT binds to an `async function` returning a `Promise`.
- A `result<T, E>` export returns `T` directly, and a thrown exception becomes `err`. Returning `{ ok: true, value }` or `{ ok: false, error }` also works.
- `--core-only` emits the standalone core WebAssembly module. `--keep-names` preserves function names in the custom `name` section for guest profiling.
- WASI 0.3 packages resolve from `WASI_WIT_PATH`. Packages in `wit/deps` take precedence over them.

### Exported Interfaces

A function of an exported interface binds to an exported function named after both: `add` of `interface math` is `mathAdd`.
The entry module may instead export one object per interface, as components written for `jco componentize` do:

```wit
interface math {
  add: func(x: u32, y: u32) -> u32;
}

world calculator {
  export math;
}
```

```ts
export const math = {
  add(x: number, y: number): number {
    return x + y;
  },
};
```

- The object is named after the interface in `camelCase`, without its package. It may be declared first and exported with `export { implementation as math }`.
- A property is a method, an arrow function, a function expression, or the name of a function declaration.
- Inside the object, `this.member` and `math.member` reach the other properties. Properties the interface does not declare stay internal.
- The object exists only in source. The compiler rewrites it to the exported functions above, so it cannot be passed around as a value.
- A function the world exports directly may be written `export const run = () => { ... }`.

### Untyped Exports

The world states each export's signature, so an export may leave its parameter and return types out.
The compiler takes them from the WIT function, which lets a plain JavaScript entry module implement a world:

```js
export const math = {
  add(x, y) {
    return x + y;
  },
};
```

- Types the source declares are kept. Only missing annotations are filled in.
- The return type of a `result` or `option` export is left to inference, since the source may spell it more than one way.
- Only exports are typed this way. Other functions and local variables still need types the compiler can infer.
- An export that declares more parameters than its WIT function is rejected.

### Imported Resources

An imported WIT resource is an opaque value, and its constructor, methods and static functions are imported functions that take it.
A module that exports an interface as an object, or imports a resource by name, may use the resource as a class instead:

```wit
interface counters {
  resource counter {
    constructor(start: u32);
    value: func() -> u32;
    checked: func() -> result<u32, string>;
    make: static func(start: u32) -> counter;
  }
  all: func() -> list<counter>;
}
```

```ts
import { Counter, all } from 'example:app/counters';

export function run(): number {
  using first = new Counter(1);
  const second = Counter.make(2);
  return first.value() + second.checked() + all().length;
}
```

- `new`, method calls and static calls map to the resource's WIT functions. The class name is also the resource's type.
- `using` and `value[Symbol.dispose]()` drop the resource.
- A function or method declared to return a WIT `result` returns its payload and throws its error. A `result` passed as a value is `{ tag: 'ok', val }` or `{ tag: 'err', val }`.
- Resources convert wherever they appear: in lists, tuples, records, variants and options, and in the parameters and results of exports.
- An `option` argument of an imported function, constructor or method may be `undefined` as well as `null`. An `option` the component receives is `null` when empty.
- Only the entry module's imports are converted. Other modules can receive and use the class instances.
- A module that does neither keeps the opaque values: it passes them to imported functions and drops one with the generated `dropCounter(value)`.

### Sleeping

`setTimeout` from `node:timers/promises` returns a promise. There are no callback timers and no `Promise` constructor, except in the portable way to sleep, which compiles to the same timer:

```ts
await new Promise((resolve) => setTimeout(resolve, 1000));
```

The timer's callback may also be a function that only calls `resolve()`. The world needs to import the clock, as for any timer.

### Exported Resources

A resource of an exported interface is implemented by a class named in the interface's export object, under the resource's PascalCase name:

```wit
interface counters {
  resource counter {
    constructor(start: u32);
    get: func() -> u32;
    make: static func(start: u32) -> counter;
  }
  total: func(items: list<borrow<counter>>) -> u32;
}
```

```ts
class Counter {
  #value: number;
  constructor(start: number) { this.#value = start; }
  get() { return this.#value; }
  static make(start: number) { return new Counter(start); }
  [Symbol.dispose]() { /* the host dropped this counter */ }
}

export const counters = {
  Counter,
  total(items: Counter[]) { return items.reduce((sum, item) => sum + item.get(), 0); },
};
```

- The property may name a class declared in the entry module, under any name, or hold a class expression.
- The constructor, methods and static methods implement the resource's WIT functions, and take their types from them where the class declares none. Every WIT method needs one.
- Exports take and return instances wherever the resource appears. Each instance returned becomes a new handle.
- When the host drops a handle, the instance's `[Symbol.dispose]` method runs, if it has one. An instance passed to an export as an owned argument is consumed without being disposed.
- Fields the constructor assigns from its parameters or from literals need no declaration. `#private` members compile as ordinary members.
- A module may instead export the resource's functions itself, over the opaque values: `countersConstructorCounter`, `countersMethodCounterGet(self)` and `countersStaticCounterMake`, with `newCounter(representation)` and `counterRep(value)` from `example:app/counters`. An exported `countersDtorCounter(representation)` runs when a handle is dropped.

## Generated Types

`perry-wit gen-types` writes `.d.ts` declarations for a world's imports and exports, plus a `tsconfig.json` that checks the entry module against them:

```sh
perry-wit gen-types --wit wit --world task --entry src/index.ts -o .perry/types
tsc --noEmit -p .perry/types
```

WIT integer types appear as `number` and `bigint`. See [Semantics](semantics.md) for how the compiler uses them.

## Commands

A world that includes `wasi:cli/command@0.3.0` turns the module's top-level statements, including top-level `await`, into the `wasi:cli/run` export:

```wit check=command
package example:cli@0.1.0;

world app {
  include wasi:cli/command@0.3.0;
}
```

```ts check=command
import { readFile } from "node:fs/promises";

const text = await readFile("input.json", "utf8");
const items: number[] = JSON.parse(text);
console.log("sum:", items.reduce((a, b) => a + b, 0));
if (items.length === 0) process.exitCode = 1;
```

```sh
perry-wit src/main.ts --wit wit --world app -o app.wasm
wasmtime run -S p3=y -W component-model-async=y --dir . app.wasm
```

`console` writes to WASI standard output and error, `process.env` reads the WASI environment, and `process.exitCode` or `process.exit(code)` sets the exit status.

If the module also exports functions, its top-level statements initialize the module once instead, and `wasi:cli/run` and every export wait for that initialization.
Top-level statements that start asynchronous work without awaiting it are then a compile error.

## HTTP Handlers

A module implementing the `perry:http-handler` world receives a request record and returns a response record.
The compiler wraps it in an adapter, so the component exports `wasi:http/handler@0.3.0`.
The world ships in `wit/http-handler`:

```wit check=handler
{{#include ../../wit/http-handler/world.wit}}
```

```ts check=handler
import type { Request, Response } from "./handler-types/world";

const encoder = new TextEncoder();

export function handle(request: Request): Response {
  const path = request.pathWithQuery ?? "/";
  return {
    status: 200,
    headers: [["content-type", encoder.encode("text/plain")]],
    body: encoder.encode(`${request.method.tag} ${path}\n`),
  };
}
```

```sh
perry-wit gen-types --wit wit/http-handler --world handler -o handler-types --no-tsconfig
perry-wit handler.ts --wit wit/http-handler --world handler -o handler.wasm
wasmtime serve -S p3=y -W component-model-async=y handler.wasm
```

The adapter reads the whole request body before calling `handle`.
An uncaught exception becomes a `500` response, and the instance keeps serving later requests.

### Fetch Listeners

A world that exports `wasi:http/incoming-handler@0.2` is implemented by an `incomingHandler` export object whose `handle(request, responseOut)` uses the resource classes of `wasi:http/types`.
A module without that object may register a `fetch` listener instead, at its top level:

```ts
async function reply(request: Request): Promise<Response> {
  const name = (await request.text()) || "world";
  return new Response(`Hello ${name}\n`, { headers: { "content-type": "text/plain" } });
}

addEventListener("fetch", (event) => event.respondWith(reply(event.request)));
```

- The listener is a function, or the name of a function declared in the module, taking the event. `self.addEventListener` and `globalThis.addEventListener` register it too.
- Its last statement calls `event.respondWith` with a `Response` or a promise of one. An immediately called function, as in `event.respondWith((async () => { ... })())`, is accepted.
- `event.request` is a `Request` with the method, URL, headers and body of the incoming request. The whole body is read before the listener runs.
- The response's status and body are sent, with these headers when it sets them: the `content-*` headers `type`, `encoding`, `language`, `disposition`, `location`, `range` and `security-policy`, `cache-control`, `location`, `set-cookie`, `etag`, `last-modified`, `expires`, `vary`, `allow`, `link`, `retry-after`, `www-authenticate`, `accept-ranges`, the `access-control-*` response headers, `strict-transport-security`, `x-content-type-options` and `x-frame-options`. Other headers are not sent: a `Headers` value cannot be enumerated yet.
- A listener that throws, or whose promise rejects, produces a `500` response.

## New Projects

The flake template creates a standalone project. Its development shell generates `.perry/types` on entry, and `nix build` type-checks and compiles the component:

```sh
mkdir my-component && cd my-component
nix flake init -t github:ahirner/perry-wit
nix develop
nix build
```

After editing `wit/*.wit` inside the shell, run `perry-wit gen-types --no-tsconfig --wit wit --entry src/index.ts` again before `tsc --noEmit -p .perry/types`.
