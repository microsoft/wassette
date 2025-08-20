# Install Wassette with Homebrew

This guide explains how to install `wassette` using Homebrew.

This uses the Formula in `Formula/wassette.rb`, which is setup to download and install the latest release of `wassette` from GitHub.

You must have Homebrew installed on your macOS or Linux system. If you don't, you can install it by following the instructions on the [official Homebrew website](https://brew.sh/).

## Installation

You can install `wassette` with a single command:

```bash
brew tap microsoft/wassette https://github.com/microsoft/wassette
brew install wassette
```

This command will automatically tap the `microsoft/wassette` repository and install the `wassette` formula.

## Verifying the Installation

To make sure everything worked, you can run:

```bash
wassette --version
```

## Upgrading

To upgrade `wassette` to the latest version, first update Homebrew's package list and then upgrade the `wassette` package.

```bash
brew update
brew upgrade wassette
```
