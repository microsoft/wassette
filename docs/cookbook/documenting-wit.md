# Documenting WIT Interfaces

Documentation in your WIT files is automatically extracted and embedded into your compiled Wasm components. AI agents use this documentation to understand what your tools do and when to use them.

## How It Works

Wassette uses [`wit-docs-inject`](https://github.com/Mossaka/wit-docs-inject) to automatically extract documentation from your WIT files and embed them as a `package-docs` custom section in the WASM binary. This happens during the build process - you just need to write the documentation.

## Basic Syntax

Use `///` for documentation comments:

```wit
package local:my-component;

world my-component {
    /// Fetch data from a URL and return the response body.
    ///
    /// Returns an error if the request fails or the URL is invalid.
    export fetch: func(url: string) -> result<string, string>;
}
```

## Documenting Types

```wit
/// Statistics about analyzed text
record text-stats {
    /// Total number of characters
    character-count: u32,

    /// Total number of words
    word-count: u32,
}

/// Processing status
variant status {
    /// Waiting to be processed
    pending,

    /// Currently processing
    processing(u32),

    /// Completed successfully
    completed(string),

    /// Failed with error
    failed(string),
}
```

## Verifying Documentation

After building, verify documentation is embedded:

```bash
# Build your component
just build release

# Inspect the component
./target/debug/component2json path/to/your/component.wasm
```

You should see:
```
Found package docs!
fetch, Some("Fetch data from a URL and return the response body")
```

## Impact on AI Agents

**Without documentation:**
```json
{
  "name": "process",
  "description": "Auto-generated schema for function 'process'"
}
```

**With documentation:**
```json
{
  "name": "process",
  "description": "Process text input by normalizing whitespace and converting to uppercase.\n\nReturns an error if the input is empty after normalization."
}
```

The documentation helps AI agents understand when and how to use your tools effectively.

## Language-Specific Guides

For implementation details in your language:

- [Rust Guide](./rust.md)
- [Go Guide](./go.md)
- [Python Guide](./python.md)
- [JavaScript/TypeScript Guide](./javascript.md)

## Resources

- [WIT Specification](https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md)
- [wit-docs-inject Tool](https://github.com/Mossaka/wit-docs-inject)
- [WebAssembly Component Model](https://component-model.bytecodealliance.org/)
