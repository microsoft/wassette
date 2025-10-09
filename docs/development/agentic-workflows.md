# Agentic Workflows

This repository uses [GitHub Agentic Workflows](https://githubnext.github.io/gh-aw/) (@githubnext/gh-aw) to automate various tasks using AI-powered workflows. Agentic workflows are markdown files that combine natural language instructions with GitHub Actions capabilities to create intelligent automation.

## What are Agentic Workflows?

Agentic workflows are a novel way to define GitHub Actions workflows using natural language. Instead of writing complex YAML configurations, you write markdown files with:

- **YAML frontmatter** for configuration (triggers, permissions, tools)
- **Natural language instructions** for what the AI agent should do
- **GitHub context variables** to access issue/PR data

The workflows are compiled into standard GitHub Actions workflows using the `gh aw compile` command.

## Workflows in This Repository

### Issue Triage Bot

**File:** `.github/workflows/issue-triage.md`

Automatically triages new issues by:
- Analyzing issue content
- Selecting appropriate labels
- Checking for duplicates
- Providing triage notes

**Trigger:** Runs when issues are opened or reopened

**Example usage:** Simply create a new issue, and the bot will automatically analyze it and add relevant labels.

### Scout Research Agent

**File:** `.github/workflows/scout.md`

A deep research agent that can investigate topics using web search capabilities.

**Trigger:** Use the `/scout` command in issue or PR comments, or trigger manually with a research topic

**Example usage:**
```
/scout What are the best practices for WebAssembly memory management?
```

The Scout agent will:
- Conduct comprehensive web searches
- Synthesize findings from multiple sources
- Provide actionable recommendations
- Cite sources with links

### CI Doctor

**File:** `.github/workflows/ci-doctor.md`

An AI-powered CI failure investigator that automatically diagnoses test and build failures.

**Trigger:** Automatically runs when the Rust workflow completes on the main branch

**What it does:**
- Analyzes failed CI workflow runs
- Extracts error messages and logs
- Identifies root causes (compilation errors, test failures, linting issues, etc.)
- Checks for similar past failures
- Provides actionable recommendations with specific commands and file locations
- Comments on the workflow run with diagnostic reports

The CI Doctor helps maintainers quickly understand and fix CI failures without manually digging through logs.

## Creating Your Own Agentic Workflows

### Basic Structure

Create a markdown file in `.github/workflows/` with this structure:

```markdown
---
on:
  issues:
    types: [opened]
permissions:
  contents: read
  issues: write
tools:
  github:
    allowed: [get_issue, add_issue_comment]
engine: claude
timeout_minutes: 10
---

# Your Workflow Title

Natural language instructions for what the AI should do.

You can reference GitHub context like:
- Issue number: ${{ github.event.issue.number }}
- Repository: ${{ github.repository }}
- Triggering user: ${{ github.actor }}
```

### Key Configuration Options

#### Triggers (`on:`)
- **Standard events:** `issues`, `pull_request`, `push`, `schedule`, `workflow_dispatch`
- **Command triggers:** Use `command: { name: bot-name }` to respond to `/bot-name` mentions

#### Permissions
Only request what you need:
```yaml
permissions:
  contents: read
  issues: write
  actions: read
```

#### Tools
Control what the AI can access:
```yaml
tools:
  github:
    allowed: [get_issue, add_issue_comment, list_issues]
  bash: [":*"]  # Shell commands
  edit:          # File editing
  web-fetch:     # Web content fetching
  web-search:    # Web searching
```

#### Engines
Choose your AI processor:
- `claude` - Default, good for most tasks
- `copilot` - GitHub Copilot
- `codex` - OpenAI Codex

### Compiling Workflows

After creating or modifying a workflow, compile it:

```bash
# Compile all workflows
gh aw compile

# Compile a specific workflow
gh aw compile issue-triage

# Compile with verbose output
gh aw compile --verbose
```

This generates a `.lock.yml` file that GitHub Actions will execute.

## Best Practices

### 1. Security First
- Use minimal permissions (read-only when possible)
- Use `safe-outputs` for controlled write operations
- Be cautious with `bash` tool access

### 2. Clear Instructions
- Write specific, actionable instructions
- Use numbered steps for complex workflows
- Include examples when helpful

### 3. Context Awareness
- Use `${{ needs.activation.outputs.text }}` for sanitized content
- Reference relevant issue/PR data
- Provide context about the repository

### 4. Testing
- Test workflows with `workflow_dispatch` triggers first
- Monitor logs with `gh aw logs`
- Iterate based on actual behavior

## Advanced Features

### Safe Outputs

Use `safe-outputs` to separate permissions - the main AI job doesn't need write permissions:

```yaml
permissions:
  contents: read
  actions: read

safe-outputs:
  create-issue:
    title-prefix: "[ai] "
    labels: [automation, ai-generated]
  add-comment:
    max: 1
```

### Memory Caching

Enable persistent memory across workflow runs:

```yaml
tools:
  cache-memory:
    retention-days: 7
```

### Network Permissions

Control network access for the Claude engine:

```yaml
network:
  allowed:
    - defaults         # Basic infrastructure
    - python          # Python/PyPI ecosystem
    - node            # Node.js/NPM ecosystem
    - "api.custom.com" # Custom domain
```

### Include Directives

Reuse shared configuration:

```markdown
@include agentics/shared/common-setup.md
```

## Monitoring and Debugging

### View Logs

```bash
# Download logs for all workflows
gh aw logs

# Download logs for a specific workflow
gh aw logs issue-triage

# Filter by engine type
gh aw logs --engine claude

# Filter by date range
gh aw logs --start-date -1w  # Last week
```

### Inspect MCP Servers

```bash
# List workflows with MCP configurations
gh aw mcp inspect

# Inspect MCP servers in a specific workflow
gh aw mcp inspect scout

# Show detailed tool information
gh aw mcp inspect scout --server tavily-mcp --tool search
```

## Resources

- **Official Documentation:** [gh-aw docs](https://githubnext.github.io/gh-aw/)
- **Installation:** `gh extension install githubnext/gh-aw`
- **Instructions File:** `.github/instructions/github-agentic-workflows.instructions.md`
- **Example Workflows:** `.github/workflows/*.md`

## Contributing

When creating new agentic workflows for this repository:

1. Create the workflow file in `.github/workflows/`
2. Follow the naming convention: `workflow-name.md`
3. Test thoroughly with `workflow_dispatch` first
4. Run `gh aw compile` to generate the `.lock.yml`
5. Commit both `.md` and `.lock.yml` files
6. Update this documentation if needed

Remember to update the `CHANGELOG.md` when adding new workflows!
