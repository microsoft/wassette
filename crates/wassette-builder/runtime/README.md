# Inline binding runtime

The Rust files other than `support.rs` are the runtime and resource support
from the published `wit-bindgen` **0.62.0** crate, used under its MIT license
(included as `LICENSE-MIT`). They are embedded in the host crate, staged read-only,
and compiled **inside the VM** alongside the generated bindings and user source.
No host Cargo resolution, build scripts or procedural macros execute per job.

The fixed guest compiler configuration enables `std` and `async`, not
`async-spawn`, `futures-stream`, `inter-task-wakeup`, or external `bitflags`.
Native WIT async functions, resources, streams and futures use the upstream
runtime; application dependencies and detached tasks are not in this profile.
`support.rs` supplies the single-runtime task slot instead of linking the
upstream precompiled C archive. The complete runtime and driver are covered by the builder profile version in
build evidence.
