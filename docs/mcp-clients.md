# Model Context Protocol (MCP) Clients

If you haven't installed Wassette yet, follow the [installation instructions](https://github.com/microsoft/wassette?tab=readme-ov-file#installation) first.

## Visual Studio Code

Add the Wassette MCP Server to GitHub Copilot in Visual Studio Code by clicking the **Install in VS Code** or **Install in VS Code Insiders** badge below:

[![Install in VS Code](https://img.shields.io/badge/VS_Code-Install_Server-0098FF?style=flat-square&logo=visualstudiocode&logoColor=white)](https://vscode.dev/redirect?url=vscode:mcp/install?%7B%22name%22%3A%22wassette%22%2C%22gallery%22%3Afalse%2C%22command%22%3A%22wassette%22%2C%22args%22%3A%5B%22serve%22%2C%22--stdio%22%5D%7D) [![Install in VS Code Insiders](https://img.shields.io/badge/VS_Code_Insiders-Install_Server-24bfa5?style=flat-square&logo=visualstudiocode&logoColor=white)](https://vscode.dev/redirect?url=vscode-insiders:mcp/install?%7B%22name%22%3A%22wassette%22%2C%22gallery%22%3Afalse%2C%22command%22%3A%22wassette%22%2C%22args%22%3A%5B%22serve%22%2C%22--stdio%22%5D%7D)

Alternatively, you can add the Wassete MCP server to VS Code from the command line using the `code` command in a bash/zsh or PowerShell terminal:

### bash/zsh

```bash
code --add-mcp '{"name":"Wassette","command":"wassette","args":["serve","--stdio"]}'
```

### PowerShell

```powershell
 code --% --add-mcp "{\"name\":\"wassette\",\"command\":\"wassette\",\"args\":[\"serve\",\"--stdio\"]}"
```

You can list and configure MCP servers in VS Code by running the command `MCP: List Servers` in the command palette (Ctrl+Shift+P or Cmd+Shift+P).

## Cursor

Click the below button to use the [one-click installation](https://docs.cursor.com/en/context/mcp#one-click-installation) to add Wassette to Cursor.

[![Install MCP Server](https://cursor.com/deeplink/mcp-install-light.svg)](https://cursor.com/install-mcp?name=wassette&config=JTdCJTIyY29tbWFuZCUyMiUzQSUyMndhc3NldHRlJTIwc2VydmUlMjAtLXN0ZGlvJTIyJTdE)

## Claude Code

First, [install Claude Code](https://github.com/anthropics/claude-code?tab=readme-ov-file#get-started) (requires Node.js 18 or higher):

```bash
npm install -g @anthropic-ai/claude-code
```

Add the Wassette MCP server to Claude Code using the following command:

```bash
claude mcp add -- wassette wassette serve --stdio
```

This will configure the Wassette MCP server as a local stdio server that Claude Code can use to execute Wassette commands and interact with your data infrastructure.

You can verify the installation by running:
```bash
claude mcp list
```

To remove the server if needed:
```bash
claude mcp remove wassette
```

## Gemini CLI

First, [install Gemini CLI](https://github.com/google-gemini/gemini-cli?tab=readme-ov-file#quickstart) (requires Node.js 20 or higher):

```bash
npm install -g @google/gemini-cli
```

To add the Wassette MCP server to Gemini CLI, you need to configure it in your settings file at `~/.gemini/settings.json`. Create or edit this file to include:

```json
{
  "mcpServers": {
    "wassette": {
      "command": "wassette",
      "args": ["serve", "--stdio"]
    }
  }
}
```

Quit the Gemini CLI and reopen it.

Open Gemini CLI and verify the installation by running `/mcp` inside of Gemini CLI.

## OpenAI Codex CLI

First, [install Codex CLI](https://github.com/openai/codex?tab=readme-ov-file#installing-and-running-codex-cli) (requires Node.js) using either npm or Homebrew:

```bash
npm install -g @openai/codex
```

Or with Homebrew:

```bash
brew install codex
```

Add the Wassette MCP server to Codex CLI using the following command:

```bash
codex mcp add wassette wassette serve --stdio
```

Run `codex` to start the CLI.

Verify the installation by running `/mcp` inside of Codex CLI.
