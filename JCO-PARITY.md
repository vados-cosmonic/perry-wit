# Jco parity status

> Tracking file for the `feat/jco-guest-conventions` pull request.
> **Remove this file and `JCO-PARITY.snapshot.json` before merge.**

This records how many of Jco's JavaScript and TypeScript components compile with the
`perry-wit` componentize backend.

## How the snapshot was produced

- perry-wit: this branch at `20bc70c`, built as the wasm compiler in the `perry-wit` npm package.
- Jco: [`vados-cosmonic/jco`](https://github.com/vados-cosmonic/jco) branch
  `feat/perry-wit-componentize` at `3422fe9b`.
- Command, from the Jco checkout, with the package installed in `<project>`:

  ```sh
  node scripts/perry-wit-parity.mjs --project <project> --json results.json
  ```

- Node v26.13.0. Date: 2026-11-13.

The script compiles each component of `examples/components/*` and
`packages/jco/test/fixtures/componentize/*` from its original source.
"Compiled" means the compiler produced a component; the script does not run it.

## Totals

| | Components | Compiled | Rejected | Skipped |
| --- | --- | --- | --- | --- |
| All | 88 | 16 | 66 | 6 |
| No Node.js built-ins | 32 | 16 | 12 | 4 |
| Node.js built-ins | 56 | 0 | 54 | 2 |

Before this branch (`p2-on-npm-package` at `e0de5cd`), 2 compiled: `local-dependency` and `typescript-direct`.

Skipped: four are out of scope for an ahead-of-time compiler
(`http-server-hono`, `http-server-hono-with-bindings`, `p3-stream-echo-qjs`, `quickjs-futures`),
`node-compat` has no entry module without a build, and `node-tls` has no WIT world.

The Node.js components are not a goal of this branch.
They fail on untyped JavaScript and on Node.js modules perry-wit does not model.

## Components without Node.js built-ins

### Compiling (16)

`add`, `adder`, `builtin-bundle-ts`, `host-logging`, `http-hello-world`,
`http-server-fetch-handler`, `string-reverse`, `string-reverse-upper`, `ts-resource-export`,
`local-dependency`, `resource-disposal`, `script-only/calc.ts`, `simple-resource`,
`typescript-direct`, `wasi-http-detection-new`, `wasi-http-detection-old`.

### Still failing (12)

| Component | Diagnostic | Cause | Where a fix belongs |
| --- | --- | --- | --- |
| `script-only/greet.js` | `codegen-failure: optimizing MIR` | Numeric specialization gives a class a layout that its module binding does not share, so `perry.global_load` fails verification. Repro in the message of `143f47a`. | perry-wit, MIR specializer |
| `ts-resource-import` | `codegen-failure: lowering function runRun to wasm` | The source uses `console`, and the world does not import `wasi:cli/stderr`. | The example's world, or perry-wit dropping `console` output without stderr |
| `typegen-async-export` | `codegen-failure: lowering async function ... to wasm` | The sleep idiom is rewritten to a timer, but the world imports no clock. | The example's world |
| `fs-write-file` | `unsupported-expression: method openAt is not supported on record yet` | WIT `flags` are not supported at the ABI, so `descriptor.open-at` is not bound. | perry-wit |
| `webidl-book-library` | `unsupported-expression: static field ... NonFiction not found` | A static constant on an imported resource class. | perry-wit |
| `script-only-pipeline/dataset.ts`, `geoip.ts`, `risk-score.ts`, `top-pages.ts` | `unsupported-expression: ... GlobalGet(0)` | The scripts share state on `globalThis` across one StarlingMonkey realm. | Out of reach by design |
| `browser-p2-shims`, `native-messaging` | `missing-type-annotation` | Untyped values inside the module, beyond the exports that take their types from the world. | The sources (annotations), or wider inference |
| `wintertc` | `unsupported-expression: ... ReadableStreamDefaultController` | Web APIs such as `ReadableStream`. | perry-wit runtime surface |

### Out of scope (4, skipped)

| Component | Reason |
| --- | --- |
| `http-server-hono`, `http-server-hono-with-bindings` | Depend on the `hono` npm package, which needs a bundler. |
| `p3-stream-echo-qjs`, `quickjs-futures` | Written against the QuickJS backend's API. |

## Known differences from Jco's engine backends

- An empty `option` the component receives is `null`, not `undefined`.
  Only `option` arguments of imports accept `undefined`.
- A fetch listener's response sends a fixed list of headers, because `Headers` cannot be enumerated.
- The conventions apply to the entry module only.

## Verification status

- Run: the frontend unit tests, and harness cases that compile with the wasm build of the
  compiler, transpile with `jco transpile` and run under Node.
- Not run: the integration tests added on this branch
  (`tests/integration/conventions_test.rs`, `tests/integration/p2_test.rs`).
  They type-check, but the native Wasmtime test binary could not be linked where they were written.

## Full snapshot

`JCO-PARITY.snapshot.json` holds the same rows.

| Set | Component | Status | First diagnostic or reason |
| --- | --- | --- | --- |
| example | `add` | compiled |  |
| example | `adder` | compiled |  |
| example | `browser-p2-shims` | rejected | missing-type-annotation: the value type cannot be inferred for the MIR backend |
| example | `builtin-bundle-ts` | compiled |  |
| example | `fs-write-file` | rejected | unsupported-expression: method openAt is not supported on record yet |
| example | `host-logging` | compiled |  |
| example | `http-hello-world` | compiled |  |
| example | `http-server-fetch-handler` | compiled |  |
| example | `http-server-hono` | skipped | depends on the hono npm package, which needs a bundler |
| example | `http-server-hono-with-bindings` | skipped | depends on the hono npm package, which needs a bundler |
| example | `native-messaging` | rejected | missing-type-annotation: the value type cannot be inferred for the MIR backend |
| example | `node-compat` | skipped | the entry module does not exist |
| example | `node-fetch` | rejected | codegen-failure: lowering async function simpleRequestGetJson to wasm |
| example | `node-grpc-server` | rejected | module-resolution: Cannot resolve './gen/example/greeter/v1/greeter_pb.js' from server.js |
| example | `node-mcp-server` | rejected | unsupported-expression: Readable from stream is not supported yet |
| example | `nodejs-platform` | rejected | missing-type-annotation: the value type cannot be inferred for the MIR backend |
| example | `p3-stream-echo-qjs` | skipped | written for the QuickJS backend |
| example | `script-only-pipeline/dataset.ts` | rejected | unsupported-expression: MIR backend does not support expression GlobalGet(0) yet |
| example | `script-only-pipeline/geoip.ts` | rejected | unsupported-expression: MIR backend does not support expression GlobalGet(0) yet |
| example | `script-only-pipeline/risk-score.ts` | rejected | unsupported-expression: MIR backend does not support expression GlobalGet(0) yet |
| example | `script-only-pipeline/top-pages.ts` | rejected | unsupported-expression: MIR backend does not support expression GlobalGet(0) yet |
| example | `string-reverse` | compiled |  |
| example | `string-reverse-upper` | compiled |  |
| example | `ts-resource-export` | compiled |  |
| example | `ts-resource-import` | rejected | codegen-failure: lowering function runRun to wasm |
| example | `typegen-async-export` | rejected | codegen-failure: lowering async function __perry_integer_1_exampleSlowDouble to wasm |
| example | `webidl-book-library` | rejected | unsupported-expression: static field __jco_WebidlPkgGlobalBookLibraryBookManager.NonFiction not found |
| example | `wintertc` | rejected | unsupported-expression: MIR backend does not support unresolved generic type ReadableStreamDefaultController |
| fixture | `local-dependency` | compiled |  |
| fixture | `node-assert` | rejected | missing-type-annotation: the value type cannot be inferred for the MIR backend |
| fixture | `node-buffer` | rejected | unsupported-expression: failed to lower source.js: The requested module 'node:buffer' does not provide an export named 'SlowBuffer' |
| fixture | `node-child-process` | rejected | unsupported-expression: MIR backend does not support expression ChildProcessSpawnSync { command: String("node"), args: Some(Array([String("-e"), String("process.stdin.pipe(process.stdout)")], None)),  |
| fixture | `node-cluster` | rejected | missing-type-annotation: the value type cannot be inferred for the MIR backend |
| fixture | `node-cluster-roundtrip` | rejected | unsupported-expression: an empty array literal needs a declared element type |
| fixture | `node-console` | rejected | unsupported-expression: MIR backend does not support expression NativeMethodCall { module: "console", class_name: None, object: None, method: "log", args: [String("guest stdout %d"), Integer(24)] } ye |
| fixture | `node-dgram` | rejected | missing-type-annotation: the value type cannot be inferred for the MIR backend |
| fixture | `node-dns` | rejected | unsupported-expression: MIR backend does not support expression NativeModuleRef("dns") yet |
| fixture | `node-errors` | rejected | unsupported-expression: type AggregateError is not supported by the MIR backend yet |
| fixture | `node-events` | rejected | unsupported-expression: type EventEmitter is not supported by the MIR backend yet |
| fixture | `node-express` | rejected | unsupported-expression: MIR backend does not support expression DynamicImport { paths: [], arg: String("express"), options: None, byte_offset: 167, deferred_error: None, synchronous: false, deferred:  |
| fixture | `node-ffi` | rejected | missing-type-annotation: the value type cannot be inferred for the MIR backend |
| fixture | `node-fs` | rejected | unsupported-expression: Unsupported filesystem import fs: synchronous and callback APIs are not supported; use node:fs/promises |
| fixture | `node-globals` | rejected | missing-type-annotation: the value type cannot be inferred for the MIR backend |
| fixture | `node-http` | rejected | missing-type-annotation: the value type cannot be inferred for the MIR backend |
| fixture | `node-http-server` | rejected | unsupported-expression: MIR backend does not support expression NativeMethodCall { module: "http", class_name: None, object: None, method: "createServer", args: [Closure { func_id: 3, params: [Param { |
| fixture | `node-http-sockets-server` | rejected | unsupported-expression: MIR backend does not support expression NativeMethodCall { module: "http", class_name: None, object: None, method: "createServer", args: [Closure { func_id: 2, params: [Param { |
| fixture | `node-http2` | rejected | missing-type-annotation: the value type cannot be inferred for the MIR backend |
| fixture | `node-https` | rejected | missing-type-annotation: the value type cannot be inferred for the MIR backend |
| fixture | `node-https-server` | rejected | unsupported-expression: MIR backend does not support expression NativeMethodCall { module: "https", class_name: None, object: None, method: "createServer", args: [New { class_name: "__AnonShape_b8f1e9 |
| fixture | `node-https-wasi-tls` | rejected | unsupported-expression: class Promise is not supported yet |
| fixture | `node-inspector` | rejected | unsupported-expression: an empty array literal needs a declared element type |
| fixture | `node-module` | rejected | missing-type-annotation: the value type cannot be inferred for the MIR backend |
| fixture | `node-net` | rejected | unsupported-expression: MIR backend does not support expression NativeMethodCall { module: "net", class_name: None, object: None, method: "BlockList", args: [] } yet |
| fixture | `node-os` | rejected | missing-type-annotation: the value type cannot be inferred for the MIR backend |
| fixture | `node-path` | rejected | unsupported-expression: MIR backend does not support expression PathJoin(PathJoin(PathJoin(String("a"), String("b")), String("..")), String("c")) yet |
| fixture | `node-path-missing-environment` | rejected | unsupported-expression: MIR backend does not support expression PathJoin(String("a"), String("b")) yet |
| fixture | `node-path-p3` | rejected | missing-type-annotation: the value type cannot be inferred for the MIR backend |
| fixture | `node-perf-hooks` | rejected | unsupported-expression: MIR backend does not support expression NativeMethodCall { module: "perf_hooks", class_name: None, object: None, method: "clearMarks", args: [] } yet |
| fixture | `node-process` | rejected | unsupported-expression: Unsupported capability import process: only node:timers/promises, node:fs/promises and Writable from node:stream are supported so far |
| fixture | `node-process-custom` | rejected | unsupported-expression: Unsupported capability import process: only node:timers/promises, node:fs/promises and Writable from node:stream are supported so far |
| fixture | `node-querystring` | rejected | unsupported-expression: MIR backend does not support expression NativeMethodCall { module: "querystring", class_name: None, object: None, method: "parse", args: [String("value=one&value=two+words&empt |
| fixture | `node-readline` | rejected | unsupported-expression: failed to lower source.js: The requested module 'node:readline' does not provide an export named 'Interface' |
| fixture | `node-repl` | rejected | unsupported-expression: failed to lower source.js: The requested module 'node:readline' does not provide an export named 'Interface' |
| fixture | `node-sqlite` | rejected | unsupported-expression: Unsupported capability import node:sqlite: only node:timers/promises, node:fs/promises and Writable from node:stream are supported so far |
| fixture | `node-stream` | rejected | unsupported-expression: failed to lower source.js: Cannot find module 'node:stream/iter'. No such built-in module: node:stream/iter |
| fixture | `node-stream-classic` | rejected | unsupported-expression: only named imports from stream are supported yet |
| fixture | `node-string-decoder` | rejected | unsupported-expression: type StringDecoder is not supported by the MIR backend yet |
| fixture | `node-test` | rejected | unsupported-expression: failed to lower source.js: The requested module 'node:test' does not provide an export named 'getTestContext' |
| fixture | `node-timers` | rejected | unsupported-expression: Unsupported capability import timers: only node:timers/promises, node:fs/promises and Writable from node:stream are supported so far |
| fixture | `node-tls` | skipped | the component has no WIT world |
| fixture | `node-trace-events` | rejected | missing-type-annotation: the value type cannot be inferred for the MIR backend |
| fixture | `node-tty` | rejected | missing-type-annotation: the value type cannot be inferred for the MIR backend |
| fixture | `node-url` | rejected | unsupported-expression: failed to lower source.js: URL constructor requires at least 1 argument |
| fixture | `node-util` | rejected | unsupported-expression: lone UTF-16 surrogate in a string literal; Perry strings contain Unicode scalars only |
| fixture | `node-v8` | rejected | missing-type-annotation: the value type cannot be inferred for the MIR backend |
| fixture | `node-vfs` | rejected | unsupported-expression: failed to lower component.js: Cannot find module 'node:vfs'. No such built-in module: node:vfs |
| fixture | `node-vm` | rejected | missing-type-annotation: the value type cannot be inferred for the MIR backend |
| fixture | `node-wasi` | rejected | missing-type-annotation: the value type cannot be inferred for the MIR backend |
| fixture | `node-worker-threads` | rejected | missing-type-annotation: the value type cannot be inferred for the MIR backend |
| fixture | `node-zlib` | rejected | unsupported-expression: Transform from stream is not supported yet |
| fixture | `quickjs-futures` | skipped | uses the QuickJS backend's `wit` global |
| fixture | `resource-disposal` | compiled |  |
| fixture | `script-only/calc.ts` | compiled |  |
| fixture | `script-only/greet.js` | rejected | codegen-failure: optimizing MIR |
| fixture | `simple-resource` | compiled |  |
| fixture | `typescript-direct` | compiled |  |
| fixture | `wasi-http-detection-new` | compiled |  |
| fixture | `wasi-http-detection-old` | compiled |  |
