#!/usr/bin/env bash
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT license.

set -euo pipefail

# Script to validate component URIs in component-registry.json
# Pull requests validate registry structure without dereferencing unpublished OCI
# artifacts. Successful example publication runs validate every registered URI.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
REGISTRY_FILE="$REPO_ROOT/component-registry.json"
WASSETTE_BIN="$REPO_ROOT/bin/wassette"
TMP_DIR=$(mktemp -d)

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo "=== Component Registry Validation ==="
echo ""

# Ensure temporary files are removed even when pull-request validation exits
# before the server cleanup trap is installed.
trap 'rm -rf "$TMP_DIR"' EXIT INT TERM

# Validate the registry shape before attempting any remote loads.
if ! jq -e '
    type == "array" and
    all(.[]; (
        ((.name | type) == "string" and (try (.name | length > 0) catch false)) and
        ((.description | type) == "string" and (try (.description | length > 0) catch false)) and
        ((.uri | type) == "string" and (try (.uri | startswith("oci://")) catch false))
    ))
' "$REGISTRY_FILE" > /dev/null; then
    echo -e "${RED}Error: component-registry.json must contain non-empty name, description, and oci:// URI strings${NC}"
    exit 1
fi

DUPLICATE_URIS=$(jq -r '.[].uri' "$REGISTRY_FILE" | sort | uniq -d)
if [[ -n "$DUPLICATE_URIS" ]]; then
    echo -e "${RED}Error: component-registry.json contains duplicate URIs:${NC}"
    printf '%s\n' "$DUPLICATE_URIS" | sed 's/^/  - /'
    exit 1
fi

if [[ "${VALIDATE_REMOTE_COMPONENTS:-false}" != "true" ]]; then
    echo -e "${GREEN}✓ Registry structure is valid.${NC}"
    echo "OCI component loading is deferred until the Publish Examples workflow completes on main."
    exit 0
fi

# Cleanup function
cleanup() {
    local exit_code=$?
    echo ""
    echo "Cleaning up..."

    # Kill wassette server if running
    if [[ -n "${WASSETTE_PID:-}" ]] && kill -0 "$WASSETTE_PID" 2>/dev/null; then
        echo "Stopping wassette server (PID: $WASSETTE_PID)..."
        kill "$WASSETTE_PID" 2>/dev/null || true
        wait "$WASSETTE_PID" 2>/dev/null || true
    fi

    # Clean up temporary files
    rm -rf "$TMP_DIR"

    exit "$exit_code"
}

trap cleanup EXIT INT TERM

# Check if wassette binary exists
if [[ ! -f "$WASSETTE_BIN" ]]; then
    echo -e "${RED}Error: wassette binary not found at $WASSETTE_BIN${NC}"
    echo "Please run 'just build' first"
    exit 1
fi

# Get the registry from main branch for pull-request comparisons.
MAIN_REGISTRY="$TMP_DIR/registry-main.json"
if git rev-parse --git-dir > /dev/null 2>&1; then
    if git show origin/main:component-registry.json > "$MAIN_REGISTRY" 2>/dev/null; then
        echo "Retrieved component-registry.json from main branch"
    else
        echo -e "${YELLOW}Warning: Could not retrieve registry from main branch, will validate all components${NC}"
        echo "[]" > "$MAIN_REGISTRY"
    fi
else
    echo -e "${YELLOW}Warning: Not in a git repository, will validate all components${NC}"
    echo "[]" > "$MAIN_REGISTRY"
fi

# Extract URIs from both registries
CURRENT_URIS="$TMP_DIR/current-uris.txt"
MAIN_URIS="$TMP_DIR/main-uris.txt"

jq -r '.[].uri' "$REGISTRY_FILE" | sort > "$CURRENT_URIS"
jq -r '.[].uri' "$MAIN_REGISTRY" | sort > "$MAIN_URIS"

# Select URIs to validate: new/modified entries on pull requests, or every
# entry after the examples have been published on main.
NEW_OR_MODIFIED="$TMP_DIR/to-validate.txt"
if [[ "${VALIDATE_REMOTE_COMPONENTS:-false}" == "true" ]]; then
    cp "$CURRENT_URIS" "$NEW_OR_MODIFIED"
else
    comm -13 "$MAIN_URIS" "$CURRENT_URIS" > "$NEW_OR_MODIFIED"
fi

# Count components to validate
VALIDATE_COUNT=$(wc -l < "$NEW_OR_MODIFIED" | tr -d ' ')

if [[ "$VALIDATE_COUNT" -eq 0 ]]; then
    echo -e "${GREEN}✓ No new or modified components found. Nothing to validate.${NC}"
    exit 0
fi

echo "Found $VALIDATE_COUNT component(s) to validate:"
cat "$NEW_OR_MODIFIED" | sed 's/^/  - /'
echo ""

# Start wassette server in background with Streamable HTTP transport
WASSETTE_LOG="$TMP_DIR/wassette.log"
echo "Starting wassette server..."
RUST_LOG=warn "$WASSETTE_BIN" serve --streamable-http > "$WASSETTE_LOG" 2>&1 &
WASSETTE_PID=$!

# Wait for server to be ready
echo "Waiting for wassette server to start..."
MAX_WAIT=30
WAITED=0
while [[ $WAITED -lt $MAX_WAIT ]]; do
    if timeout 2 curl --fail --silent http://127.0.0.1:9001/ready >/dev/null 2>&1; then
        echo "Wassette server is ready (PID: $WASSETTE_PID)"
        break
    fi
    sleep 1
    WAITED=$((WAITED + 1))

    # Check if server process is still running
    if ! kill -0 "$WASSETTE_PID" 2>/dev/null; then
        echo -e "${RED}Error: wassette server failed to start${NC}"
        echo "Server log:"
        cat "$WASSETTE_LOG"
        exit 1
    fi
done

if [[ $WAITED -eq $MAX_WAIT ]]; then
    echo -e "${RED}Error: wassette server failed to start within ${MAX_WAIT}s${NC}"
    echo "Server log:"
    cat "$WASSETTE_LOG"
    exit 1
fi

echo ""
echo "=== Validating Components ==="
echo ""

# Validate each component
FAILED_COUNT=0
FAILED_COMPONENTS=()

while IFS= read -r uri; do
    if [[ -z "$uri" ]]; then
        continue
    fi

    # Get component name for better output
    COMPONENT_NAME=$(jq -r --arg uri "$uri" '.[] | select(.uri == $uri) | .name' "$REGISTRY_FILE")

    echo -n "Validating: $COMPONENT_NAME ($uri) ... "

    # Attempt to load the component using MCP protocol
    LOAD_RESULT="$TMP_DIR/load-result.json"
    if npx -y @modelcontextprotocol/inspector \
        --cli http://127.0.0.1:9001/mcp \
        --transport http \
        --method tools/call \
        --tool-name load-component \
        --tool-arg "path=$uri" > "$LOAD_RESULT" 2>&1; then

        # Check if the result indicates success
        if jq -e '.content[0].text | contains("component loaded successfully") or contains("component reloaded successfully")' "$LOAD_RESULT" > /dev/null 2>&1; then
            echo -e "${GREEN}✓ PASS${NC}"
        else
            echo -e "${RED}✗ FAIL${NC}"
            echo "  Result:"
            jq -r '.content[0].text // .error // "Unknown error"' "$LOAD_RESULT" | sed 's/^/    /'
            FAILED_COUNT=$((FAILED_COUNT + 1))
            FAILED_COMPONENTS+=("$COMPONENT_NAME ($uri)")
        fi
    else
        echo -e "${RED}✗ FAIL${NC}"
        echo "  Failed to load component"
        if [[ -f "$LOAD_RESULT" ]]; then
            cat "$LOAD_RESULT" | sed 's/^/    /'
        fi
        FAILED_COUNT=$((FAILED_COUNT + 1))
        FAILED_COMPONENTS+=("$COMPONENT_NAME ($uri)")
    fi
done < "$NEW_OR_MODIFIED"

echo ""
echo "=== Validation Results ==="
echo ""

PASSED_COUNT=$((VALIDATE_COUNT - FAILED_COUNT))
echo "Validated: $VALIDATE_COUNT component(s)"
echo -e "Passed: ${GREEN}$PASSED_COUNT${NC}"
echo -e "Failed: ${RED}$FAILED_COUNT${NC}"

if [[ $FAILED_COUNT -gt 0 ]]; then
    echo ""
    echo -e "${RED}The following components failed validation:${NC}"
    for component in "${FAILED_COMPONENTS[@]}"; do
        echo "  - $component"
    done
    echo ""
    echo -e "${RED}✗ Validation failed. Please fix the component URIs above.${NC}"
    exit 1
fi

echo ""
echo -e "${GREEN}✓ All components validated successfully!${NC}"
exit 0
