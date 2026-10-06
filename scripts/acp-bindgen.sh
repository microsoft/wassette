#!/usr/bin/env bash
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT license.
#
# Regenerate the checked-in `src/bindings.rs` of every ACP component under
# `components/` from `crates/wassette-acp/wit/acp`. The installed
# `wit-bindgen` CLI must match the components' `wit-bindgen` crate version.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WIT="$REPO_ROOT/crates/wassette-acp/wit/acp"
WANT="$(sed -nE 's/^wit-bindgen = \{ version = "([0-9.]+)".*/\1/p' \
    "$REPO_ROOT/components/acp-echo-provider/Cargo.toml")"
HAVE="$(wit-bindgen --version | awk '{print $2}')"
if [ "$WANT" != "$HAVE" ]; then
    echo "error: wit-bindgen CLI $HAVE does not match crate version $WANT" >&2
    echo "  cargo install wit-bindgen-cli --version $WANT --locked" >&2
    exit 1
fi

# component-dir:world
COMPONENTS=(
    "acp-echo-provider:provider"
    "acp-ollama-provider:provider"
    "acp-copilot-provider:provider"
    "acp-uppercase-layer:layer"
)

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

for entry in "${COMPONENTS[@]}"; do
    dir="${entry%%:*}"
    world="${entry##*:}"
    wit-bindgen rust "$WIT" \
        --world "$world" \
        --runtime-path wit_bindgen::rt \
        --pub-export-macro \
        --generate-all \
        --format \
        --out-dir "$TMP/$dir"
    mv "$TMP/$dir/$world.rs" "$REPO_ROOT/components/$dir/src/bindings.rs"
    echo "regenerated components/$dir/src/bindings.rs ($world)" >&2
done
