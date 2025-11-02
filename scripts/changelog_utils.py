#!/usr/bin/env python3
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT license.

"""Utilities for managing CHANGELOG.md during releases."""

import re
import sys
from datetime import date
from pathlib import Path
from typing import Optional


def extract_changelog_content(changelog_path: Path, version: str) -> str:
    """
    Extract changelog content for a specific version.
    
    Falls back to extracting from [Unreleased] section if the version
    is not found, which is the expected state before a release is tagged.
    
    Args:
        changelog_path: Path to CHANGELOG.md file
        version: Version to extract (e.g., "v0.4.0" or "0.4.0")
        
    Returns:
        The changelog content for the specified version (without the header)
        
    Raises:
        FileNotFoundError: If changelog file doesn't exist
        ValueError: If version not found and [Unreleased] section is empty
    """
    if not changelog_path.exists():
        raise FileNotFoundError(f"Changelog not found: {changelog_path}")
    
    # Normalize version (remove 'v' prefix if present)
    version_normalized = version.lstrip('v')
    
    content = changelog_path.read_text()
    lines = content.split('\n')
    
    # Find the version header
    version_pattern = re.compile(rf'^## \[v?{re.escape(version_normalized)}\]')
    
    found = False
    output_lines = []
    
    for i, line in enumerate(lines):
        # Start collecting when we find the version header
        if version_pattern.match(line):
            found = True
            continue  # Skip the header itself
            
        # Stop at next version header or comparison links
        if found:
            if line.startswith('## [') or line.startswith('[Unreleased]:') or line.startswith('[v'):
                break
            output_lines.append(line)
    
    # If version not found, try to extract from [Unreleased] section
    if not found:
        output_lines = []
        for i, line in enumerate(lines):
            if line == '## [Unreleased]':
                found = True
                continue  # Skip the header itself
                
            if found:
                # Stop at next version header or comparison links
                if line.startswith('## [') or line.startswith('[Unreleased]:') or line.startswith('[v'):
                    break
                output_lines.append(line)
        
        if not found or not any(line.strip() for line in output_lines):
            raise ValueError(
                f"Version {version} not found in {changelog_path} and "
                f"[Unreleased] section is empty or missing"
            )
    
    # Remove trailing empty lines
    while output_lines and not output_lines[-1].strip():
        output_lines.pop()
    
    return '\n'.join(output_lines)


def update_changelog_post_release(
    changelog_path: Path,
    new_version: str,
    previous_version: str,
    release_date: Optional[str] = None
) -> None:
    """
    Update CHANGELOG.md after a release.
    
    Converts [Unreleased] section to the new version with date,
    adds a new empty [Unreleased] section, and updates comparison links.
    
    Args:
        changelog_path: Path to CHANGELOG.md file
        new_version: New version being released (e.g., "v0.4.0")
        previous_version: Previous version (e.g., "v0.3.0")
        release_date: Release date in ISO format (defaults to today)
        
    Raises:
        FileNotFoundError: If changelog file doesn't exist
    """
    if not changelog_path.exists():
        raise FileNotFoundError(f"Changelog not found: {changelog_path}")
    
    # Ensure versions have 'v' prefix
    new_version = new_version if new_version.startswith('v') else f'v{new_version}'
    previous_version = previous_version if previous_version.startswith('v') else f'v{previous_version}'
    
    if release_date is None:
        release_date = date.today().isoformat()
    
    content = changelog_path.read_text()
    lines = content.split('\n')
    output_lines = []
    
    unreleased_updated = False
    unreleased_link_updated = False
    
    for line in lines:
        # Update [Unreleased] header to versioned header with date
        if line == '## [Unreleased]' and not unreleased_updated:
            output_lines.append('## [Unreleased]')
            output_lines.append('')
            output_lines.append(f'## [{new_version}] - {release_date}')
            unreleased_updated = True
            continue
        
        # Update [Unreleased] comparison link
        if line.startswith('[Unreleased]:') and not unreleased_link_updated:
            # Extract repository URL
            match = re.search(r'(https://github\.com/[^/]+/[^/]+)', line)
            if match:
                repo_url = match.group(1)
                output_lines.append(f'[Unreleased]: {repo_url}/compare/{new_version}...HEAD')
                output_lines.append(f'[{new_version}]: {repo_url}/compare/{previous_version}...{new_version}')
                unreleased_link_updated = True
                continue
        
        output_lines.append(line)
    
    changelog_path.write_text('\n'.join(output_lines))


def main():
    """Command-line interface for changelog utilities."""
    if len(sys.argv) < 2:
        print("Usage:")
        print("  Extract: changelog_utils.py extract <version> [changelog_path]")
        print("  Update:  changelog_utils.py update <new_version> <prev_version> [changelog_path]")
        sys.exit(1)
    
    command = sys.argv[1]
    
    if command == 'extract':
        if len(sys.argv) < 3:
            print("Error: Version required")
            print("Usage: changelog_utils.py extract <version> [changelog_path]")
            sys.exit(1)
        
        version = sys.argv[2]
        changelog_path = Path(sys.argv[3]) if len(sys.argv) > 3 else Path('CHANGELOG.md')
        
        try:
            content = extract_changelog_content(changelog_path, version)
            print(content)
        except (FileNotFoundError, ValueError) as e:
            print(f"Error: {e}", file=sys.stderr)
            sys.exit(1)
    
    elif command == 'update':
        if len(sys.argv) < 4:
            print("Error: New version and previous version required")
            print("Usage: changelog_utils.py update <new_version> <prev_version> [changelog_path]")
            sys.exit(1)
        
        new_version = sys.argv[2]
        prev_version = sys.argv[3]
        changelog_path = Path(sys.argv[4]) if len(sys.argv) > 4 else Path('CHANGELOG.md')
        
        try:
            update_changelog_post_release(changelog_path, new_version, prev_version)
            print(f"✓ Updated {changelog_path}:")
            print(f"  - Added new empty [Unreleased] section")
            print(f"  - Updated version to [{new_version}] with release date")
            print(f"  - Updated comparison links")
        except FileNotFoundError as e:
            print(f"Error: {e}", file=sys.stderr)
            sys.exit(1)
    
    else:
        print(f"Error: Unknown command '{command}'")
        print("Valid commands: extract, update")
        sys.exit(1)


if __name__ == '__main__':
    main()
