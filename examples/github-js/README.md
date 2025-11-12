# GitHub API Component

A WebAssembly component that exposes common GitHub APIs as exported functions, compatible with the tools provided by the [official GitHub MCP server](https://github.com/github/github-mcp-server).

## Overview

This component provides a comprehensive set of GitHub API operations organized into the following categories:

- **Repository Operations**: Create, fork, manage files, branches, commits, tags, releases, and search code
- **Issue Operations**: List, create, update issues and comments
- **Pull Request Operations**: Create, update, merge PRs, manage reviews
- **GitHub Actions/Workflows**: Manage workflows, runs, jobs, artifacts, and logs
- **Labels**: Create, update, delete repository labels
- **User & Organization Operations**: Search users and organizations, manage teams
- **Gists**: Create, update, and manage gists
- **Notifications**: Manage GitHub notifications
- **Security Features**: Code scanning, secret scanning, Dependabot alerts, security advisories
- **Stars**: Manage starred repositories
- **Projects**: GitHub Projects V2 (requires GraphQL - not yet implemented)
- **Discussions**: GitHub Discussions (requires GraphQL - not yet implemented)

## Prerequisites

- Node.js and npm installed
- A GitHub Personal Access Token with appropriate permissions

## Building

```bash
just build
```

Or manually:

```bash
npm install
npm run build
```

This will generate `github.wasm` component.

## Configuration

The component requires a `GITHUB_TOKEN` environment variable to be set. This should be a GitHub Personal Access Token with the necessary scopes for the operations you want to perform.

### Required Token Scopes

Depending on the operations you need, grant the following scopes to your Personal Access Token:

- `repo` - Full control of private repositories
- `public_repo` - Access to public repositories only
- `read:org` - Read organization data
- `write:org` - Write organization data
- `admin:org` - Full control of organizations
- `gist` - Create and update gists
- `notifications` - Access notifications
- `user` - Read user profile data
- `read:packages` - Download packages
- `write:packages` - Upload packages
- `delete:packages` - Delete packages
- `workflow` - Update GitHub Actions workflows
- `security_events` - Read and write security events

## Usage with Wassette

1. Build the component:
   ```bash
   just build
   ```

2. Set your GitHub token:
   ```bash
   export GITHUB_TOKEN=ghp_your_token_here
   ```

3. Load the component in Wassette (via your MCP client):
   ```
   Please load the GitHub component from file:///path/to/github.wasm
   ```

4. Use the GitHub API functions through your AI agent:
   ```
   List the open issues in the microsoft/wassette repository
   ```

## API Functions

### Repository Operations

- `get-repository(owner, repo)` - Get repository information
- `create-repository(name, description?, private)` - Create a new repository
- `fork-repository(owner, repo)` - Fork a repository
- `get-file-contents(owner, repo, path, ref?)` - Get file or directory contents
- `create-or-update-file(owner, repo, path, content, message, branch?)` - Create or update a file
- `delete-file(owner, repo, path, message, branch?)` - Delete a file
- `list-branches(owner, repo, page?, per-page?)` - List repository branches
- `create-branch(owner, repo, branch, from-branch?)` - Create a new branch
- `list-commits(owner, repo, sha?, page?, per-page?)` - List commits
- `get-commit(owner, repo, sha)` - Get commit details
- `list-tags(owner, repo, page?, per-page?)` - List repository tags
- `get-tag(owner, repo, tag)` - Get tag details
- `list-releases(owner, repo, page?, per-page?)` - List releases
- `get-latest-release(owner, repo)` - Get the latest release
- `get-release-by-tag(owner, repo, tag)` - Get a specific release by tag
- `get-repository-tree(owner, repo, tree-sha?, recursive)` - Get repository tree
- `search-code(query, page?, per-page?)` - Search code across GitHub
- `search-repositories(query, page?, per-page?)` - Search repositories

### Issue Operations

- `list-issues(owner, repo, state?, labels?, page?, per-page?)` - List issues
- `issue-read(owner, repo, issue-number, method)` - Read issue details (methods: get, get_comments, get_labels)
- `issue-write(owner, repo, method, params)` - Create or update issues (methods: create, update, close)
- `add-issue-comment(owner, repo, issue-number, body)` - Add a comment to an issue
- `search-issues(query, page?, per-page?)` - Search issues across GitHub

### Pull Request Operations

- `list-pull-requests(owner, repo, state?, head?, base?, page?, per-page?)` - List pull requests
- `create-pull-request(owner, repo, title, head, base, body?, draft)` - Create a pull request
- `pull-request-read(owner, repo, pull-number, method)` - Read PR details (methods: get, get_diff, get_files, get_comments, get_reviews, get_status)
- `update-pull-request(owner, repo, pull-number, params)` - Update a pull request
- `merge-pull-request(owner, repo, pull-number, merge-method?)` - Merge a pull request
- `search-pull-requests(query, page?, per-page?)` - Search pull requests
- `pull-request-review-write(owner, repo, pull-number, action, params)` - Manage PR reviews (actions: create, submit, delete)
- `add-comment-to-pending-review(owner, repo, pull-number, path, body, line?)` - Add comment to pending review
- `update-pull-request-branch(owner, repo, pull-number)` - Update PR branch with base branch

### GitHub Actions/Workflows

- `list-workflows(owner, repo, page?, per-page?)` - List repository workflows
- `list-workflow-runs(owner, repo, workflow-id, page?, per-page?)` - List workflow runs
- `get-workflow-run(owner, repo, run-id)` - Get workflow run details
- `get-workflow-run-usage(owner, repo, run-id)` - Get workflow run usage/timing
- `cancel-workflow-run(owner, repo, run-id)` - Cancel a workflow run
- `rerun-workflow-run(owner, repo, run-id)` - Rerun a workflow run
- `rerun-failed-jobs(owner, repo, run-id)` - Rerun failed jobs in a workflow run
- `run-workflow(owner, repo, workflow-id, ref, inputs?)` - Trigger a workflow run
- `list-workflow-jobs(owner, repo, run-id, page?, per-page?)` - List workflow jobs
- `get-job-logs(owner, repo, job-id, run-id?)` - Get job logs
- `list-workflow-run-artifacts(owner, repo, run-id, page?, per-page?)` - List workflow artifacts
- `download-workflow-run-artifact(owner, repo, artifact-id)` - Download workflow artifact
- `get-workflow-run-logs(owner, repo, run-id)` - Get all workflow run logs
- `delete-workflow-run-logs(owner, repo, run-id)` - Delete workflow run logs

### Labels

- `list-label(owner, repo, page?, per-page?)` - List repository labels
- `get-label(owner, repo, name)` - Get a specific label
- `label-write(owner, repo, action, params)` - Create, update, or delete labels (actions: create, update, delete)

### User & Organization Operations

- `get-me()` - Get authenticated user profile
- `search-users(query, page?, per-page?)` - Search users
- `search-orgs(query, page?, per-page?)` - Search organizations
- `get-teams(org, page?, per-page?)` - Get organization teams
- `get-team-members(org, team-slug, page?, per-page?)` - Get team members

### Gists

- `list-gists(page?, per-page?)` - List user's gists
- `get-gist(gist-id)` - Get a specific gist
- `create-gist(description?, files, public)` - Create a new gist
- `update-gist(gist-id, description?, files?)` - Update a gist

### Notifications

- `list-notifications(all, page?, per-page?)` - List notifications
- `get-notification-details(thread-id)` - Get notification details
- `mark-all-notifications-read()` - Mark all notifications as read
- `dismiss-notification(thread-id)` - Dismiss a notification
- `manage-notification-subscription(thread-id, subscribed)` - Manage notification subscription
- `manage-repository-notification-subscription(owner, repo, subscribed)` - Manage repository notifications

### Security Features

#### Code Scanning
- `list-code-scanning-alerts(owner, repo, state?, ref?, page?, per-page?)` - List code scanning alerts
- `get-code-scanning-alert(owner, repo, alert-number)` - Get a specific alert

#### Secret Scanning
- `list-secret-scanning-alerts(owner, repo, state?, page?, per-page?)` - List secret scanning alerts
- `get-secret-scanning-alert(owner, repo, alert-number)` - Get a specific alert

#### Dependabot
- `list-dependabot-alerts(owner, repo, state?, page?, per-page?)` - List Dependabot alerts
- `get-dependabot-alert(owner, repo, alert-number)` - Get a specific alert

#### Security Advisories
- `list-global-security-advisories(cve-id?, ghsa-id?, page?, per-page?)` - List global security advisories
- `get-global-security-advisory(advisory-id)` - Get a specific advisory
- `list-repository-security-advisories(owner, repo, state?, page?, per-page?)` - List repository security advisories
- `list-org-repository-security-advisories(org, state?, page?, per-page?)` - List organization security advisories

### Stars

- `list-starred-repositories(page?, per-page?)` - List starred repositories
- `star-repository(owner, repo)` - Star a repository
- `unstar-repository(owner, repo)` - Unstar a repository

## Limitations

Some features from the official GitHub MCP server are not yet implemented because they require GraphQL API or are GitHub Copilot-specific features:

- **Projects V2** - Requires GraphQL API
- **Discussions** - Requires GraphQL API
- **Sub-issues** - Requires GraphQL API (taskLists)
- **Issue Types** - Requires GraphQL API
- **Copilot-specific features** - `assign-copilot-to-issue`, `request-copilot-review`
- **Push Files** - Complex multi-file push operation (use `create-or-update-file` for single files)

## API Response Format

All functions return results in JSON format. Successful responses contain the GitHub API response data, while errors return error messages with context.

Example success response:
```json
{
  "id": 123456,
  "name": "repository-name",
  "full_name": "owner/repository-name",
  ...
}
```

Example error response:
```json
{
  "error": "GitHub API error (404): Not Found"
}
```

## Security

This component follows security best practices:

- Requires explicit permission for network access to `api.github.com`
- Requires explicit permission for `GITHUB_TOKEN` environment variable
- All API requests include proper authentication headers
- Uses GitHub API version `2022-11-28` for consistency

## License

This component is licensed under the MIT License.

## Contributing

Contributions are welcome! Please ensure any changes maintain compatibility with the official GitHub MCP server's tool interface.

## Related Projects

- [Official GitHub MCP Server](https://github.com/github/github-mcp-server) - The reference implementation
- [Wassette](https://github.com/microsoft/wassette) - WebAssembly runtime for MCP
