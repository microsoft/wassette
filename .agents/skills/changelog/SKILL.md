---
name: changelog
description: Maintain Wassette's Keep a Changelog-style CHANGELOG.md by reviewing a focused change, deciding whether it affects users, and updating the [Unreleased] section with concise, categorized, non-duplicate entries.
allowed-tools: Bash, Read, Write, Edit, Glob, Grep
---

# changelog skill

Maintain `CHANGELOG.md` as part of every user-facing Wassette change. Prefer a
small, accurate entry over a commit-log summary.

## Workflow

1. Read `AGENTS.md` and the `[Unreleased]` section of `CHANGELOG.md`.
2. Inspect only the change being documented:
   - Use the user-provided diff, pull request, or commit range when available.
   - On a pull request branch, compare against the merge base with `main`.
   - Otherwise inspect staged and unstaged changes without including unrelated
     worktree changes.
3. Check whether that focused change adds or modifies files under
   `changelog.d/`. Treat only those fragments as authoritative wording and
   metadata input; do not scan unrelated historical fragments.
4. Decide whether users, operators, component authors, or downstream
   integrators can observe the change.
5. Update the existing `[Unreleased]` section, incorporating any relevant
   fragment content and deduplicating it against existing entries.
6. Review the final diff for placement, duplication, formatting, and scope.

## What needs an entry

Add an entry for:

- New features, commands, options, APIs, integrations, examples, and supported
  platforms.
- Behavior changes, defaults, compatibility changes, and breaking changes.
- Deprecations and removals.
- User-visible bug fixes.
- Security fixes. Avoid unnecessary disclosure before coordinated release.
- Significant installation, deployment, component-publishing, or operational
  changes.

An entry is usually unnecessary for tests, internal refactors, formatting,
comments, routine dependency maintenance, and CI-only changes that do not alter
the delivered product. For such pull requests, use the `skip-changelog` label.
When relevance is genuinely ambiguous, ask rather than guessing.

## Categories

Use these Keep a Changelog headings in this order:

1. `Added`
2. `Changed`
3. `Deprecated`
4. `Removed`
5. `Fixed`
6. `Security`

Reuse an existing heading when possible. Add a missing heading only when the
entry needs it.

## Writing entries

- Write one concise bullet describing the resulting user impact.
- Match the surrounding voice, punctuation, capitalization, and terminology.
- Mention breaking behavior explicitly with `**BREAKING CHANGE**`.
- Combine tightly related changes into one entry.
- Do not duplicate an existing `[Unreleased]` entry.
- When a focused fragment exists, prefer its wording over a summary generated
  from the implementation diff. Make only minimal edits needed for surrounding
  style, clarity, or deduplication.
- Derive the pull request number and category from a fragment named
  `<pr_number>.<type>.md`. Map `feature` to `Added`, `bugfix` to `Fixed`,
  `removal` to `Removed`, and `doc` or `misc` to `Changed`.
- Add a pull request link only when its number is known, using
  `([#N](https://github.com/microsoft/wassette/pull/N))`.
- Do not invent a pull request number or leave a placeholder.

## Wassette release rules

- Edit only the current `[Unreleased]` section.
- Do not add a version or release date; `release.yml` owns that transition.
- Do not create `changelog.d/` fragments. The release pipeline reads
  `CHANGELOG.md` directly and does not run Towncrier.
- If the focused change already contains fragments, use them as input and
  leave them unchanged.
- Do not rewrite historical releases unless the user explicitly requests a
  correction.
- Do not include routine dependency bumps unless they fix a user-visible issue,
  change compatibility, or address a security advisory.

## Validation checklist

- The entry is inside `[Unreleased]`, before the first released version.
- The category matches the type of user impact.
- Focused fragment wording and filename metadata were preserved where possible.
- The wording is understandable without reading the implementation diff.
- Existing headings, blank lines, links, and historical releases are preserved.
- All `changelog.d/` files are unchanged.
- The diff contains no unrelated changelog edits.
- If no entry was needed, report that the pull request needs the
  `skip-changelog` label.
