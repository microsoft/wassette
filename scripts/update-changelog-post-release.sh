#!/usr/bin/env bash
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT license.

set -euo pipefail

# Script to update CHANGELOG.md after a release
# - Converts [Unreleased] header to the new version with date
# - Adds a new empty [Unreleased] section at the top
# - Updates comparison links
# Usage: update-changelog-post-release.sh <version> <previous-version>
# Example: update-changelog-post-release.sh v0.4.0 v0.3.0

VERSION="${1:-}"
PREVIOUS_VERSION="${2:-}"

if [ -z "$VERSION" ]; then
    echo "Error: Version argument is required"
    echo "Usage: $0 <version> <previous-version>"
    echo "Example: $0 v0.4.0 v0.3.0"
    exit 1
fi

if [ -z "$PREVIOUS_VERSION" ]; then
    echo "Error: Previous version argument is required"
    echo "Usage: $0 <version> <previous-version>"
    echo "Example: $0 v0.4.0 v0.3.0"
    exit 1
fi

# Ensure versions have 'v' prefix
VERSION="${VERSION#v}"
VERSION="v${VERSION}"
PREVIOUS_VERSION="${PREVIOUS_VERSION#v}"
PREVIOUS_VERSION="v${PREVIOUS_VERSION}"

CHANGELOG_FILE="${CHANGELOG_FILE:-CHANGELOG.md}"

if [ ! -f "$CHANGELOG_FILE" ]; then
    echo "Error: $CHANGELOG_FILE not found"
    exit 1
fi

# Get today's date in ISO format
RELEASE_DATE=$(date +%Y-%m-%d)

# Create a temporary file
TMP_FILE=$(mktemp)

# Process the CHANGELOG
awk -v version="$VERSION" -v prev_version="$PREVIOUS_VERSION" -v release_date="$RELEASE_DATE" '
BEGIN {
    in_unreleased = 0
    header_printed = 0
    unreleased_link_updated = 0
}

# Match the [Unreleased] header
/^## \[Unreleased\]$/ {
    if (!header_printed) {
        # Print new empty Unreleased section
        print "## [Unreleased]"
        print ""
        # Print the versioned header with date
        print "## [" version "] - " release_date
        header_printed = 1
        in_unreleased = 1
        next
    }
}

# Stop being in unreleased section when we hit another version header
/^## \[v[0-9]/ {
    in_unreleased = 0
}

# Update the [Unreleased] comparison link
/^\[Unreleased\]:/ && !unreleased_link_updated {
    # Extract the repository URL from the existing link
    match($0, /https:\/\/github\.com\/[^\/]+\/[^\/]+/)
    repo_url = substr($0, RSTART, RLENGTH)
    
    print "[Unreleased]: " repo_url "/compare/" version "...HEAD"
    print "[" version "]: " repo_url "/compare/" prev_version "..." version
    unreleased_link_updated = 1
    next
}

# Print all other lines unchanged
{
    print
}
' "$CHANGELOG_FILE" > "$TMP_FILE"

# Replace the original file
mv "$TMP_FILE" "$CHANGELOG_FILE"

echo "✓ Updated $CHANGELOG_FILE:"
echo "  - Added new empty [Unreleased] section"
echo "  - Updated version to [$VERSION] - $RELEASE_DATE"
echo "  - Updated comparison links"
