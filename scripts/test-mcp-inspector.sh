#!/usr/bin/env bash
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT license.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INSPECTOR_PACKAGE="${INSPECTOR_PACKAGE:-@modelcontextprotocol/inspector@2.2.0}"
INSPECTOR_BIN="${INSPECTOR_BIN:-$REPO_ROOT/tests/mcp-inspector/node_modules/.bin/mcp-inspector}"
INSPECTOR_CONFIG_SOURCE="${INSPECTOR_CONFIG:-$REPO_ROOT/.config/mcp-inspector.json}"
WASSETTE_BIN="${WASSETTE_BIN:-$REPO_ROOT/bin/wassette}"
WASSETTE_PORT="${WASSETTE_PORT:-}"
FIXTURE_PORT="${FIXTURE_PORT:-}"

FETCH_COMPONENT="${FETCH_COMPONENT:-$REPO_ROOT/examples/fetch-rs/target/wasm32-wasip2/release/fetch_rs.wasm}"
FILESYSTEM_COMPONENT="${FILESYSTEM_COMPONENT:-$REPO_ROOT/examples/filesystem-rs/target/wasm32-wasip2/release/filesystem.wasm}"
TIME_COMPONENT="${TIME_COMPONENT:-$REPO_ROOT/examples/time-server-js/time.wasm}"

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/wassette-inspector.XXXXXX")"
INSPECTOR_CONFIG="$TMP_DIR/mcp-inspector.json"
WASSETTE_PID=""
HTTP_PID=""
LOCKED_PID=""

stop_process() {
    local pid=$1

    if [[ -z "$pid" ]]; then
        return
    fi

    if kill -0 "$pid" 2>/dev/null; then
        kill -TERM "$pid" 2>/dev/null || true
        for _ in $(seq 1 50); do
            if ! kill -0 "$pid" 2>/dev/null; then
                break
            fi
            sleep 0.1
        done
        if kill -0 "$pid" 2>/dev/null; then
            kill -KILL "$pid" 2>/dev/null || true
        fi
    fi
    wait "$pid" 2>/dev/null || true
}

cleanup() {
    local exit_code=$?
    trap - EXIT INT TERM

    stop_process "$WASSETTE_PID"
    stop_process "$LOCKED_PID"
    stop_process "$HTTP_PID"
    rm -rf "$TMP_DIR"
    exit "$exit_code"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

require_command() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "error: $1 is required" >&2
        exit 1
    fi
}

for command in curl jq node npx python3; do
    require_command "$command"
done

node -e '
const [major, minor] = process.versions.node.split(".").map(Number);
if (major < 22 || (major === 22 && minor < 19)) {
  console.error(`error: MCP Inspector v2 requires Node >=22.19.0; found ${process.versions.node}`);
  process.exit(1);
}
'

for path in "$INSPECTOR_CONFIG_SOURCE" "$WASSETTE_BIN" "$FETCH_COMPONENT" "$FILESYSTEM_COMPONENT" "$TIME_COMPONENT"; do
    if [[ ! -e "$path" ]]; then
        echo "error: required test artifact does not exist: $path" >&2
        exit 1
    fi
done

path_to_file_uri() {
    python3 - "$1" <<'PY'
import pathlib
import sys

print(pathlib.Path(sys.argv[1]).resolve().as_uri())
PY
}

FETCH_COMPONENT_URI="$(path_to_file_uri "$FETCH_COMPONENT")"
FILESYSTEM_COMPONENT_URI="$(path_to_file_uri "$FILESYSTEM_COMPONENT")"
TIME_COMPONENT_URI="$(path_to_file_uri "$TIME_COMPONENT")"

read -r WASSETTE_PORT FIXTURE_PORT < <(
    python3 - "$WASSETTE_PORT" "$FIXTURE_PORT" <<'PY'
import socket
import sys

sockets = []
ports = []
try:
    for value in sys.argv[1:]:
        port = int(value) if value else 0
        if not 0 <= port <= 65535:
            raise ValueError(f"invalid TCP port: {port}")
        sock = socket.socket()
        sock.bind(("127.0.0.1", port))
        sockets.append(sock)
        ports.append(sock.getsockname()[1])
except (OSError, ValueError) as error:
    print(f"error: cannot reserve MCP Inspector test ports: {error}", file=sys.stderr)
    sys.exit(1)
finally:
    for sock in sockets:
        sock.close()

print(*ports)
PY
)

MCP_URL="http://127.0.0.1:$WASSETTE_PORT/mcp"
READY_URL="http://127.0.0.1:$WASSETTE_PORT/ready"
FIXTURE_URL="http://127.0.0.1:$FIXTURE_PORT/fixture.txt"

mkdir -p "$TMP_DIR/components" "$TMP_DIR/http" "$TMP_DIR/fs"
printf 'served by the Wassette Inspector fixture\n' > "$TMP_DIR/http/fixture.txt"
printf 'read through a real filesystem component\n' > "$TMP_DIR/fs/component.txt"
jq --arg url "$MCP_URL" '
    (.mcpServers[] | select(.type == "http") | .url) = $url
' "$INSPECTOR_CONFIG_SOURCE" > "$INSPECTOR_CONFIG"

wait_for_url() {
    local pid=$1
    local url=$2
    local label=$3
    local log=$4

    for _ in $(seq 1 100); do
        if ! kill -0 "$pid" 2>/dev/null; then
            echo "error: $label exited before becoming ready" >&2
            cat "$log" >&2
            return 1
        fi
        if curl --fail --silent "$url" >/dev/null; then
            return
        fi
        sleep 0.1
    done

    echo "error: $label did not become ready" >&2
    cat "$log" >&2
    return 1
}

python3 -m http.server "$FIXTURE_PORT" --bind 127.0.0.1 --directory "$TMP_DIR/http" \
    >"$TMP_DIR/http.log" 2>&1 &
HTTP_PID=$!
wait_for_url "$HTTP_PID" "$FIXTURE_URL" "HTTP fixture" "$TMP_DIR/http.log"

RUST_LOG=warn "$WASSETTE_BIN" serve \
    --streamable-http \
    --bind-address "127.0.0.1:$WASSETTE_PORT" \
    --component-dir "$TMP_DIR/components" \
    >"$TMP_DIR/wassette.log" 2>&1 &
WASSETTE_PID=$!
wait_for_url "$WASSETTE_PID" "$READY_URL" "Wassette" "$TMP_DIR/wassette.log"

inspector_with_config() {
    local config=$1
    local server=$2
    local output
    shift 2
    if [[ -x "$INSPECTOR_BIN" ]]; then
        if ! output="$("$INSPECTOR_BIN" --cli \
            --config "$config" \
            --server "$server" \
            "$@" \
            --format json)"; then
            printf '%s\n' "$output" >&2
            return 1
        fi
    else
        if ! output="$(npx --yes "$INSPECTOR_PACKAGE" --cli \
            --config "$config" \
            --server "$server" \
            "$@" \
            --format json)"; then
            printf '%s\n' "$output" >&2
            return 1
        fi
    fi
    printf '%s\n' "$output"
}

inspector() {
    local server=$1
    shift
    inspector_with_config "$INSPECTOR_CONFIG" "$server" "$@"
}

call_tool() {
    local server=$1
    local name=$2
    local arguments=$3
    inspector "$server" \
        --method tools/call \
        --tool-name "$name" \
        --tool-args-json "$arguments"
}

assert_tool_call_succeeded() {
    jq -e '
        .result
        | (.isError // false) == false
        and (.content | type == "array" and length > 0)
    ' >/dev/null
}

# A component denied by policy does not fail the MCP call. Wasmtime simply never
# shows it the resource, so the component reports its own error and `isError`
# stays false. Assert on the component's payload, not on the MCP envelope.
assert_component_reported_error() {
    jq -e '
        .result
        | (.structuredContent.result.err // empty)
        | type == "string" and length > 0
    ' >/dev/null
}

assert_component_succeeded() {
    jq -e '
        .result
        | (.structuredContent.result.err // null) == null
    ' >/dev/null
}

echo "Checking MCP 2 discovery and legacy initialization"
modern_info="$(inspector wassette-modern --method initialize)"
legacy_info="$(inspector wassette-legacy --method initialize)"
jq -e '
    .result.protocolVersion == "2026-07-28"
    and .result.capabilities.tools.listChanged == true
    and (.result.capabilities | has("prompts") | not)
    and (.result.capabilities | has("resources") | not)
' <<<"$modern_info" >/dev/null
jq -e '
    .result.protocolVersion == "2025-11-25"
    and .result.capabilities.tools.listChanged == true
    and (.result.serverInfo.name | type == "string" and length > 0)
' \
    <<<"$legacy_info" >/dev/null

echo "Checking tool discovery and initial state in both protocol eras"
for server in wassette-modern wassette-legacy; do
    tools="$(inspector "$server" --method tools/list)"
    jq -e '.result.tools | any(.name == "load-component")' <<<"$tools" >/dev/null

    components="$(call_tool "$server" list-components '{}')"
    assert_tool_call_succeeded <<<"$components"
    jq -e '
        .result.content[0].text
        | fromjson
        | .components
        | type == "array" and length == 0
    ' <<<"$components" >/dev/null
done

echo "Loading representative Rust and JavaScript components through MCP 2"
fetch_load="$(call_tool wassette-modern load-component "$(jq -cn --arg path "$FETCH_COMPONENT_URI" '{path: $path}')")"
filesystem_load="$(call_tool wassette-modern load-component "$(jq -cn --arg path "$FILESYSTEM_COMPONENT_URI" '{path: $path}')")"
time_load="$(call_tool wassette-modern load-component "$(jq -cn --arg path "$TIME_COMPONENT_URI" '{path: $path}')")"
assert_tool_call_succeeded <<<"$fetch_load"
assert_tool_call_succeeded <<<"$filesystem_load"
assert_tool_call_succeeded <<<"$time_load"

fetch_id="$(jq -r '.result.content[0].text | fromjson | .id' <<<"$fetch_load")"
filesystem_id="$(jq -r '.result.content[0].text | fromjson | .id' <<<"$filesystem_load")"
time_id="$(jq -r '.result.content[0].text | fromjson | .id' <<<"$time_load")"
time_tool="$(jq -r '.result.content[0].text | fromjson | .tools[0]' <<<"$time_load")"

[[ "$fetch_id" == "fetch_rs" ]]
[[ "$filesystem_id" == "filesystem" ]]
[[ -n "$time_id" && "$time_id" != "null" ]]
[[ -n "$time_tool" && "$time_tool" != "null" ]]

for server in wassette-modern wassette-legacy; do
    tools="$(inspector "$server" --method tools/list)"
    jq -e --arg time_tool "$time_tool" '
        .result.tools
        | (any(.name == "fetch"))
          and (any(.name == "read-file"))
          and (any(.name == $time_tool))
    ' <<<"$tools" >/dev/null
done

echo "Calling the JavaScript component through both protocol eras"
for server in wassette-modern wassette-legacy; do
    time_result="$(call_tool "$server" "$time_tool" '{}')"
    assert_tool_call_succeeded <<<"$time_result"
    jq -e '
        .result.content
        | any(
            .text?
            | strings
            | test("[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}.*Z")
        )
    ' <<<"$time_result" >/dev/null
done

echo "Confirming policy denies access before any grant is made"
unauthorized_read="$(call_tool wassette-modern read-file "$(jq -cn --arg path "$TMP_DIR/fs/component.txt" '{path: $path}')")"
assert_component_reported_error <<<"$unauthorized_read"
if jq -e '.result.content[0].text | contains("read through a real filesystem component")' \
    <<<"$unauthorized_read" >/dev/null; then
    echo "error: filesystem component read a file it was never granted" >&2
    exit 1
fi

unauthorized_fetch="$(call_tool wassette-modern fetch "$(jq -cn --arg url "$FIXTURE_URL" '{url: $url}')")"
assert_component_reported_error <<<"$unauthorized_fetch"
if jq -e '.result.content[0].text | contains("served by the Wassette Inspector fixture")' \
    <<<"$unauthorized_fetch" >/dev/null; then
    echo "error: fetch component reached a host it was never granted" >&2
    exit 1
fi

echo "Granting and exercising filesystem access through MCP 2"
storage_args="$(jq -cn \
    --arg component_id "$filesystem_id" \
    --arg uri "fs://$TMP_DIR/fs" \
    '{component_id: $component_id, details: {uri: $uri, access: ["read"]}}')"
call_tool wassette-modern grant-storage-permission "$storage_args" | assert_tool_call_succeeded
read_args="$(jq -cn --arg path "$TMP_DIR/fs/component.txt" '{path: $path}')"
read_result="$(call_tool wassette-modern read-file "$read_args")"
assert_tool_call_succeeded <<<"$read_result"
assert_component_succeeded <<<"$read_result"
jq -e '.result.content[0].text | contains("read through a real filesystem component")' \
    <<<"$read_result" >/dev/null

echo "Revoking the storage grant and confirming access is refused again"
revoke_args="$(jq -cn \
    --arg component_id "$filesystem_id" \
    --arg uri "fs://$TMP_DIR/fs" \
    '{component_id: $component_id, details: {uri: $uri, access: ["read"]}}')"
call_tool wassette-modern revoke-storage-permission "$revoke_args" | assert_tool_call_succeeded
revoked_read="$(call_tool wassette-modern read-file "$read_args")"
assert_component_reported_error <<<"$revoked_read"
if jq -e '.result.content[0].text | contains("read through a real filesystem component")' \
    <<<"$revoked_read" >/dev/null; then
    echo "error: filesystem component still read the file after its grant was revoked" >&2
    exit 1
fi

echo "Granting and exercising network access through legacy MCP"
network_args="$(jq -cn \
    --arg component_id "$fetch_id" \
    '{component_id: $component_id, details: {host: "127.0.0.1"}}')"
call_tool wassette-legacy grant-network-permission "$network_args" | assert_tool_call_succeeded
fetch_args="$(jq -cn --arg url "$FIXTURE_URL" '{url: $url}')"
fetch_result="$(call_tool wassette-legacy fetch "$fetch_args")"
assert_tool_call_succeeded <<<"$fetch_result"
jq -e '.result.content[0].text | contains("served by the Wassette Inspector fixture")' \
    <<<"$fetch_result" >/dev/null

echo "Confirming shared server state from both client eras"
for server in wassette-modern wassette-legacy; do
    components="$(call_tool "$server" list-components '{}')"
    assert_tool_call_succeeded <<<"$components"
    jq -e \
        --arg fetch_id "$fetch_id" \
        --arg filesystem_id "$filesystem_id" \
        --arg time_id "$time_id" '
        .result.content[0].text
        | fromjson
        | .components
        | (any(.id == $fetch_id))
          and (any(.id == $filesystem_id))
          and (any(.id == $time_id))
          and (length == 3)
    ' <<<"$components" >/dev/null
done

echo "Unloading a component and confirming its tools leave the list"
call_tool wassette-modern unload-component "$(jq -cn --arg id "$time_id" '{id: $id}')" \
    | assert_tool_call_succeeded
for server in wassette-modern wassette-legacy; do
    if inspector "$server" --method tools/list \
        | jq -e --arg name "$time_tool" '.result.tools | any(.name == $name)' >/dev/null; then
        echo "error: $time_tool is still listed by $server after unload-component" >&2
        exit 1
    fi
done
remaining="$(call_tool wassette-modern list-components '{}')"
jq -e --arg id "$time_id" '
    .result.content[0].text | fromjson | .components
    | (any(.id == $id) | not) and (length == 2)
' <<<"$remaining" >/dev/null

echo "Checking error paths return errors rather than hanging or panicking"
# The Inspector CLI exits non-zero and prints an extra error envelope when a tool
# reports isError, so tolerate the exit status and assert on the tool result it
# also emits.
missing_load="$(call_tool wassette-modern load-component \
    '{"path": "file:///nonexistent/definitely-not-a-component.wasm"}' 2>&1 || true)"
jq -e -s '
    map(select(.result != null))
    | length > 0
    and (.[0].result.isError == true)
    and (.[0].result.content[0].text | test("does not exist"; "i"))
' <<<"$missing_load" >/dev/null

unknown_tool="$(inspector wassette-modern --method tools/call \
    --tool-name definitely-not-a-tool --tool-args-json '{}' 2>&1 || true)"
if ! grep -qiE "error|not found|unknown" <<<"$unknown_tool"; then
    echo "error: calling an unknown tool did not report an error" >&2
    exit 1
fi

# The server must still be healthy after being handed bad input.
curl --fail --silent "$READY_URL" >/dev/null

echo "Checking --disable-builtin-tools removes the management plane"
LOCKED_PORT="$(python3 - <<'PORT'
import socket
sock = socket.socket()
sock.bind(("127.0.0.1", 0))
print(sock.getsockname()[1])
sock.close()
PORT
)"
LOCKED_CONFIG="$TMP_DIR/inspector-locked.json"
jq --arg url "http://127.0.0.1:$LOCKED_PORT/mcp" '
    (.mcpServers[] | select(.type == "http") | .url) = $url
' "$INSPECTOR_CONFIG_SOURCE" > "$LOCKED_CONFIG"

# Reuse the component directory the main server populated, so this instance
# starts with real components already present and the only difference is that
# the management tools are refused.
RUST_LOG=warn "$WASSETTE_BIN" serve \
    --streamable-http \
    --bind-address "127.0.0.1:$LOCKED_PORT" \
    --component-dir "$TMP_DIR/components" \
    --disable-builtin-tools \
    >"$TMP_DIR/wassette-locked.log" 2>&1 &
LOCKED_PID=$!
wait_for_url "$LOCKED_PID" "http://127.0.0.1:$LOCKED_PORT/ready" \
    "Wassette (--disable-builtin-tools)" "$TMP_DIR/wassette-locked.log"

locked_tools="$(inspector_with_config "$LOCKED_CONFIG" wassette-modern \
    --method tools/list)"

# Every management tool must be gone...
for management_tool in load-component unload-component list-components \
    grant-storage-permission grant-network-permission grant-environment-variable-permission \
    revoke-storage-permission revoke-network-permission revoke-environment-variable-permission \
    reset-permission get-policy search-components; do
    if jq -e --arg name "$management_tool" '.result.tools | any(.name == $name)' \
        <<<"$locked_tools" >/dev/null; then
        echo "error: $management_tool is still exposed with --disable-builtin-tools" >&2
        exit 1
    fi
done

# ...while the components provisioned from disk stay callable.
jq -e '.result.tools | length > 0' <<<"$locked_tools" >/dev/null
jq -e '.result.tools | any(.name == "read-file")' <<<"$locked_tools" >/dev/null

locked_load="$(inspector_with_config "$LOCKED_CONFIG" wassette-modern \
    --method tools/call --tool-name load-component \
    --tool-args-json "$(jq -cn --arg uri "$TIME_COMPONENT_URI" '{path: $uri}')" \
    2>&1 || true)"
if ! grep -qiE "error|not found|unknown" <<<"$locked_load"; then
    echo "error: load-component was accepted despite --disable-builtin-tools" >&2
    exit 1
fi

stop_process "$LOCKED_PID" "Wassette (--disable-builtin-tools)"
LOCKED_PID=""

echo "MCP Inspector dual-era component tests passed"
