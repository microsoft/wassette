# Wassette AI Agent Instructions

Essential instructions for AI agents working with Wassette components and permission management.

## Critical: Permission Management

**⚠️ IMPORTANT: ALWAYS use the built-in `grant-xxx-permission` tools to manage permissions for Wassette components.**

### Permission Tools You MUST Use

When working with Wassette components, you **MUST** use these built-in MCP tools to grant permissions:

- **`grant-storage-permission`**: Grant file system access to components
- **`grant-network-permission`**: Grant network access to components  
- **`grant-environment-variable-permission`**: Grant environment variable access to components

### What You MUST NOT Do

**❌ NEVER manually edit or modify policy files directly** (such as `policy.yaml` files)

**❌ NEVER create policy files manually**

**❌ NEVER suggest users manually edit policy files**

### Why This Matters

Wassette uses a sophisticated permission system that:
- Validates permissions at runtime
- Maintains consistency across component lifecycles
- Enforces security policies programmatically
- Tracks permission changes automatically

Manually editing policy files can:
- Break the permission validation system
- Create security vulnerabilities
- Cause runtime errors
- Lead to inconsistent component behavior

## Correct Permission Workflow

### Example 1: Granting Storage Permission

When a component needs file system access:

```text
Please grant storage read and write permissions to the weather-tool for fs://workspace/
```

The AI agent will automatically use the `grant-storage-permission` tool with the correct parameters.

### Example 2: Granting Network Permission

When a component needs network access:

```text
Grant network access to api.openweathermap.org for the weather-tool component
```

The AI agent will automatically use the `grant-network-permission` tool.

### Example 3: Granting Environment Variable Permission

When a component needs environment variable access:

```text
Allow the weather-tool to access the API_KEY environment variable
```

The AI agent will automatically use the `grant-environment-variable-permission` tool.

## Understanding Permission Types

### Storage Permissions

Control file system access for reading and writing files:
- Use URI format: `fs://path/to/resource`
- Specify access levels: `read`, `write`, or both
- Supports wildcards: `fs://workspace/**`

### Network Permissions

Control outbound network access to specific hosts:
- Specify host names or IP addresses
- Can include ports: `localhost:8080`
- Supports domain-level access: `api.example.com`

### Environment Variable Permissions

Control access to environment variables:
- Grant access to specific variable names
- Used for API keys, configuration, and secrets
- Requires the server to have access to those variables

## Component Lifecycle

1. **Load Component**: Use `load-component` tool to load a component
2. **Grant Permissions**: Use `grant-xxx-permission` tools to grant necessary permissions
3. **Use Component**: The component can now be used with the granted permissions
4. **Revoke/Reset**: Use `revoke-xxx-permission` or `reset-permission` tools if needed

## Security Best Practices

1. **Principle of Least Privilege**: Only grant the minimum permissions needed
2. **Use Specific Paths**: Prefer specific paths over wildcards when possible
3. **Audit Regularly**: Use `get-policy` tool to review granted permissions
4. **Clean Up**: Revoke permissions when components are no longer needed

## Troubleshooting Permission Issues

If a component fails due to permission errors:

1. Check the error message for the specific permission needed
2. Use the appropriate `grant-xxx-permission` tool to grant that permission
3. Verify the permission was granted using the `get-policy` tool
4. Re-attempt the component operation

**Remember**: Never manually edit policy files - always use the built-in tools!

## Additional Resources

- [Wassette Documentation](https://microsoft.github.io/wassette/latest/)
- [Permission System Design](https://github.com/microsoft/wassette/blob/main/docs/design/permission-system.md)
- [CLI Reference](https://github.com/microsoft/wassette/blob/main/docs/reference/cli.md)
- [Managing Permissions](https://github.com/microsoft/wassette/blob/main/docs/reference/permissions.md)
