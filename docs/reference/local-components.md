# Local component discovery

Place the finished WebAssembly component in Wassette's local drop directory.
Its logical ID is `local:<visible-filename-stem>` (without `.wasm`); embedded
root `component-name` metadata is cosmetic and may be missing or different.
`wassette run` scans this directory at startup and watches for subsequent
changes. For headless deployments, `wassette serve` leaves discovery off
unless enabled.

```bash
# The visible filename determines the local logical ID: local:weather
# Finish the build before moving the file into the inbox.
mkdir -p "${XDG_DATA_HOME:-$HOME/.local/share}/wassette/local-components"
mv ./build/weather.wasm "${XDG_DATA_HOME:-$HOME/.local/share}/wassette/local-components/weather.wasm"
wassette component sync
wassette run --local-components watch
```

The default drop directory is the platform data directory's
`wassette/local-components`: `$XDG_DATA_HOME/wassette/local-components` on
Linux and macOS (typically `~/.local/share/wassette/local-components`) and
`%APPDATA%\wassette\local-components` on Windows. This is **not** the managed
`wassette/components` store. Use `--local-component-dir` or
`WASSETTE_LOCAL_COMPONENT_DIR` to choose another directory.

`--local-components off|startup|watch` controls discovery. `startup` scans
once; `watch` polls the source directory every two seconds to pick up later
drops, including changes to symlink targets. `component sync` explicitly runs one
pass and prints the results; `--force` retries an unchanged component that
was explicitly unloaded. The same settings can be placed in `config.toml` as
`local_component_dir` and `local_components`, or provided via the
corresponding `WASSETTE_` environment variables. CLI options take precedence
over environment variables, which take precedence over the config file.

`component sync --link /absolute/path/to/tool.wasm` first registers a finished
build as a stable symlink in the resolved drop directory, then reconciles only
the explicitly linked filenames. Repeat `--link` for several components. A
plain `component sync` still scans the complete directory and performs normal
pruning. This is the mechanism used by the repository's `just install`: an
unrelated inbox entry cannot become part of that install, and the managed store
still contains receipt-backed copies, never raw links to a checkout. On Unix,
an existing link can be retargeted to a new worktree only when its source owner
and source-derived logical ID match the current receipt. Wassette refuses to
replace regular files, unrelated links, explicit/registry installations, or
conflicting names. Native Windows link registration is not currently supported.

`--adopt-explicit-local` is a narrow, explicit ownership migration for
source-checkout installers. With `--link`, it can transfer a matching explicit
local-file installation to managed link ownership when its source-derived
logical ID and visible filename match. The original admitted file source
remains the continuity identity, so the existing receipt, policy, and secrets
stay bound. Registry, HTTPS, generated, and differently named file sources are
never adopted. `just install` uses this migration to upgrade components
previously loaded directly from another Wassette worktree.

Only non-hidden `.wasm` files directly in this directory are candidates;
subdirectories and temporary filenames are ignored. The visible filename
determines the ID: `weather.wasm` becomes `local:weather`. Renaming the file
changes the ID. Embedded root names do not affect discovery or component
selection. Build into another directory and rename into the drop directory
when complete to avoid partially written inputs.

Wassette checks ownership and write permissions on Unix before reading local
files and follows symlinks only after checking their target and relevant parent
directories. Do not share the drop directory with untrusted users; Windows cannot
enforce the same Unix ownership checks. Discovery does not run a component:
ordinary tools are validated before installation, and permissions remain
governed by the component policy. ACP providers and layers require an
ACP-capable validator and are never activated by discovery. A process without
that validator reports them as deferred rather than installing them.

An explicitly loaded package does not become owned by this drop directory.
Conflicting IDs are reported instead of replaced. Deleting a drop file cannot
remove an installation with a different owner. An explicit unload suppresses
automatic reinstallation of the same unchanged source; rebuilding it, changing
its sidecar, or running `component sync --force` retries it.
Renaming a drop file changes its logical ID as well as its managed source
owner. Wassette does not transfer the old receipt, grants, secrets, or
ownership namespace to the renamed source. Resolve any conflict explicitly;
unrecorded artifacts remain protected and are not auto-adopted.

Deleting a worktree can leave its stable source links dangling. A failed
capture does not replace or prune the last-good receipt. Run `just install`
from the new checkout to retarget the links; do not delete them merely to fix a
build, because deliberate source removal can prune managed membership.
