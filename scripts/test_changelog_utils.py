#!/usr/bin/env python3
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT license.

"""Unit tests for changelog_utils module."""

import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

from changelog_utils import extract_changelog_content, update_changelog_post_release


class TestChangelogUtils(unittest.TestCase):
    """Test cases for changelog utilities."""
    
    def setUp(self):
        """Create a temporary directory for test files."""
        self.temp_dir = TemporaryDirectory()
        self.changelog_path = Path(self.temp_dir.name) / 'CHANGELOG.md'
    
    def tearDown(self):
        """Clean up temporary directory."""
        self.temp_dir.cleanup()
    
    def test_extract_existing_version(self):
        """Test extracting content for an existing version."""
        self.changelog_path.write_text("""# Changelog

## [Unreleased]

### Added
- Feature A

## [v0.3.0] - 2025-10-03

### Added
- Feature X
- Feature Y

### Fixed
- Bug Z

## [v0.2.0] - 2025-09-01

### Added
- Feature W

[Unreleased]: https://github.com/test/repo/compare/v0.3.0...HEAD
[v0.3.0]: https://github.com/test/repo/compare/v0.2.0...v0.3.0
""")
        
        content = extract_changelog_content(self.changelog_path, 'v0.3.0')
        
        self.assertIn('### Added', content)
        self.assertIn('- Feature X', content)
        self.assertIn('- Feature Y', content)
        self.assertIn('### Fixed', content)
        self.assertIn('- Bug Z', content)
        self.assertNotIn('## [v0.3.0]', content)
        self.assertNotIn('## [v0.2.0]', content)
    
    def test_extract_version_without_v_prefix(self):
        """Test extracting with version without 'v' prefix."""
        self.changelog_path.write_text("""# Changelog

## [v0.3.0] - 2025-10-03

### Added
- Feature X

## [v0.2.0] - 2025-09-01

### Added
- Feature W
""")
        
        content = extract_changelog_content(self.changelog_path, '0.3.0')
        self.assertIn('### Added', content)
        self.assertIn('- Feature X', content)
    
    def test_extract_nonexistent_version(self):
        """Test extracting content for a version that doesn't exist."""
        self.changelog_path.write_text("""# Changelog

## [v0.3.0] - 2025-10-03

### Added
- Feature X
""")
        
        with self.assertRaises(ValueError):
            extract_changelog_content(self.changelog_path, 'v0.9.9')
    
    def test_extract_missing_file(self):
        """Test extracting from a non-existent file."""
        with self.assertRaises(FileNotFoundError):
            extract_changelog_content(Path('/nonexistent/file.md'), 'v0.3.0')
    
    def test_extract_falls_back_to_unreleased(self):
        """Test extracting falls back to [Unreleased] when version not found."""
        self.changelog_path.write_text("""# Changelog

## [Unreleased]

### Added
- Feature A
- Feature B

### Fixed
- Bug C

## [v0.3.0] - 2025-10-03

### Added
- Feature X
""")
        
        # Version v0.4.0 doesn't exist, should extract from [Unreleased]
        content = extract_changelog_content(self.changelog_path, 'v0.4.0')
        
        self.assertIn('### Added', content)
        self.assertIn('- Feature A', content)
        self.assertIn('- Feature B', content)
        self.assertIn('### Fixed', content)
        self.assertIn('- Bug C', content)
        self.assertNotIn('## [Unreleased]', content)
        self.assertNotIn('## [v0.3.0]', content)
    
    def test_extract_empty_unreleased_raises_error(self):
        """Test that extracting non-existent version with empty [Unreleased] raises error."""
        self.changelog_path.write_text("""# Changelog

## [Unreleased]

## [v0.3.0] - 2025-10-03

### Added
- Feature X
""")
        
        # Version v0.9.9 doesn't exist and [Unreleased] is empty
        with self.assertRaises(ValueError) as context:
            extract_changelog_content(self.changelog_path, 'v0.9.9')
        
        self.assertIn('Unreleased', str(context.exception))
        self.assertIn('empty', str(context.exception))
    
    def test_extract_prefers_existing_version_over_unreleased(self):
        """Test that extraction prefers existing version over [Unreleased]."""
        self.changelog_path.write_text("""# Changelog

## [Unreleased]

### Added
- Future Feature

## [v0.3.0] - 2025-10-03

### Added
- Feature X
- Feature Y

## [v0.2.0] - 2025-09-01

### Added
- Feature W
""")
        
        # Should extract v0.3.0 content, not [Unreleased]
        content = extract_changelog_content(self.changelog_path, 'v0.3.0')
        
        self.assertIn('- Feature X', content)
        self.assertIn('- Feature Y', content)
        self.assertNotIn('- Future Feature', content)
    
    def test_update_changelog_adds_new_unreleased(self):
        """Test that update adds a new empty Unreleased section."""
        self.changelog_path.write_text("""# Changelog

## [Unreleased]

### Added
- Feature A
- Feature B

### Fixed
- Bug C

## [v0.3.0] - 2025-10-03

### Added
- Feature X

[Unreleased]: https://github.com/test/repo/compare/v0.3.0...HEAD
[v0.3.0]: https://github.com/test/repo/compare/v0.2.0...v0.3.0
""")
        
        update_changelog_post_release(
            self.changelog_path, 
            'v0.4.0', 
            'v0.3.0',
            '2025-10-16'
        )
        
        content = self.changelog_path.read_text()
        
        # Check for new empty Unreleased section
        self.assertIn('## [Unreleased]\n\n## [v0.4.0] - 2025-10-16', content)
        
        # Check that features are now under v0.4.0
        lines = content.split('\n')
        unreleased_idx = lines.index('## [Unreleased]')
        v040_idx = lines.index('## [v0.4.0] - 2025-10-16')
        
        # Unreleased should be empty (only blank line between headers)
        self.assertEqual(v040_idx - unreleased_idx, 2)
        
        # Features should be under v0.4.0
        feature_a_idx = next(i for i, line in enumerate(lines) if '- Feature A' in line)
        self.assertGreater(feature_a_idx, v040_idx)
    
    def test_update_changelog_updates_links(self):
        """Test that update correctly updates comparison links."""
        self.changelog_path.write_text("""# Changelog

## [Unreleased]

### Added
- Feature A

## [v0.3.0] - 2025-10-03

### Added
- Feature X

[Unreleased]: https://github.com/test/repo/compare/v0.3.0...HEAD
[v0.3.0]: https://github.com/test/repo/compare/v0.2.0...v0.3.0
""")
        
        update_changelog_post_release(
            self.changelog_path,
            'v0.4.0',
            'v0.3.0',
            '2025-10-16'
        )
        
        content = self.changelog_path.read_text()
        
        # Check that Unreleased link points to v0.4.0
        self.assertIn('[Unreleased]: https://github.com/test/repo/compare/v0.4.0...HEAD', content)
        
        # Check that v0.4.0 link is added
        self.assertIn('[v0.4.0]: https://github.com/test/repo/compare/v0.3.0...v0.4.0', content)
        
        # Check that v0.3.0 link is still there
        self.assertIn('[v0.3.0]: https://github.com/test/repo/compare/v0.2.0...v0.3.0', content)
    
    def test_update_changelog_handles_versions_without_v(self):
        """Test that update handles versions without 'v' prefix."""
        self.changelog_path.write_text("""# Changelog

## [Unreleased]

### Added
- Feature A

[Unreleased]: https://github.com/test/repo/compare/v0.3.0...HEAD
""")
        
        # Pass versions without 'v' prefix
        update_changelog_post_release(
            self.changelog_path,
            '0.4.0',  # No 'v'
            '0.3.0',  # No 'v'
            '2025-10-16'
        )
        
        content = self.changelog_path.read_text()
        
        # Should add 'v' prefix automatically
        self.assertIn('## [v0.4.0] - 2025-10-16', content)
        self.assertIn('[Unreleased]: https://github.com/test/repo/compare/v0.4.0...HEAD', content)
        self.assertIn('[v0.4.0]: https://github.com/test/repo/compare/v0.3.0...v0.4.0', content)
    
    def test_update_changelog_missing_file(self):
        """Test updating a non-existent file."""
        with self.assertRaises(FileNotFoundError):
            update_changelog_post_release(
                Path('/nonexistent/file.md'),
                'v0.4.0',
                'v0.3.0'
            )


if __name__ == '__main__':
    unittest.main()
