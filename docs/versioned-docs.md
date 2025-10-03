# Versioned Documentation

The Wassette documentation supports multiple versions, allowing users to view documentation for different releases and the latest development version.

## Version Structure

The documentation is organized as follows:

- **`/wassette/latest/`** - Documentation built from the `main` branch, representing the latest development version
- **`/wassette/vX.Y.Z/`** - Documentation built from release tags (e.g., `v0.3.0`, `v0.4.0`)
- **`/wassette/`** - Root redirect that automatically forwards to `/wassette/latest/`

## Version Dropdown

Each documentation page includes a version dropdown in the header navigation bar. This dropdown allows you to:

1. **Switch between versions** - Select any available version from the dropdown
2. **Maintain context** - When switching versions, the system attempts to navigate to the same page in the new version
3. **Fallback behavior** - If the current page doesn't exist in the target version, you'll be redirected to that version's index page

## How It Works

### For Developers

The version switching functionality is implemented using:

1. **JavaScript** (`theme/version-picker.js`) - Handles version detection, dropdown creation, and navigation
2. **CSS** (`theme/version-picker.css`) - Styles the version dropdown to match the documentation theme
3. **Version Index** (`versions.json`) - Maintains a list of all available documentation versions

### For CI/CD

The documentation workflow (`.github/workflows/docs.yml`) automatically:

1. **On main branch pushes**: Builds and publishes documentation to `/wassette/latest/`
2. **On tag pushes** (tags matching `v*`): Builds and publishes documentation to `/wassette/vX.Y.Z/`
3. **Updates** the `versions.json` file to include newly published versions
4. **Preserves** all existing versions during each deployment

## Adding a New Version

New versions are added automatically when you:

1. Push changes to the `main` branch - updates `/wassette/latest/`
2. Create and push a new tag starting with `v` (e.g., `v0.4.0`) - creates `/wassette/v0.4.0/`

The GitHub Actions workflow handles all the necessary steps:
- Building the documentation with mdBook
- Creating the versioned directory structure
- Updating the `versions.json` index
- Deploying to GitHub Pages

## Configuration

The version picker is configured in `docs/book.toml`:

```toml
[output.html]
additional-css = ["theme/version-picker.css"]
additional-js = ["theme/version-picker.js"]
```

## Maintenance

### Removing Old Versions

To remove old documentation versions:

1. Manually edit the `versions.json` file in the `gh-pages` branch
2. Remove the corresponding version directory from `gh-pages`
3. The next deployment will use the updated version list

### Version Ordering

Versions appear in the dropdown in the order they're listed in `versions.json`:
- `latest` always appears first
- Release versions (e.g., `v0.4.0`, `v0.3.0`) are listed in the order they were published
- You can manually reorder versions by editing `versions.json` in the `gh-pages` branch
