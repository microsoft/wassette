# GitHub API Component (.NET)

This .NET 10 port intentionally isolates a NativeAOT-safe, read-only subset
of the much larger `github-js` component: repository metadata, file contents,
branches, issues, and the authenticated-user profile. It preserves those tool
names and uses explicit WASI HTTP and environment imports.

Write operations, Actions, pull requests, labels, organizations, security
endpoints, GraphQL-only features, and artifact/log downloads are not exposed
in this preview port. They are documented here rather than silently dropped
from a claimed full API replacement.

The policy grants access only to `api.github.com` and the `GITHUB_TOKEN`
environment key. Supply the token through Wassette at runtime; do not place
credentials in this directory or in CI configuration.

```bash
dotnet build -c Release
just inject-docs examples/github-dotnet/bin/Release/net10.0/wasi-wasm/native/github-dotnet.wasm examples/github-dotnet/wit
```
