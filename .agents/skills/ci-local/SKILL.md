---
name: ci-local
description: Reproduce Wassette's CI locally with the Docker-based just recipes — full CI runs, non-Docker build and test, optional GHCR registry tests, and cache inspection or cleanup. Use to debug CI failures or validate changes in a CI-equivalent environment.
allowed-tools: Bash, Read, Write, Edit, Glob, Grep
---

# ci-local skill

Run Wassette's CI locally to reproduce failures in a CI-equivalent environment.
The Docker recipes automatically map your user to prevent permission issues.

## Running CI

```bash
just ci-local             # Run CI tests locally with Docker
just ci-build-test        # Build and test without Docker
just ci-build-test-ghcr   # Build and test, including GHCR registry tests
```

## Cache and cleanup

```bash
just ci-cache-info        # Show Docker cache information
just ci-clean             # Remove Docker images and cache
```

## Environment variables

- `GITHUB_TOKEN`: required for CI and GHCR (GitHub Container Registry) tests.
- `RUST_LOG`: set log verbosity (`info`, `debug`, `trace`).
