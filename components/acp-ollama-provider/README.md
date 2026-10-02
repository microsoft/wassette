# Ollama provider

`acp-ollama-provider` is an ACP provider component that forwards prompts to a
local [Ollama](https://ollama.com) server and streams the reply back to the
editor. It's a `wassette:acp/provider` guest that
[`wassette acp`](../../docs/design/acp.md) loads like any other provider.
Ported from the `ollama-provider` crate in
[`yoshuawuyts/playground-wasm-acp`](https://github.com/yoshuawuyts/playground-wasm-acp).

## Build and run

From the repository root, with Ollama running and a model pulled
(`ollama pull llama3.2`):

```shell
just build-acp-examples
cargo run -p wassette-mcp-server -- acp --allow-all \
    --provider components/acp-ollama-provider/target/wasm32-wasip2/release/acp_ollama_provider.wasm
```

`--allow-all` grants the network and environment access the provider needs;
a policy granting `localhost` is the least-privilege alternative.

## Configuration

Read from the (inherited) host environment:

| Variable       | Default                           | Purpose                  |
|----------------|-----------------------------------|--------------------------|
| `OLLAMA_URL`   | `http://localhost:11434/api/chat` | Ollama `/api/chat` URL   |
| `OLLAMA_MODEL` | `llama3.2`                        | Fallback model id        |

Each session exposes a model selector populated from `/api/tags`, and each
prompt turn reports context-window usage from `/api/show`. Sessions persist
under the provider's `/data` directory.

## Tests

`crates/wassette-acp/tests/acp_model_providers.rs` drives this provider end
to end against a local mock of the Ollama API; `just test-acp` builds and
runs it.
