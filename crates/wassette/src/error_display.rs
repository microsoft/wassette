// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Helpers for rendering [`anyhow::Error`] values for human consumption.

/// Renders an error together with its full source chain on a single line.
///
/// `anyhow`'s default [`Display`](std::fmt::Display) implementation prints only
/// the outermost message and silently drops every source added with
/// [`Context`](anyhow::Context). The alternate form (`{:#}`) joins the whole
/// chain as `outer: inner: innermost`, which keeps the root cause visible on
/// user-facing surfaces such as the CLI, MCP tool results and tracing output.
///
/// This is generic over [`Display`](std::fmt::Display) so it applies equally to
/// [`anyhow::Error`] and [`wasmtime::Error`], which is a distinct type with the
/// same alternate-form behaviour.
pub fn format_error_chain<E>(error: &E) -> String
where
    E: std::fmt::Display + ?Sized,
{
    format!("{error:#}")
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;

    use super::format_error_chain;

    /// Mirrors the error chain produced by `LifecycleManager::load_component`
    /// when a component imports an interface the linker does not provide.
    fn load_component_error() -> anyhow::Error {
        anyhow!("function implementation is missing")
            .context("instance export `get` has the wrong type")
            .context(
                "component imports instance `wasi:config/store@0.2.0-draft`, but a matching \
                 implementation was not found in the linker",
            )
            .context(
                "Failed to compile component from path: /tmp/component.wasm. Please ensure the \
                 file is a valid WebAssembly component.",
            )
            .context("Failed to load component: oci://ghcr.io/microsoft/get-weather-js:latest")
    }

    #[test]
    fn plain_display_drops_the_source_chain() {
        // This is the bug the helper exists to prevent: `{}` renders only the
        // outermost context.
        let rendered = format!("{}", load_component_error());
        assert_eq!(
            rendered,
            "Failed to load component: oci://ghcr.io/microsoft/get-weather-js:latest"
        );
        assert!(!rendered.contains("not found in the linker"));
    }

    #[test]
    fn format_error_chain_keeps_the_outer_context() {
        let rendered = format_error_chain(&load_component_error());
        assert!(
            rendered.starts_with(
                "Failed to load component: oci://ghcr.io/microsoft/get-weather-js:latest"
            ),
            "outer context must be preserved, got: {rendered}"
        );
    }

    #[test]
    fn format_error_chain_reaches_the_innermost_cause() {
        let rendered = format_error_chain(&load_component_error());
        assert!(
            rendered.contains("wasi:config/store@0.2.0-draft"),
            "missing failing import, got: {rendered}"
        );
        assert!(
            rendered.contains("not found in the linker"),
            "missing linker cause, got: {rendered}"
        );
        assert!(
            rendered.contains("instance export `get` has the wrong type"),
            "missing intermediate cause, got: {rendered}"
        );
        assert!(
            rendered.ends_with("function implementation is missing"),
            "innermost message must reach the user, got: {rendered}"
        );
    }

    #[test]
    fn format_error_chain_renders_on_a_single_line() {
        let rendered = format_error_chain(&load_component_error());
        assert!(
            !rendered.contains('\n'),
            "chain must stay on one line, got: {rendered}"
        );
    }

    #[test]
    fn format_error_chain_handles_a_bare_error() {
        let rendered = format_error_chain(&anyhow!("no sources here"));
        assert_eq!(rendered, "no sources here");
    }
}
