#!/usr/bin/env python3
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT license.

"""Embed an explicitly declared first-party producer name before publication."""

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def read_u32(data: bytes, offset: int) -> tuple:
    value = 0
    for shift in range(0, 35, 7):
        if offset >= len(data):
            raise ValueError("truncated component section length")
        byte = data[offset]
        offset += 1
        if shift == 28 and byte > 15:
            raise ValueError("component section length overflows u32")
        value |= (byte & 127) << shift
        if byte < 128:
            return value, offset
    raise ValueError("invalid component section length")


def check_name_sections(data: bytes) -> None:
    if data[:8] != b"\0asm\x0d\0\x01\0":
        raise ValueError("expected a binary WebAssembly component")
    offset = 8
    name_sections = 0
    while offset < len(data):
        section_id = data[offset]
        size, start = read_u32(data, offset + 1)
        offset = start + size
        if offset > len(data):
            raise ValueError("truncated component section")
        if section_id != 0:
            continue
        section = data[start:offset]
        length, start = read_u32(section, 0)
        if start + length > len(section):
            raise ValueError("truncated custom section name")
        if section[start:start + length] == b"component-name":
            name_sections += 1
    # wasm-tools updates each root name section separately, producing duplicate names.
    if name_sections > 1:
        raise ValueError("multiple root component-name sections; fix the producer first")


def name_component(project: Path, artifact: Path) -> None:
    root = Path(__file__).resolve().parent.parent
    declarations = json.loads((root / "scripts/component-names.json").read_text())
    project_key = project.resolve().relative_to(root).as_posix()
    if project_key not in declarations:
        raise ValueError(f"no authored component name for project {project_key}")
    name = declarations[project_key]
    if not isinstance(name, str) or not name.strip() or any(
        ord(c) < 32 or 127 <= ord(c) <= 159 for c in name
    ):
        raise ValueError(f"invalid authored component name for project {project_key}")

    artifact = artifact.resolve(strict=True)
    check_name_sections(artifact.read_bytes())

    with tempfile.NamedTemporaryFile(
        prefix=".name-component-", suffix=".wasm", dir=artifact.parent, delete=False
    ) as output:
        temporary = Path(output.name)
    try:
        subprocess.run(
            [
                "wasm-tools", "metadata", "add", "--name", name,
                str(artifact), "--output", str(temporary),
            ],
            check=True,
        )
        temporary.chmod(artifact.stat().st_mode)
        os.replace(temporary, artifact)
    finally:
        temporary.unlink(missing_ok=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("project", type=Path, help="declared first-party source project")
    parser.add_argument("artifact", type=Path, help="component built from that project")
    args = parser.parse_args()
    try:
        name_component(args.project, args.artifact)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
