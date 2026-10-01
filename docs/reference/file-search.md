# File Search Tool

`wassette:file-search` is a first-party Wassette tool that searches file
contents below a directory, like a sandboxed `rg`. It uses the ripgrep search
crates (`grep-regex`, `grep-searcher`, and `ignore`) compiled to a WebAssembly
component, and it can read only the directories its storage policy grants.

## Install

From a source checkout, `just install` builds and installs it alongside the ACP
components. The component publication workflow publishes it as
`ghcr.io/microsoft/file-search`:

```text
Please load the component from oci://ghcr.io/microsoft/file-search:latest
```

## Grant access

The component starts with no filesystem access. Grant read access to each root
you want to search:

```json
{
  "component_id": "wassette:file-search",
  "details": { "uri": "fs:///path/to/project", "access": ["read"] }
}
```

Pass this to the `grant-storage-permission` built-in tool, or ship it in a
policy file. [`components/file-search/policy.yaml`](https://github.com/microsoft/wassette/blob/main/components/file-search/policy.yaml)
is the default policy and grants nothing. Searching an ungranted root, including
one reached through `..`, fails with an error that names the grant to request.

## `search-files`

| Argument | Type | Description |
| --- | --- | --- |
| `root` | string | Absolute directory or file to search. Must be granted. |
| `pattern` | string | Rust regular expression, or literal text when `literal` is true. |
| `literal` | bool | Match `pattern` as plain text. |
| `case-insensitive` | bool | Ignore case. |
| `glob` | string or null | Only search matching paths, such as `*.rs` or `src/**`. |
| `max-results` | u32 or null | Match limit; defaults to 200, capped at 10000. |

```json
{
  "root": "/path/to/project",
  "pattern": "TODO",
  "literal": true,
  "case-insensitive": false,
  "glob": "*.rs",
  "max-results": 20
}
```

The result lists matches in path order:

```json
{
  "ok": {
    "matches": [
      { "path": "/path/to/project/src/lib.rs", "line-number": 42, "line": "// TODO: tidy" }
    ],
    "truncated": false,
    "files-searched": 12
  }
}
```

The search skips hidden files, binary files, and paths ignored by `.gitignore`
or `.ignore` (even when the granted root is below the repository's `.git`).
Returned lines are clipped to 1000 bytes. `truncated` is true when more matches
existed than `max-results` allowed.
