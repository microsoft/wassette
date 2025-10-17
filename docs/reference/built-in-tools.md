# Built-in Tools

Wassette comes with several built-in tools for managing components and their permissions. These tools are available immediately when you start the MCP server.

| Tool | Description |
|------|-------------|
| `load-component` | Dynamically loads a new tool or component from either the filesystem or OCI registries |
| `unload-component` | Unloads a tool or component |
| `list-components` | Lists all currently loaded components or tools |
| `search-components` | Lists all known components that can be fetched and loaded from the component registry |
| `get-policy` | Gets the policy information for a specific component |
| `grant-storage-permission` | Grants storage access permission to a component, allowing it to read from and/or write to specific storage locations |
| `grant-network-permission` | Grants network access permission to a component, allowing it to make network requests to specific hosts |
| `grant-environment-variable-permission` | Grants environment variable access permission to a component, allowing it to access specific environment variables |
| `revoke-storage-permission` | Revokes all storage access permissions from a component for the specified URI path, removing both read and write access to that location |
| `revoke-network-permission` | Revokes network access permission from a component, removing its ability to make network requests to specific hosts |
| `revoke-environment-variable-permission` | Revokes environment variable access permission from a component, removing its ability to access specific environment variables |
| `reset-permission` | Resets all permissions for a component, removing all granted permissions and returning it to the default state |

<details>
<summary><strong>Component Management Tools</strong></summary>

## load-component
**Parameters:**
- `path` (string, required): Path to the component from either filesystem or OCI registries (e.g., `oci://ghcr.io/microsoft/time-server-js:latest` or `/path/to/component.wasm`)

**Returns:**
```json
{
  "status": "component loaded successfully",
  "id": "component-unique-id",
  "tools": ["tool-one", "tool-two"]
}
```
When an existing component is replaced, the `status` value becomes
`component reloaded successfully`.

## unload-component
**Parameters:**
- `id` (string, required): Unique identifier of the component to unload

**Returns:**
```json
{
  "status": "component unloaded successfully",
  "id": "component-unique-id"
}
```

## list-components
**Parameters:** None

**Returns:**
```json
{
  "components": [
    {
      "id": "component-id",
      "tools_count": 2,
      "schema": {
        "tools": [...]
      }
    }
  ],
  "total": 1
}
```

## search-components
**Parameters:** None

**Returns:**
```json
{
  "status": "Component list found",
  "components": [
    {
      "name": "Weather Server",
      "description": "A weather component written in JavaScript",
      "uri": "oci://ghcr.io/microsoft/get-weather-js:latest"
    },
    {
      "name": "Time Server", 
      "description": "A time server component written in JavaScript",
      "uri": "oci://ghcr.io/microsoft/time-server-js:latest"
    }
  ]
}
```

</details>

<details>
<summary><strong>Policy Management Tools</strong></summary>

## get-policy
**Parameters:**
- `component_id` (string, required): ID of the component to get policy information for

**Returns:**
```json
{
  "status": "policy found",
  "component_id": "component-id",
  "policy_info": {
    "policy_id": "policy-uuid",
    "source_uri": "oci://registry.example.com/component:tag",
    "local_path": "/path/to/cached/component",
    "created_at": 1640995200
  }
}
```

</details>

<details>
<summary><strong>Permission Grant Tools</strong></summary>

## grant-storage-permission
**Parameters:**
- `component_id` (string, required): ID of the component to grant storage permission to
- `details` (object, required):
  - `uri` (string, required): URI of the storage resource (e.g., `fs:///tmp/test`)
  - `access` (array, required): Array of access types, must be `["read"]`, `["write"]`, or `["read", "write"]`

**Returns:**
```json
{
  "status": "permission granted successfully",
  "component_id": "component-id",
  "permission_type": "storage",
  "details": {
    "uri": "fs:///tmp/test",
    "access": ["read", "write"]
  }
}
```

## grant-network-permission
**Parameters:**
- `component_id` (string, required): ID of the component to grant network permission to
- `details` (object, required):
  - `host` (string, required): Host to grant network access to (e.g., `api.example.com`)

**Returns:**
```json
{
  "status": "permission granted successfully",
  "component_id": "component-id",
  "permission_type": "network",
  "details": {
    "host": "api.example.com"
  }
}
```

## grant-environment-variable-permission
**Parameters:**
- `component_id` (string, required): ID of the component to grant environment variable permission to
- `details` (object, required):
  - `key` (string, required): Environment variable key to grant access to (e.g., `API_KEY`)

**Returns:**
```json
{
  "status": "permission granted successfully",
  "component_id": "component-id",
  "permission_type": "environment",
  "details": {
    "key": "API_KEY"
  }
}
```

</details>

<details>
<summary><strong>Permission Revoke Tools</strong></summary>

## revoke-storage-permission
**Parameters:**
- `component_id` (string, required): ID of the component to revoke storage permission from
- `details` (object, required):
  - `uri` (string, required): URI of the storage resource to revoke access from (e.g., `fs:///tmp/test`)

**Returns:**
```json
{
  "status": "permission revoked successfully",
  "component_id": "component-id",
  "uri": "fs:///tmp/test",
  "message": "All access (read and write) to the specified URI has been revoked"
}
```

## revoke-network-permission
**Parameters:**
- `component_id` (string, required): ID of the component to revoke network permission from
- `details` (object, required):
  - `host` (string, required): Host to revoke network access from (e.g., `api.example.com`)

**Returns:**
```json
{
  "status": "permission revoked",
  "component_id": "component-id",
  "permission_type": "network",
  "details": {
    "host": "api.example.com"
  }
}
```

## revoke-environment-variable-permission
**Parameters:**
- `component_id` (string, required): ID of the component to revoke environment variable permission from
- `details` (object, required):
  - `key` (string, required): Environment variable key to revoke access from (e.g., `API_KEY`)

**Returns:**
```json
{
  "status": "permission revoked",
  "component_id": "component-id",
  "permission_type": "environment",
  "details": {
    "key": "API_KEY"
  }
}
```

## reset-permission
**Parameters:**
- `component_id` (string, required): ID of the component to reset permissions for

**Returns:**
```json
{
  "status": "permissions reset successfully",
  "component_id": "component-id"
}
```

</details>

These tools enable you to dynamically manage components and their security permissions without needing to restart the server or modify configuration files directly.
