test:
    cargo test --workspace -- --nocapture
    cargo test --doc --workspace -- --nocapture

build mode="debug":
    mkdir -p bin
    cargo build --workspace {{ if mode == "release" { "--release" } else { "" } }}
    cp target/{{ mode }}/weld-mcp-server bin/
    
build-examples mode="debug":
    mkdir -p bin
    (cd examples/fetch-rs && just build mode)
    (cd examples/filesystem-rs && just build mode)
    (cd examples/get-weather-js && just build)
    (cd examples/time-server-js && just build)
    (cd examples/eval-py && just build)
    (cd examples/gomodule-go && just build)
    cp examples/fetch-rs/target/wasm32-wasip2/{{ mode }}/fetch_rs.wasm bin/fetch-rs.wasm
    cp examples/filesystem-rs/target/wasm32-wasip2/{{ mode }}/filesystem.wasm bin/filesystem.wasm
    cp examples/get-weather-js/weather.wasm bin/get-weather-js.wasm
    cp examples/time-server-js/time.wasm bin/time-server-js.wasm
    cp examples/eval-py/eval.wasm bin/eval-py.wasm
    cp examples/gomodule-go/gomodule.wasm bin/gomodule.wasm
    
clean:
    cargo clean
    rm -rf bin

component2json path="examples/fetch-rs/target/wasm32-wasip2/release/fetch_rs.wasm":
    cargo run --bin component2json -p component2json -- {{ path }}

run RUST_LOG='info':
    RUST_LOG={{RUST_LOG}} cargo run --bin weld-mcp-server serve --http

run-filesystem RUST_LOG='info':
    RUST_LOG={{RUST_LOG}} cargo run --bin weld-mcp-server serve --http --plugin-dir ./examples/filesystem-rs --policy-file ./examples/filesystem-rs/policy.yaml

# Requires an openweather API key in the environment variable OPENWEATHER_API_KEY
run-get-weather RUST_LOG='info':
    RUST_LOG={{RUST_LOG}} cargo run --bin weld-mcp-server serve --http --plugin-dir ./examples/get-weather-js --policy-file ./examples/get-weather-js/policy.yaml

run-fetch-rs RUST_LOG='info':
    RUST_LOG={{RUST_LOG}} cargo run --bin weld-mcp-server serve --http --plugin-dir ./examples/fetch-rs --policy-file ./examples/fetch-rs/policy.yaml

