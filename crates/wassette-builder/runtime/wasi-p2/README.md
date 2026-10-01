# Admitted implicit runtime imports

These unmodified WIT packages are the WASI 0.2.12 snapshot shipped in
`wasmtime-wasi` 47.0.4 (`src/p2/wit/deps`), under its accompanying license.
They are embedded only in the host crate, never guest-mounted.

The fixed profile admits only the interfaces in `wasi:cli/imports`, at stable
0.2.0 through 0.2.12 versions. Actual imports must be typed subsets of these
interfaces; `@since` annotations exclude members introduced after the imported
version, and unstable members are not admitted. Other imports must appear in
the resolved requested world. There is no namespace-prefix wildcard admission.
The parent's matching-runtime linker and capability policy still apply.
