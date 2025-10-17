# Quick Start

After installing Wassette, get started in 3 simple steps:

**1. Connect to an AI agent**

For VS Code with GitHub Copilot, click to install:

[![Install in VS Code](https://img.shields.io/badge/VS_Code-Install_Server-0098FF?style=flat-square&logo=visualstudiocode&logoColor=white)](https://vscode.dev/redirect?url=vscode:mcp/install?%7B%22name%22%3A%22wassette%22%2C%22gallery%22%3Afalse%2C%22command%22%3A%22wassette%22%2C%22args%22%3A%5B%22serve%22%2C%22--stdio%22%5D%7D)

Or use the command line:
```bash
code --add-mcp '{"name":"Wassette","command":"wassette","args":["serve","--stdio"]}'
```

**2. Load a component**

Ask your AI agent:
```
Please load the time component from ghcr.io/microsoft/time-server-js:latest
```

**3. Use the component**

Ask your AI agent:
```
What is the current time?
```

For other AI agents (Cursor, Claude Code, Gemini CLI), see the [MCP clients guide](./mcp-clients.md).
