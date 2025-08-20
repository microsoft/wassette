# Installing Wassette with WinGet

Wassette provides a WinGet package manifest for easy installation on Windows systems.

## Installation

Since WinGet doesn't support installing directly from URLs, you need to download the manifest first:

```powershell
# Download the manifest
Invoke-WebRequest -Uri https://raw.githubusercontent.com/microsoft/wassette/main/winget/Microsoft.Wassette.yaml -OutFile Microsoft.Wassette.yaml

# Install from the downloaded manifest
winget install --manifest Microsoft.Wassette.yaml
```

If the installation fails, it is probably because the local installation feature is not enabled.
You can activate it with the following command in an administrator shell:

```powershell
winget settings --enable LocalManifestFiles
```

## Verification

After installation, verify that Wassette is available:

```powershell
wassette --version
```

## Uninstall

To uninstall Wassette, use one of these commands:

```powershell
# Try the simple name first
winget uninstall Wassette
```

If that doesn't work, list installed packages to get the exact ID:

```powershell
# List to find the exact package ID
winget list Wassette

# Use the exact ID (example output may vary)
winget uninstall "Wassette ARP\User\Arm64\Microsoft.Wassette__DefaultSource"
```

## Next Steps

After installation, follow the [MCP clients setup guide](mcp-clients.md) to configure Wassette with your AI agent of choice.
