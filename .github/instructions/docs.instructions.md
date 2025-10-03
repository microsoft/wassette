---
applyTo: "docs/**/*.md"
---

# Documentation Changes

When working on documentation changes that affect visual presentation or layout, **always use Playwright** to display and capture visual changes. This helps reviewers understand the impact of documentation modifications.

## Running the Documentation Locally

The project uses [mdbook](https://rust-lang.github.io/mdBook/) for documentation. Use the following commands:

- **Build the docs**: `just docs-build` - Builds the documentation to `docs/book/`
- **Serve with auto-reload**: `just docs-watch` - Serves the docs at `http://localhost:3000` with live reload
- **Serve and open browser**: `just docs-serve` - Serves the docs and automatically opens in your browser

Alternatively, you can use mdbook directly:
```bash
cd docs
mdbook serve        # Serve with live reload
mdbook build        # Build static HTML
```

## Using Playwright for Documentation

- Use `playwright-browser_navigate` to load the documentation page
- Use `playwright-browser_take_screenshot` to capture the visual state before and after changes
- Compare screenshots to highlight differences in layout, formatting, or content presentation
- Include screenshots in your progress reports to show visual impact

This ensures that documentation changes are properly validated and reviewers can see the actual visual impact of the modifications.
