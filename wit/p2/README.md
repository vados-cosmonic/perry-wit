# WASI Preview 2 definitions

`deps/` contains the official WASI 0.2.12 package definitions distributed with
the Bytecode Alliance `wasip2` crate. The compiler embeds these files so a P2
world can resolve standard interfaces without a separate WIT download.

The P2 runtime uses `wasip2 = 2.0.1+wasi-0.2.12`, pinned in `runtime/Cargo.lock`.
The accompanying upstream license files cover these vendored definitions.
P3 WIT continues to come from `WASI_WIT_PATH` (or the existing `wit/deps` path).
