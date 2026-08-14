#!/usr/bin/env bash
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT license.
#
# Build a real (model-backed) ACP provider component against the wasmtime
# version this workspace uses.
#
# The providers live in yoshuawuyts/playground-wasm-acp and depend on `wstd`
# with a `wasip3` feature that is only on the `p3` branch of
# bytecodealliance/wstd. That branch targets the WASI rc wasmtime 44 ships;
# this workspace is on wasmtime 47, which ships final wasi:http@0.3.0. See
# real-providers/wstd-p3-wasmtime47.patch for the two-line delta that bridges
# the gap, and docs/design/acp.md for the reasoning.
#
# Usage:
#   scripts/build-acp-real-provider.sh <path-to-playground-wasm-acp> [provider]
#
# `provider` defaults to ollama-provider; copilot-provider also works.
# The built component path is printed on stdout.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PATCH="$REPO_ROOT/crates/wassette-acp/real-providers/wstd-p3-wasmtime47.patch"
WSTD_BRANCH="p3"
WSTD_URL="https://github.com/bytecodealliance/wstd"

PLAYGROUND="${1:-}"
PROVIDER="${2:-ollama-provider}"

if [ -z "$PLAYGROUND" ] || [ ! -f "$PLAYGROUND/Cargo.toml" ]; then
    echo "usage: $0 <path-to-playground-wasm-acp checkout> [provider]" >&2
    echo "  clone it from https://github.com/yoshuawuyts/playground-wasm-acp" >&2
    exit 2
fi
PLAYGROUND="$(cd "$PLAYGROUND" && pwd)"

if ! rustup target list --installed | grep -qx wasm32-wasip2; then
    echo "error: the wasm32-wasip2 target is not installed" >&2
    echo "  rustup target add wasm32-wasip2" >&2
    exit 1
fi

WORK="${ACP_PROVIDER_WORKDIR:-$REPO_ROOT/target/acp-real-providers}"
mkdir -p "$WORK"
WSTD_DIR="$WORK/wstd-$WSTD_BRANCH"

# Clone once, then keep it pinned. Re-running must not silently pick up new
# upstream commits: the patch is written against a known tree and a moving
# branch is exactly what would break it.
if [ ! -d "$WSTD_DIR/.git" ]; then
    echo "==> cloning $WSTD_URL ($WSTD_BRANCH)" >&2
    git clone --quiet --depth 1 --branch "$WSTD_BRANCH" "$WSTD_URL" "$WSTD_DIR"
    echo "==> applying $(basename "$PATCH")" >&2
    git -C "$WSTD_DIR" apply "$PATCH"
else
    echo "==> reusing $WSTD_DIR (delete it to re-clone)" >&2
fi
echo "    wstd at $(git -C "$WSTD_DIR" rev-parse --short HEAD)" >&2

# Point the playground workspace at the patched wstd. Its own manifest patches
# wstd to a path that only exists on the upstream author's machine, so this
# rewrite is required, not merely convenient.
MANIFEST="$PLAYGROUND/Cargo.toml"
python3 - "$MANIFEST" "$WSTD_DIR" <<'PY'
import re, sys

manifest, wstd = sys.argv[1], sys.argv[2]
lines = open(manifest).read().split("\n")

# Rewrite `wstd` only inside [patch.crates-io]. The same key also appears under
# [workspace.dependencies] with `default-features = false`, and replacing that
# one breaks every member crate that inherits it.
out, section, patched = [], None, False
for line in lines:
    header = re.match(r"^\[([^\]]+)\]", line)
    if header:
        section = header.group(1)
    if section == "patch.crates-io" and re.match(r"^\s*wstd\s*=", line):
        out.append('wstd = { path = "%s" }' % wstd)
        patched = True
        continue
    out.append(line)

if not patched:
    out += ["", "[patch.crates-io]", 'wstd = { path = "%s" }' % wstd, ""]

open(manifest, "w").write("\n".join(out))
PY

echo "==> building $PROVIDER for wasm32-wasip2" >&2
(cd "$PLAYGROUND" && cargo build -p "$PROVIDER" --target wasm32-wasip2 --release >&2)

ARTIFACT="$PLAYGROUND/target/wasm32-wasip2/release/${PROVIDER//-/_}.wasm"
if [ ! -f "$ARTIFACT" ]; then
    echo "error: expected artifact not found at $ARTIFACT" >&2
    exit 1
fi
echo "$ARTIFACT"
