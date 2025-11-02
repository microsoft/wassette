# Agentic Workflows

This repository uses [GitHub Agentic Workflows](https://githubnext.github.io/gh-aw/) (@githubnext/gh-aw) to automate tasks with AI agents. These are markdown files with YAML frontmatter for configuration and natural language instructions that compile to standard GitHub Actions workflows using `gh aw compile`.

## Workflows in This Repository

- **Issue Triage Bot** (`.github/workflows/issue-triage.md`) - Automatically analyzes and labels new issues when they are opened or reopened.
- **Scout Research Agent** (`.github/workflows/scout.md`) - Responds to `/scout` commands to research topics using web search and provide comprehensive reports.
- **CI Doctor** (`.github/workflows/ci-doctor.md`) - Automatically investigates and diagnoses CI failures when the Rust workflow completes on main.
- **Release Doctor** (`.github/workflows/release-doctor.md`) - Monitors the entire release pipeline (prepare-release, release, update-package-manifests) and automatically creates diagnostic issues when failures occur, including verification of release binaries, CHANGELOG synchronization, version consistency, and package manifest updates.

## Creating Your Own Agentic Workflows

Create a markdown file in `.github/workflows/` with YAML frontmatter (triggers, permissions, tools, engine) followed by natural language instructions. Key configuration:

- **Triggers:** Standard events (`issues`, `pull_request`, `push`) or command triggers (`command: { name: bot-name }`)
- **Permissions:** Request only what you need (`contents: read`, `issues: write`, etc.)
- **Tools:** Control AI access (`github`, `bash`, `edit`, `web-fetch`, `web-search`)
- **Engines:** Choose `claude` (default), `copilot`, or `codex`

Compile with `gh aw compile` to generate the `.lock.yml` file that GitHub Actions executes.

## Monitoring and Debugging

- **View Logs:** `gh aw logs [workflow-name]` with optional filters (`--engine`, `--start-date`)
- **Inspect MCP:** `gh aw mcp inspect [workflow-name]` to view MCP server configurations and tools

## Resources

- **Official Documentation:** [gh-aw docs](https://githubnext.github.io/gh-aw/)
- **Installation:** `gh extension install githubnext/gh-aw`
- **Instructions File:** `.github/instructions/github-agentic-workflows.instructions.md`
- **Example Workflows:** `.github/workflows/*.md`

## Contributing

To add a new workflow: Create `workflow-name.md` in `.github/workflows/`, test with `workflow_dispatch`, run `gh aw compile`, commit both `.md` and `.lock.yml` files, and update `CHANGELOG.md`.
