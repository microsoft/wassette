# Fetch-rs Example

This example demonstrates the use of the `wassette` runtime to interact with HTTP APIs as a WebAssembly (Wasm) component using Rust. It showcases how to define and enforce permissions for accessing network resources using a policy file.

## Tools

- **Fetch URL**: Fetches the contents of a specified URL using HTTP GET.

## Setup

1. **Add MCP Server to VS Code**:

   - Open your `settings.json` file in VS Code.
   - Add the MCP server configuration under the `mcp.servers` section. Example:
     ```json
     "mcp": {
       "servers": {
         "wassette": {
           "type": "sse",
           "url": "http://127.0.0.1:9001/sse"
         }
       }
     }
     ```

2. **Start the MCP Server**:

   - Use the `Justfile` to start the server with the appropriate policy file:
     ```bash
     just run-fetch-rs
     ```

3. **Run a Fetch Tool**:

   - Use the agent in VS Code to execute a fetch tool, such as `fetch_url`. Ensure the tool is configured to use the MCP server.

## Policy File

By default, WebAssembly (Wasm) components do not have any access to the host machine or network. The `policy.yaml` file is used to explicitly define what network resources are made available to the component. This ensures that the component can only access the resources that are explicitly allowed.

Example:

```yaml
version: "1.0"
description: "Permission policy for fetch-rs example in wassette"
permissions:
  network:
    allow:
      - host: "https://rss.nytimes.com/"
```
