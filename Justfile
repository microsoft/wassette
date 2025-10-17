# Clean component target directories to avoid permission issues
clean-test-components:
    rm -rf examples/fetch-rs/target/
    rm -rf examples/filesystem-rs/target/

# Pre-build test components to avoid building during test execution
build-test-components:
    just clean-test-components
    just ensure-wit-docs-inject
    (cd examples/fetch-rs && cargo build --release --target wasm32-wasip2)
    (cd examples/filesystem-rs && cargo build --release --target wasm32-wasip2)
    # Inject docs for test components
    just inject-docs examples/fetch-rs/target/wasm32-wasip2/release/fetch_rs.wasm examples/fetch-rs/wit
    just inject-docs examples/filesystem-rs/target/wasm32-wasip2/release/filesystem.wasm examples/filesystem-rs/wit

test:
    just build-test-components
    cargo test --workspace -- --nocapture
    cargo test --doc --workspace -- --nocapture

build mode="debug":
    mkdir -p bin
    cargo build --workspace {{ if mode == "release" { "--release" } else { "" } }}
    cp target/{{ mode }}/wassette bin/

# Check if wit-docs-inject is installed, if not install it
ensure-wit-docs-inject:
    #!/usr/bin/env bash
    if ! command -v wit-docs-inject &> /dev/null; then
        echo "wit-docs-inject not found, installing from https://github.com/Mossaka/wit-docs-inject"
        cargo install --git https://github.com/Mossaka/wit-docs-inject
    else
        echo "wit-docs-inject is already installed"
    fi

# Inject docs into a wasm component
inject-docs wasm_path wit_dir:
    @echo "Injecting docs into {{ wasm_path }}"
    wit-docs-inject --component {{ wasm_path }} --wit-dir {{ wit_dir }} --inplace

build-examples mode="debug":
    mkdir -p bin
    just ensure-wit-docs-inject
    (cd examples/fetch-rs && just build {{ mode }})
    (cd examples/filesystem-rs && just build {{ mode }})
    (cd examples/get-weather-js && just build)
    (cd examples/time-server-js && just build)
    (cd examples/eval-py && just build)
    (cd examples/gomodule-go && just build)
    (cd examples/brave-search-rs && just build {{ mode }})
    (cd examples/context7-rs && just build {{ mode }})
    (cd examples/get-open-meteo-weather-js && just build)
    # Inject docs for Rust examples
    just inject-docs examples/fetch-rs/target/wasm32-wasip2/{{ mode }}/fetch_rs.wasm examples/fetch-rs/wit
    just inject-docs examples/filesystem-rs/target/wasm32-wasip2/{{ mode }}/filesystem.wasm examples/filesystem-rs/wit
    just inject-docs examples/brave-search-rs/target/wasm32-wasip2/{{ mode }}/brave_search_rs.wasm examples/brave-search-rs/wit
    # Note: Skipping doc injection for context7-rs due to missing WIT dependencies (wasi:http@0.2.1)
    # Inject docs for JS examples
    just inject-docs examples/get-weather-js/weather.wasm examples/get-weather-js/wit
    just inject-docs examples/time-server-js/time.wasm examples/time-server-js/wit
    just inject-docs examples/get-open-meteo-weather-js/weather.wasm examples/get-open-meteo-weather-js/wit
    # Inject docs for Python examples
    just inject-docs examples/eval-py/eval.wasm examples/eval-py/wit
    # Inject docs for Go examples
    just inject-docs examples/gomodule-go/gomodule.wasm examples/gomodule-go/wit
    # Copy to bin directory
    cp examples/fetch-rs/target/wasm32-wasip2/{{ mode }}/fetch_rs.wasm bin/fetch-rs.wasm
    cp examples/filesystem-rs/target/wasm32-wasip2/{{ mode }}/filesystem.wasm bin/filesystem.wasm
    cp examples/get-weather-js/weather.wasm bin/get-weather-js.wasm
    cp examples/time-server-js/time.wasm bin/time-server-js.wasm
    cp examples/eval-py/eval.wasm bin/eval-py.wasm
    cp examples/gomodule-go/gomodule.wasm bin/gomodule.wasm
    cp examples/brave-search-rs/target/wasm32-wasip2/{{ mode }}/brave_search_rs.wasm bin/brave-search-rs.wasm
    cp examples/context7-rs/target/wasm32-wasip2/{{ mode }}/context7.wasm bin/context7-rs.wasm
    cp examples/get-open-meteo-weather-js/weather.wasm bin/get-open-meteo-weather-js.wasm
    
clean:
    cargo clean
    rm -rf bin

component2json path="examples/fetch-rs/target/wasm32-wasip2/release/fetch_rs.wasm":
    cargo run --bin component2json -p component2json -- {{ path }}

run RUST_LOG='info':
    RUST_LOG={{RUST_LOG}} cargo run --bin wassette serve --sse

run-streamable RUST_LOG='info':
    RUST_LOG={{RUST_LOG}} cargo run --bin wassette serve --streamable-http

run-filesystem RUST_LOG='info':
    RUST_LOG={{RUST_LOG}} cargo run --bin wassette serve --sse --plugin-dir ./examples/filesystem-rs

# Requires an openweather API key in the environment variable OPENWEATHER_API_KEY
run-get-weather RUST_LOG='info':
    RUST_LOG={{RUST_LOG}} cargo run --bin wassette serve --sse --plugin-dir ./examples/get-weather-js

run-fetch-rs RUST_LOG='info':
    RUST_LOG={{RUST_LOG}} cargo run --bin wassette serve --sse --plugin-dir ./examples/fetch-rs

# Documentation commands
docs-build:
    cd docs && mdbook build

docs-serve:
    cd docs && mdbook serve --open

docs-watch:
    cd docs && mdbook serve

# CI Docker commands - automatically handle user mapping to prevent permission issues
ci-local:
    docker build \
        --build-arg USER_ID=$(id -u) \
        --build-arg GROUP_ID=$(id -g) \
        -f Dockerfile.ci \
        --target ci-test \
        -t wassette-ci-local .
    docker run --rm \
        -v $(PWD):/workspace \
        -w /workspace \
        -e GITHUB_TOKEN \
        wassette-ci-local just ci-build-test

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

ci-cache-info:
    docker system df
    docker images wassette-ci-*

ci-clean:
    docker rmi $(docker images -q wassette-ci-* 2>/dev/null) 2>/dev/null || true
    docker builder prune -f

