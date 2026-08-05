---
name: rust-code-style
description: Write idiomatic, well-structured Rust for Wassette — single-responsibility design, anyhow error handling, Arc/Mutex for shared state, and the required formatting and linting with cargo +nightly fmt, cargo clippy, and cargo machete. Use when adding or refactoring Rust code in this repository.
allowed-tools: Bash, Read, Write, Edit, Glob, Grep
---

# rust-code-style skill

Apply Wassette's Rust conventions when writing or refactoring `.rs` files.

## Best practices

- **Single responsibility**: each function and struct has one clear purpose.
- **DRY**: extract shared logic into reusable functions or modules.
- **Descriptive naming**: name functions, variables, and types for readability.
- **Unit tests**: cover every public function and its edge cases.
- **Keep it simple**: prefer straightforward solutions over clever complexity.
- **Dependencies**: manage them carefully in `Cargo.toml`; avoid unnecessary
  ones that bloat the project.
- **Error handling**: use `anyhow` to add context and stack traces.
- **Traits and generics**: use traits for shared behavior and generics for
  reusable, type-safe, extensible APIs.
- **Thread safety**: use stdlib primitives like `Arc` and `Mutex` for shared
  state.
- **Performance**: choose appropriate data types, e.g. `&str` over `String`
  when possible.

## Formatting and linting

Always run before committing:

```bash
cargo +nightly fmt          # Format (nightly toolchain required)
cargo clippy --workspace    # Lint; resolve every warning
cargo machete               # Check for unused dependencies
```

Write code that passes `cargo clippy` without warnings.

## Copyright headers

Every `.rs` file must start with the Microsoft copyright header. See the
`copyright-headers` skill for the exact format and the automated script.
