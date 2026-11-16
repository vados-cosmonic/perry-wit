import { coreSlices } from '../.generated/compiler-slices.js';

let bytesPromise;
let chain = Promise.resolve();
const trace = message => postMessage({ kind: 'progress', message: `[t] ${message}` });

/** Fetch the downloadable compiler component once per worker so the site ships a single binary. */
function compilerBytes() {
  return (bytesPromise ??= (async () => {
    trace('fetch compiler');
    const response = await fetch(new URL(`../../downloads/perry-wit.wasm${new URL(import.meta.url).search}`, import.meta.url));
    if (!response.ok) throw new Error(`Could not load compiler: HTTP ${response.status}`);
    trace('fetch headers');
    const body = new Uint8Array(await response.arrayBuffer());
    trace('fetch body');
    return body;
  })().catch(error => {
    bytesPromise = undefined;
    throw error;
  }));
}

export async function loadCoreModule(name) {
  const slice = coreSlices[name];
  if (!slice) throw new Error(`Missing compiler slice for ${name}`);
  const bytes = await compilerBytes();
  trace(`compile ${name}`);
  const module = await (chain = chain.then(() => WebAssembly.compile(bytes.subarray(slice.start, slice.end))));
  trace(`compiled ${name}`);
  return module;
}
