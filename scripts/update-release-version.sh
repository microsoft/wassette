#!/usr/bin/env bash

set -euo pipefail

readonly WIT_BINDGEN_VERSION="0.54.0"
readonly ACP_WIT_DIR="crates/wassette-acp/wit/acp"

if [[ $# -ne 1 ]]; then
    echo "usage: $0 <version>" >&2
    exit 1
fi

version=$1
if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
    echo "error: version must use X.Y.Z or X.Y.Z-suffix" >&2
    exit 1
fi

wit_bindgen=${WIT_BINDGEN:-wit-bindgen}
if ! command -v "$wit_bindgen" >/dev/null 2>&1; then
    echo "error: wit-bindgen-cli $WIT_BINDGEN_VERSION is required" >&2
    exit 1
fi

actual_wit_bindgen_version=$("$wit_bindgen" --version)
if [[ "$actual_wit_bindgen_version" != "wit-bindgen-cli $WIT_BINDGEN_VERSION" ]]; then
    echo "error: expected wit-bindgen-cli $WIT_BINDGEN_VERSION, found $actual_wit_bindgen_version" >&2
    exit 1
fi

replace_in_file() {
    local expression=$1
    local path=$2

    sed -E "$expression" "$path" > "$path.tmp"
    mv "$path.tmp" "$path"
}

awk -v version="$version" '
    BEGIN { updated = 0 }
    !updated && /^version = "[^"]+"$/ {
        print "version = \"" version "\""
        updated = 1
        next
    }
    { print }
    END {
        if (!updated) {
            exit 1
        }
    }
' Cargo.toml > Cargo.toml.tmp
mv Cargo.toml.tmp Cargo.toml

for wit_file in "$ACP_WIT_DIR"/*.wit; do
    replace_in_file \
        "s/^(package wassette:acp)(@[^;]+)?;/\\1@$version;/" \
        "$wit_file"
    if ! head -n1 "$wit_file" | grep -Fxq "package wassette:acp@$version;"; then
        echo "error: failed to update $wit_file" >&2
        exit 1
    fi
done

replace_in_file \
    "s|^(pub\\(crate\\) const EXPECTED_ACP_REQ: &str = \")[^\"]+(\";)|\\1^$version\\2|" \
    crates/wassette-acp/src/lib.rs
replace_in_file \
    "s|^(pub\\(crate\\) const HOST_ACP_VERSION: &str = \")[^\"]+(\";)|\\1$version\\2|" \
    crates/wassette-acp/src/lib.rs

if ! grep -Fxq "version = \"$version\"" Cargo.toml; then
    echo "error: failed to update Cargo.toml" >&2
    exit 1
fi
if ! grep -Fxq "pub(crate) const EXPECTED_ACP_REQ: &str = \"^$version\";" \
    crates/wassette-acp/src/lib.rs; then
    echo "error: failed to update EXPECTED_ACP_REQ" >&2
    exit 1
fi
if ! grep -Fxq "pub(crate) const HOST_ACP_VERSION: &str = \"$version\";" \
    crates/wassette-acp/src/lib.rs; then
    echo "error: failed to update HOST_ACP_VERSION" >&2
    exit 1
fi

temporary_directory=$(mktemp -d "${TMPDIR:-/tmp}/wassette-acp-bindings.XXXXXX")
cleanup() {
    rm -rf "$temporary_directory"
}
trap cleanup EXIT

"$wit_bindgen" rust "$ACP_WIT_DIR" \
    --world provider \
    --generate-all \
    --runtime-path wit_bindgen::rt \
    --pub-export-macro \
    --format \
    --out-dir "$temporary_directory/provider"
mv "$temporary_directory/provider/provider.rs" \
    components/acp-echo-provider/src/bindings.rs

"$wit_bindgen" rust "$ACP_WIT_DIR" \
    --world layer \
    --generate-all \
    --runtime-path wit_bindgen::rt \
    --pub-export-macro \
    --format \
    --out-dir "$temporary_directory/layer"
mv "$temporary_directory/layer/layer.rs" \
    components/acp-uppercase-layer/src/bindings.rs

cargo update -p wassette-mcp-server --precise "$version"
