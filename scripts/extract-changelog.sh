#!/usr/bin/env bash
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT license.

set -euo pipefail

# Script to extract changelog content for a specific version
# Usage: extract-changelog.sh <version>
# Example: extract-changelog.sh v0.4.0

VERSION="${1:-}"

if [ -z "$VERSION" ]; then
    echo "Error: Version argument is required"
    echo "Usage: $0 <version>"
    echo "Example: $0 v0.4.0"
    exit 1
fi

# Remove 'v' prefix if present for matching
VERSION_NO_V="${VERSION#v}"

CHANGELOG_FILE="${CHANGELOG_FILE:-CHANGELOG.md}"

if [ ! -f "$CHANGELOG_FILE" ]; then
    echo "Error: $CHANGELOG_FILE not found"
    exit 1
fi

# Extract content between the version header and the next version header or comparison links
# This uses awk to capture the content for the specific version
awk -v version="$VERSION_NO_V" '
BEGIN { found=0; printing=0 }

# Match the version header (with or without date)
/^## \[v?'"$VERSION_NO_V"'\]/ {
    found=1
    printing=1
    next
}

# Stop at the next version header or comparison links section
/^## \[/ {
    if (printing) {
        printing=0
        exit
    }
}

/^\[Unreleased\]:/ || /^\[v[0-9]/ {
    if (printing) {
        printing=0
        exit
    }
}

# Print lines when we are in the target version section
printing {
    print
}

END {
    if (!found) {
        print "Error: Version " version " not found in CHANGELOG.md" > "/dev/stderr"
        exit 1
    }
}
' "$CHANGELOG_FILE"
