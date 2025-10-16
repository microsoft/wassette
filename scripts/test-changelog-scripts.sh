#!/usr/bin/env bash
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT license.

# Integration test for CHANGELOG scripts

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Create test directory
TEST_DIR=$(mktemp -d)
trap 'rm -rf "$TEST_DIR"' EXIT

echo "Test directory: $TEST_DIR"

# Create a test CHANGELOG
cat > "$TEST_DIR/CHANGELOG.md" << 'EOF'
# Changelog

## [Unreleased]

### Added
- Feature A
- Feature B

### Fixed
- Bug fix C

## [v0.3.0] - 2025-10-03

### Added
- Feature X

[Unreleased]: https://github.com/microsoft/wassette/compare/v0.3.0...HEAD
[v0.3.0]: https://github.com/microsoft/wassette/compare/v0.2.0...v0.3.0
EOF

echo "=== Test 1: Extract changelog for existing version ==="
CHANGELOG_FILE="$TEST_DIR/CHANGELOG.md" "$PROJECT_ROOT/scripts/extract-changelog.sh" v0.3.0
echo ""

echo "=== Test 2: Update changelog post-release ==="
CHANGELOG_FILE="$TEST_DIR/CHANGELOG.md" "$PROJECT_ROOT/scripts/update-changelog-post-release.sh" v0.4.0 v0.3.0
echo ""

echo "=== Test 3: Verify updated CHANGELOG ==="
cat "$TEST_DIR/CHANGELOG.md"
echo ""

echo "=== Test 4: Extract changelog for new version ==="
CHANGELOG_FILE="$TEST_DIR/CHANGELOG.md" "$PROJECT_ROOT/scripts/extract-changelog.sh" v0.4.0
echo ""

echo "✓ All tests passed!"
