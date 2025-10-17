# Installation

Wassette is available for Linux, macOS, and Windows. Choose the installation method that best suits your platform and workflow.

## Quick Start

For the fastest installation experience, we recommend:

- **Linux/macOS**: Use our one-liner install script
- **macOS**: Use Homebrew
- **Windows**: Use WinGet
- **Nix users**: Use Nix flakes

## Installation by Platform

{{#tabs global="os" }}
{{#tab name="Linux" }}

### Quick Install Script (Recommended)

The easiest way to install Wassette on Linux is using our automated install script:

```bash
curl -fsSL https://raw.githubusercontent.com/microsoft/wassette/main/install.sh | bash
```

This script will:
- Automatically detect your system architecture (x86_64 or ARM64)
- Download the latest Wassette release
- Install the binary to `~/.local/bin`
- Configure your shell PATH for immediate access

### Homebrew

If you prefer using Homebrew on Linux:

```bash
brew tap microsoft/wassette https://github.com/microsoft/wassette
brew install wassette
```

### Manual Download

You can also download the latest Linux release manually from the [GitHub Releases page](https://github.com/microsoft/wassette/releases) and add it to your `$PATH`.

{{#endtab }}
{{#tab name="macOS" }}

### Homebrew (Recommended)

The recommended way to install Wassette on macOS is using Homebrew:

```bash
brew tap microsoft/wassette https://github.com/microsoft/wassette
brew install wassette
```

This method works for both Intel and Apple Silicon Macs.

### Quick Install Script

Alternatively, you can use our one-liner install script:

```bash
curl -fsSL https://raw.githubusercontent.com/microsoft/wassette/main/install.sh | bash
```

This script automatically detects whether you're running Intel or Apple Silicon and installs the appropriate binary.

### Manual Download

You can also download the latest macOS release manually from the [GitHub Releases page](https://github.com/microsoft/wassette/releases) and add it to your `$PATH`.

{{#endtab }}
{{#tab name="Windows" }}

### WinGet (Recommended)

For Windows users, we recommend installing Wassette using WinGet:

```powershell
# Download the manifest
Invoke-WebRequest -Uri https://raw.githubusercontent.com/microsoft/wassette/main/winget/Microsoft.Wassette.yaml -OutFile Microsoft.Wassette.yaml

# Install from the downloaded manifest
winget install --manifest Microsoft.Wassette.yaml
```

If the installation fails, you may need to enable local manifest files:

```powershell
winget settings --enable LocalManifestFiles
```

### Uninstall

To uninstall Wassette:

```powershell
winget uninstall Wassette
```

### Manual Download

You can also download the latest Windows release manually from the [GitHub Releases page](https://github.com/microsoft/wassette/releases) and add it to your `%PATH%`.

{{#endtab }}
{{#tab name="Nix" }}

For users who prefer Nix for reproducible environments (works on all platforms):

```bash
# Run directly without installation
nix run github:microsoft/wassette -- serve --stdio

# Install to your profile
nix profile install github:microsoft/wassette

# Or use in a development shell
nix develop github:microsoft/wassette
```

This provides a reproducible environment for using and developing Wassette.

{{#endtab }}
{{#endtabs }}

## Verifying the Installation

After installation, verify that Wassette is properly installed and accessible:

```bash
wassette --version
```

This should display the installed version of Wassette.

## Supported Platforms

Wassette supports the following platforms:

| Operating System | Architecture | Support |
|-----------------|--------------|---------|
| Linux           | x86_64 (amd64) | ✅ Full support |
| Linux           | ARM64 (aarch64) | ✅ Full support |
| macOS           | Intel (x86_64) | ✅ Full support |
| macOS           | Apple Silicon (ARM64) | ✅ Full support |
| Windows         | x86_64 | ✅ Full support |
| Windows         | ARM64 | ✅ Full support |
| Windows Subsystem for Linux | x86_64, ARM64 | ✅ Full support |

## Next Steps

Once Wassette is installed, you'll need to configure it with your AI agent:

1. **Configure with your AI agent**: Follow the [MCP clients setup guide](./mcp-clients.md) for instructions on integrating Wassette with:
   - Visual Studio Code
   - Cursor
   - Claude Code
   - Gemini CLI

2. **Install agent instructions** (Recommended): Add Wassette-specific instructions to your agent's documentation to ensure proper permission management:
   ```bash
   curl https://raw.githubusercontent.com/microsoft/wassette/main/rules/agent.md >> AGENTS.md
   ```
   This adds important guidelines that help AI agents correctly use Wassette's permission tools instead of manually editing policy files.

3. **Load your first component**: Try loading a sample component to verify everything works:
   ```
   Please load the time component from oci://ghcr.io/microsoft/time-server-js:latest
   ```

4. **Explore examples**: Check out the [examples directory](https://github.com/microsoft/wassette/tree/main/examples) for sample components in different languages.

## Troubleshooting

### Command not found

If you get a "command not found" error after installation:

- **Linux/macOS**: Ensure `~/.local/bin` is in your PATH. You may need to restart your terminal or run:
  ```bash
  export PATH="$HOME/.local/bin:$PATH"
  ```

- **Windows**: Ensure the installation directory is in your system PATH. You may need to restart your terminal or log out and back in.

### Permission denied

If you encounter permission errors:

- **Linux/macOS**: Ensure the binary has execute permissions:
  ```bash
  chmod +x ~/.local/bin/wassette
  ```

- **Windows**: Run PowerShell as Administrator when installing with WinGet.

### Other Issues

For additional help:
- Check the [FAQ](./faq.md) for common questions and answers
- Visit our [GitHub Issues](https://github.com/microsoft/wassette/issues) page
- Join our community discussions

## Upgrading

To upgrade to the latest version of Wassette:

- **Homebrew**: `brew update && brew upgrade wassette`
- **WinGet**: `winget upgrade Wassette`
- **Install script**: Re-run the install script
- **Nix**: `nix profile upgrade github:microsoft/wassette`
- **Manual**: Download the latest release and replace your existing binary
