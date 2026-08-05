---
name: copyright-headers
description: Ensure every Rust (.rs) file in Wassette starts with the required Microsoft copyright header, using the idempotent scripts/copyright.sh helper. Use when adding new Rust files or when a copyright-header check fails.
allowed-tools: Bash, Read, Write, Edit, Glob, Grep
---

# copyright-headers skill

All Rust (`.rs`) files in Wassette must begin with the Microsoft copyright
header, placed at the very top of the file and followed by a blank line before
any other content (including crate-level documentation and imports).

## Required format

```rust
// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.
```

## Apply headers

Run the idempotent helper, which adds the header to any file missing it and
leaves existing headers untouched, so it is safe to run repeatedly:

```bash
./scripts/copyright.sh
```

## Verify

```bash
grep -q "Copyright (c) Microsoft Corporation" your_file.rs
```

Apply the header before committing new files; the format must match exactly as
shown above.
