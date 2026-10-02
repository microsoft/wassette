# Sandboxed component generation

> **Status: experimental implementation.** Component generation is compiled
> into Wassette with the `component-generation` feature. It requires a
> privately provisioned builder image at
> `~/.local/share/wassette/builder/rust-initrd.cpio`, or
> `$XDG_DATA_HOME/wassette/builder/rust-initrd.cpio` when `XDG_DATA_HOME` is
> set. Wassette does not download the image.
> See the [CLI reference](../reference/cli.md#wassette-component-build),
> [management tool reference](../reference/built-in-tools.md#build-component),
> and [configuration reference](../reference/configuration-files.md#component-generation).

## Execution model

Wassette accepts bounded Rust source and WIT, prepares bindings on the host,
and runs the native compiler and component linker in a Hyperlight VM. It
validates the captured component with the appropriate runtime before
committing it through the existing component store:

```text
Rust + WIT
  -> bounded host-side preparation
  -> Hyperlight VM and private output capture
  -> component identity/kind checks and runtime validation
  -> store transaction and catalog refresh
```

The compiler image is local and private; it is not a Wasm component and is not
stored in the component catalog. The host reads the image from the path above,
records its observed digest as generation evidence, and does not accept an
image path or digest from the request. Keep the image and its parent directory
private. There is no host compiler fallback.

For each build, Wassette re-executes its own executable with a private
argument and a bounded job protocol. The child runs one Hyperlight VM; the
parent kills and reaps it on cancellation, deadline or invalid output. This
keeps native compiler and VM failures out of the server process without
installing a separate builder program or trusting a helper path or digest.
On macOS, `just install` signs the **main Wassette executable** with the
`com.apple.security.hypervisor` entitlement, which the re-executed child
inherits. The child receives a private staging directory, captured inputs
and a cleared environment. The re-exec boundary adds process startup and IPC
cost but restores crash containment and a parent-enforced wall-clock timeout.

The VM provides hardware isolation, not a general security proof. It receives
only captured source and bounded output channels—not the live component store,
project tree, secrets, network access, or management tools. The generated
component runs later under Wassette's ordinary Wasmtime tool runtime or ACP
layer runtime, not inside the builder VM.

## Availability and authorization

Generation is available when the feature is compiled in and the private image
exists. The regular `just install` recipe builds the feature-enabled CLI but
does not provision or download the image.

The host asks the editor to approve building and installation. Rebuilding an
existing generated revision is disabled by default. The build request contains
no install intent or exposure permission: ordinary catalog eligibility follows
the installed artifact kind. Every running and future ACP session picks up a
generated Tool component from the shared store and enables its exports by
default, so the model can call them on the next turn. A session can still opt
out with `/tools disable`. Invocation still follows
the component's policy and per-call approval flow.

ACP `/install` also accepts ordinary Tool components. It runs normal runtime
validation and transactional installation, then enables that component's
callable exports in the requesting session. ACP providers and layers remain
pinned to session startup; installation and catalog refresh do not replace
them. Generated ACP layers still require a new session.

## Input and validation boundaries

The current builder accepts inline Rust source and WIT. The host generates
bindings and supplies version-pinned WIT dependencies; requests cannot run
Cargo, resolve network dependencies, supply compiler flags, or select host
paths. The image fixes the compiler toolchain and build driver.

The host bounds request sizes, preparation, diagnostics, VM resources, output,
and build duration. The builder captures output privately and validates the
actual component name and kind; a requested name does not override embedded
identity. Only validated output is installed. Build failures before commit do
not replace the last committed component. Once a store transaction is
accepted, cancellation does not roll it back; inspect its reported operation
and recovery status rather than retrying as a new build.

Generated tools use the existing component permission and invocation paths.
Generated ACP layers are installed without changing a running session and
require a new session before they can be used.
