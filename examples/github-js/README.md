# GitHub API WebAssembly Component

A comprehensive GitHub REST API integration built as a WebAssembly component using JavaScript and the Component Model.

## Overview

This component provides access to GitHub's REST API v3, including operations for:

- **Repositories**: branches, commits, files, tags, releases
- **Issues**: create, update, close, comment, search
- **Pull Requests**: create, update, merge, review
- **Labels**: create, list, update, delete
- **Workflows**: list, trigger, manage runs
- **Users & Organizations**: profile info, teams, members
- **Security**: code scanning, secret scanning, Dependabot alerts

## Building

```bash
npm install
npm run build
```

This generates `github.wasm` in the current directory.

## Running with Wasmtime

### Prerequisites

- Wasmtime with WASI Preview 2 support
- GitHub Personal Access Token

### Basic Usage

```bash
wasmtime run -Shttp -Sconfig \
  -S 'config-var=GITHUB_TOKEN=your_token_here' \
  --invoke 'function-name("arg1", "arg2", ...)' \
  github.wasm
```

### Important Notes

1. **Positional Arguments Only**: Use positional arguments, not named parameters
   ```bash
   # ✅ Correct
   --invoke 'create-branch("owner", "repo", "branch-name", none)'

   # ❌ Wrong
   --invoke 'create-branch(owner: "owner", repo: "repo", ...)'
   ```

2. **Config Variables**: Use `-S config-var=` for environment variables
   ```bash
   # ✅ Correct
   -S 'config-var=GITHUB_TOKEN=ghp_...'

   # ❌ Wrong
   --env GITHUB_TOKEN=ghp_...
   ```

3. **Optional Parameters**: Use `none` or `some("value")`
   ```bash
   # No optional value
   --invoke 'list-branches("owner", "repo", none, none)'

   # With optional value
   --invoke 'create-branch("owner", "repo", "new-branch", some("main"))'
   ```

## Examples

### Repository Operations

#### Get Repository Info

```bash
wasmtime run -Shttp -Sconfig \
  -S 'config-var=GITHUB_TOKEN=ghp_...' \
  --invoke 'get-repository("owner", "repo")' \
  github.wasm
```

#### Create a File

```bash
wasmtime run -Shttp -Sconfig \
  -S 'config-var=GITHUB_TOKEN=ghp_...' \
  --invoke 'create-or-update-file("owner", "repo", "path/to/file.txt", "File content", "Commit message", none)' \
  github.wasm
```

## Testing

Run the included test suite:

```bash
chmod +x test.sh
./test.sh
```

See `TEST_RESULTS.md` for comprehensive test results.

