---
name: documentation
description: Build, serve, and write Wassette's mdBook documentation under docs/ — concise, code-first prose, the multi-version URL layout, navigation in SUMMARY.md, and Playwright screenshots for visual changes. Use when editing docs/ pages or previewing the documentation site.
allowed-tools: Bash, Read, Write, Edit, Glob, Grep
---

# documentation skill

Wassette documentation lives under `docs/` and is built with
[mdBook](https://rust-lang.github.io/mdBook/). Related rules also live in
`.github/instructions/docs.instructions.md`.

## Build and serve

```bash
just docs-build    # Build static HTML to docs/book/
just docs-watch    # Serve at http://localhost:3000 with live reload
just docs-serve    # Serve and open in the browser
```

Or directly: `cd docs && mdbook serve` / `mdbook build`.

## URL structure

Production uses a multi-version layout that `mdbook serve` does not reproduce
locally:

- **Local**: open `http://localhost:3000/overview.html` (or a specific page).
  The version picker and the root redirect only work in production.
- **Production**: `https://microsoft.github.io/wassette/latest/` (and
  `/v0.3.0/` for releases).

## Writing style

- Prefer working code examples over long prose; explain details second.
- Keep pages focused and concise; every sentence should add value.
- Use active voice and present tense; define acronyms on first use.
- Always set the language on code blocks; use descriptive link text.

## Visual changes

For changes that affect layout or presentation, use Playwright to capture
before/after screenshots and include them in progress reports so reviewers can
see the impact.

## Adding a page

1. Create the markdown file in the right `docs/` subdirectory.
2. Register it in `docs/SUMMARY.md` so it appears in navigation.
3. Match existing structure and formatting; test all links.
4. Preview locally with `just docs-serve`.
