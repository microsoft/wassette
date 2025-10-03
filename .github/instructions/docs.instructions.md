---
applyTo: "docs/**/*.md"
---

# Documentation Changes

When working on documentation changes that affect visual presentation or layout, **always use Playwright** to display and capture visual changes. This helps reviewers understand the impact of documentation modifications.

## Using Playwright for Documentation

- Use `playwright-browser_navigate` to load the documentation page
- Use `playwright-browser_take_screenshot` to capture the visual state before and after changes
- Compare screenshots to highlight differences in layout, formatting, or content presentation
- Include screenshots in your progress reports to show visual impact

This ensures that documentation changes are properly validated and reviewers can see the actual visual impact of the modifications.
