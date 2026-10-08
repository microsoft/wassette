# Context7 Example (.NET)

This example keeps the same WIT contract and API behavior as `context7-rs` for
the .NET 10 port. It uses explicit WASI HTTP and environment imports, maps
search records with `System.Text.Json`, and returns documentation text.

It is built and validated in CI but is not published by the workflow until the
preview API endpoint and response shape have stabilized.
