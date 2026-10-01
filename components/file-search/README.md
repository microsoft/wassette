# wassette:file-search

A default Wassette tool that searches file contents below directories granted
by its storage policy, using the ripgrep search crates. See the
[reference documentation](../../docs/reference/file-search.md).

```bash
# From the repository root: build, add WIT docs, and embed the component name
just build-default-tools

# Unit tests run natively
cargo test --locked
```

`just install` installs it with the ACP components, and the component
publication workflow publishes it as `ghcr.io/microsoft/file-search`.
