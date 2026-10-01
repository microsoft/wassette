# Clean component target directories to avoid permission issues
clean-test-components:
    rm -rf examples/fetch-rs/target/
    rm -rf examples/filesystem-rs/target/

# Pre-build test components to avoid building during test execution
build-test-components:
    just clean-test-components
    just ensure-wit-docs-inject
    (cd examples/fetch-rs && CARGO_TARGET_DIR=target cargo build --release --target wasm32-wasip2)
    (cd examples/filesystem-rs && CARGO_TARGET_DIR=target cargo build --release --target wasm32-wasip2)
    # Inject docs for test components
    just finalize-component examples/fetch-rs examples/fetch-rs/target/wasm32-wasip2/release/fetch_rs.wasm
    just finalize-component examples/filesystem-rs examples/filesystem-rs/target/wasm32-wasip2/release/filesystem.wasm

test:
    just build-test-components
    just build-acp-examples
    just build-acp-routing-fixture
    (cd components/acp-copilot-provider && cargo test)
    cargo build -p wassette-mcp-server
    cargo test --workspace -- --nocapture
    cargo test --doc --workspace -- --nocapture

build-mcp-inspector-components:
    just build-test-components
    (cd examples/time-server-js && npm ci && npm run build)
    (cd examples/get-weather-js && npm ci && npm run build:component)

# Release, not debug: loading the JavaScript fixture through a debug-built
# Cranelift takes about 46s and exceeds the Inspector CLI request timeout, so a
# debug binary fails this before it reaches an assertion. Matches CI.
test-mcp-inspector:
    just build release
    just build-mcp-inspector-components
    npm ci --prefix tests/mcp-inspector
    ./scripts/test-mcp-inspector.sh

test-mcp-clients:
    just build release
    ./scripts/test-mcp-clients.sh

test-mcp-clients-negative:
    just build release
    ./scripts/test-mcp-clients.sh --negative

# Build the standalone ACP components (providers + layer)
build-acp-examples:
    (cd components/acp-echo-provider && cargo build --release --target wasm32-wasip2)
    just name-component components/acp-echo-provider components/acp-echo-provider/target/wasm32-wasip2/release/acp_echo_provider.wasm
    (cd components/acp-uppercase-layer && cargo build --release --target wasm32-wasip2)
    just name-component components/acp-uppercase-layer components/acp-uppercase-layer/target/wasm32-wasip2/release/acp_uppercase_layer.wasm
    (cd components/acp-ollama-provider && cargo build --release --target wasm32-wasip2)
    just name-component components/acp-ollama-provider components/acp-ollama-provider/target/wasm32-wasip2/release/acp_ollama_provider.wasm
    (cd components/acp-copilot-provider && cargo build --release --target wasm32-wasip2)
    just name-component components/acp-copilot-provider components/acp-copilot-provider/target/wasm32-wasip2/release/acp_copilot_provider.wasm

# Regenerate the ACP components' checked-in `bindings.rs` from
# crates/wassette-acp/wit/acp. Needs a matching `wit-bindgen` CLI.
acp-bindgen:
    ./scripts/acp-bindgen.sh

# Run the `wassette acp` end-to-end tests against the example components.
# They drive the built `wassette` binary over stdio, so build it first.
test-acp:
    just build-acp-examples
    just build-acp-tool-fixture
    just build-acp-routing-fixture
    (cd components/acp-copilot-provider && cargo test)
    cargo build -p wassette-mcp-server
    cargo test -p wassette-acp -- --nocapture

# Ordinary tool used by the ACP guest-import end-to-end tests.
build-acp-tool-fixture:
    (cd examples/filesystem-rs && cargo build --release --target wasm32-wasip2)
    just name-component examples/filesystem-rs examples/filesystem-rs/target/wasm32-wasip2/release/filesystem.wasm

# Offline provider with deliberately colliding local IDs for routing tests.
build-acp-routing-fixture:
    (cd crates/wassette-acp/tests/fixtures/routing-provider && CARGO_TARGET_DIR=target cargo build --locked --release --target wasm32-wasip2)

build mode="debug":
    mkdir -p bin
    cargo build --workspace {{ if mode == "release" { "--release" } else { "" } }}
    cp target/{{ mode }}/wassette bin/

# Opt-in helper only; this does not acquire an initrd or run a compiler VM.
build-component-builder mode="debug":
    #!/usr/bin/env bash
    set -euo pipefail
    mode={{ quote(mode) }}
    case "$mode" in debug|release) ;; *) echo "mode must be debug or release" >&2; exit 1 ;; esac
    if [[ "$mode" == release ]]; then
        cargo build -p wassette-builder --features hyperlight --release
    else
        cargo build -p wassette-builder --features hyperlight
    fi
    target_dir="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
    helper="$target_dir/$mode/wassette-builder"
    if [[ "$(uname -s)" == Darwin ]]; then
        codesign --force --sign - --entitlements scripts/generation-builder-entitlements.plist "$helper"
    fi
    printf 'Builder helper: %s\nCompute the configured helper digest after this signing step.\n' "$helper"

# Build the default-off generation CLI and its separately supervised helper.
build-component-generation mode="debug": (build-component-builder mode)
    cargo build -p wassette-mcp-server --features component-generation {{ if mode == "release" { "--release" } else { "" } }}

# Install this checkout's CLI and finalized components from components/.
install-preflight:
    python3 scripts/install-local.py --check

# Install this checkout's CLI and finalized components from components/.
install mode="release": install-preflight build-acp-examples
    python3 scripts/install-local.py --mode {{ quote(mode) }}

# Create a stable or prerelease version bump PR with the current GitHub identity.
prepare-release version:
    #!/usr/bin/env bash
    set -euo pipefail
    version={{ quote(version) }}
    repo="microsoft/wassette"
    branch="release/v$version"

    if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
        echo "error: version must use X.Y.Z or X.Y.Z-suffix format" >&2
        exit 1
    fi

    for command in cargo gh git; do
        if ! command -v "$command" >/dev/null 2>&1; then
            echo "error: $command is required" >&2
            exit 1
        fi
    done

    existing_pr=$(gh pr list \
        --repo "$repo" \
        --head "$branch" \
        --state all \
        --json url \
        --jq '.[0].url')
    if [[ -n "$existing_pr" ]]; then
        echo "Release PR already exists: $existing_pr"
        exit 0
    fi

    create_pr() {
        gh pr create \
            --repo "$repo" \
            --base main \
            --head "$branch" \
            --title "chore(release): bump version to $version" \
            --body "This pull request prepares the $version release by updating the version in \`Cargo.toml\` and \`Cargo.lock\`. After merge, run the Release workflow (e.g. \`gh workflow run release.yml -f version=$version\`) to build, tag \`v$version\`, and publish the GitHub release. Versions with a suffix are prereleases and skip stable-release updates." \
            --label release \
            --label automated
    }

    if git ls-remote --exit-code origin "refs/heads/$branch" >/dev/null 2>&1; then
        echo "Using existing remote branch $branch"
        create_pr
        exit 0
    fi

    git fetch origin main
    temporary_directory=$(mktemp -d "${TMPDIR:-/tmp}/wassette-release.XXXXXX")
    worktree="$temporary_directory/worktree"
    cleanup() {
        git worktree remove --force "$worktree" >/dev/null 2>&1 || true
        rmdir "$temporary_directory" >/dev/null 2>&1 || true
    }
    trap cleanup EXIT

    git worktree add --detach "$worktree" origin/main
    sed -i.bak "s/^version = \".*\"/version = \"$version\"/" "$worktree/Cargo.toml"
    rm "$worktree/Cargo.toml.bak"
    cargo update \
        --manifest-path "$worktree/Cargo.toml" \
        -p wassette-mcp-server \
        --precise "$version"
    git -C "$worktree" diff --check
    git -C "$worktree" add Cargo.toml Cargo.lock
    git -C "$worktree" commit -m "chore(release): bump version to $version"
    git -C "$worktree" push origin "HEAD:refs/heads/$branch"
    create_pr

# Check if wit-docs-inject is installed, if not install it
ensure-wit-docs-inject:
    #!/usr/bin/env bash
    if ! command -v wit-docs-inject &> /dev/null; then
        echo "wit-docs-inject not found, installing from https://github.com/Mossaka/wit-docs-inject"
        cargo install --git https://github.com/Mossaka/wit-docs-inject
    else
        echo "wit-docs-inject is already installed"
    fi

# Name a first-party build output using its authored producer declaration
name-component project wasm_path:
    python3 scripts/name-component.py {{ quote(project) }} {{ quote(wasm_path) }}

# Inject docs into a wasm component
inject-docs wasm_path wit_dir:
    @echo "Injecting docs into {{ wasm_path }}"
    wit-docs-inject --component {{ wasm_path }} --wit-dir {{ wit_dir }} --inplace

# Finalize a first-party source build before copying or publishing
finalize-component project wasm_path:
    just inject-docs {{ quote(wasm_path) }} {{ quote(project) }}/wit
    just name-component {{ quote(project) }} {{ quote(wasm_path) }}

build-examples mode="debug":
    mkdir -p bin
    just ensure-wit-docs-inject
    (cd examples/fetch-rs && just build {{ mode }})
    (cd examples/filesystem-rs && just build {{ mode }})
    (cd examples/get-weather-js && just build)
    (cd examples/time-server-js && just build)
    (cd examples/memory-js && just build)
    (cd examples/eval-py && just build)
    (cd examples/gomodule-go && just build)
    (cd examples/brave-search-rs && just build {{ mode }})
    (cd examples/context7-rs && just build {{ mode }})
    (cd examples/get-open-meteo-weather-js && just build)
    (cd examples/arxiv-rs && just build {{ mode }})
    (cd examples/github-js && just build)
    # Inject docs for Rust examples
    just finalize-component examples/fetch-rs examples/fetch-rs/target/wasm32-wasip2/{{ mode }}/fetch_rs.wasm
    just finalize-component examples/filesystem-rs examples/filesystem-rs/target/wasm32-wasip2/{{ mode }}/filesystem.wasm
    just finalize-component examples/brave-search-rs examples/brave-search-rs/target/wasm32-wasip2/{{ mode }}/brave_search_rs.wasm
    just finalize-component examples/arxiv-rs examples/arxiv-rs/target/wasm32-wasip2/{{ mode }}/arxiv_rs.wasm
    just finalize-component examples/context7-rs examples/context7-rs/target/wasm32-wasip2/{{ mode }}/context7.wasm
    # Inject docs for JS examples
    just finalize-component examples/get-weather-js examples/get-weather-js/weather.wasm
    just finalize-component examples/time-server-js examples/time-server-js/time.wasm
    just finalize-component examples/memory-js examples/memory-js/memory.wasm
    just finalize-component examples/get-open-meteo-weather-js examples/get-open-meteo-weather-js/weather.wasm
    just finalize-component examples/github-js examples/github-js/github.wasm
    # Inject docs for Python examples
    just finalize-component examples/eval-py examples/eval-py/eval.wasm
    # Inject docs for Go examples
    just finalize-component examples/gomodule-go examples/gomodule-go/gomodule.wasm
    # Copy to bin directory
    cp examples/fetch-rs/target/wasm32-wasip2/{{ mode }}/fetch_rs.wasm bin/fetch-rs.wasm
    cp examples/filesystem-rs/target/wasm32-wasip2/{{ mode }}/filesystem.wasm bin/filesystem.wasm
    cp examples/get-weather-js/weather.wasm bin/get-weather-js.wasm
    cp examples/time-server-js/time.wasm bin/time-server-js.wasm
    cp examples/memory-js/memory.wasm bin/memory-js.wasm
    cp examples/eval-py/eval.wasm bin/eval-py.wasm
    cp examples/gomodule-go/gomodule.wasm bin/gomodule.wasm
    cp examples/brave-search-rs/target/wasm32-wasip2/{{ mode }}/brave_search_rs.wasm bin/brave-search-rs.wasm
    cp examples/arxiv-rs/target/wasm32-wasip2/{{ mode }}/arxiv_rs.wasm bin/arxiv-rs.wasm
    cp examples/context7-rs/target/wasm32-wasip2/{{ mode }}/context7.wasm bin/context7-rs.wasm
    cp examples/get-open-meteo-weather-js/weather.wasm bin/get-open-meteo-weather-js.wasm
    cp examples/github-js/github.wasm bin/github-js.wasm
    
clean:
    cargo clean
    rm -rf bin

component2json path="examples/fetch-rs/target/wasm32-wasip2/release/fetch_rs.wasm":
    cargo run --bin component2json -p component2json -- {{ path }}

run RUST_LOG='info':
    RUST_LOG={{RUST_LOG}} cargo run --bin wassette serve --streamable-http

run-streamable RUST_LOG='info':
    RUST_LOG={{RUST_LOG}} cargo run --bin wassette serve --streamable-http

run-filesystem RUST_LOG='info':
    RUST_LOG={{RUST_LOG}} cargo run --bin wassette serve --streamable-http --component-dir ./examples/filesystem-rs

# Requires an openweather API key in the environment variable OPENWEATHER_API_KEY
run-get-weather RUST_LOG='info':
    RUST_LOG={{RUST_LOG}} cargo run --bin wassette serve --streamable-http --component-dir ./examples/get-weather-js

run-fetch-rs RUST_LOG='info':
    RUST_LOG={{RUST_LOG}} cargo run --bin wassette serve --streamable-http --component-dir ./examples/fetch-rs

run-memory RUST_LOG='info':
    RUST_LOG={{RUST_LOG}} cargo run --bin wassette serve --streamable-http --component-dir ./examples/memory-js

# Documentation commands
docs-build:
    cd docs && mdbook build

docs-serve:
    cd docs && mdbook serve --open

docs-watch:
    cd docs && mdbook serve

ci-build-test:
    just build-test-components
    cargo build --workspace
    cargo test --workspace -- --nocapture
    cargo test --doc --workspace -- --nocapture

ci-build-test-ghcr:
    just build-test-components
    cargo build --workspace
    cargo test --workspace -- --nocapture --include-ignored
    cargo test --doc --workspace -- --nocapture
