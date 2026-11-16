import { coreSlices } from '../.generated/compiler-slices.js';

let bytesPromise;

/** Fetch the downloadable compiler component once per worker so the site ships a single binary. */
function compilerBytes() {
  return (bytesPromise ??= (async () => {
    const response = await fetch(new URL(`../../downloads/perry-wit.wasm${new URL(import.meta.url).search}`, import.meta.url));
    if (!response.ok) throw new Error(`Could not load compiler: HTTP ${response.status}`);
    return new Uint8Array(await response.arrayBuffer());
  })().catch(error => {
    bytesPromise = undefined;
    throw error;
  }));
}

export async function loadCoreModule(name) {
  const slice = coreSlices[name];
  if (!slice) throw new Error(`Missing compiler slice for ${name}`);
  const bytes = await compilerBytes();
  // WebKit (seen in Playwright's Linux build) can leave a finished compilation undelivered while
  // the worker is otherwise idle, so keep a timer pending until the module arrives.
  const awake = setInterval(() => {}, 50);
  try {
    return await WebAssembly.compile(bytes.subarray(slice.start, slice.end));
  } finally {
    clearInterval(awake);
  }
}
