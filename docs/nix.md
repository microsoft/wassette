# Nix

You can install Wassette using Nix flakes:

```bash
# Run directly without installation
nix run github:microsoft/wassette -- serve --stdio

# Install to your profile
nix profile install github:microsoft/wassette

# Or in a development shell
nix develop github:microsoft/wassette
```

This provides a reproducible environment for using and developing Wassette.
The flake provides a `wassette` package and a development shell with all
the necessary dependencies.
