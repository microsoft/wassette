#!/usr/bin/env python3
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT license.

"""Write a component-generation operator profile for local files.

The profile pins the builder helper and an operator-supplied initrd by their
SHA-256 digests and permits build and install. Exposure and rebuilds stay off
unless explicitly requested. Nothing is downloaded or published, and an
existing profile is never overwritten.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import sys
from typing import List


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for chunk in iter(lambda: file.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def existing_file(value: str) -> Path:
    path = Path(value).expanduser().resolve()
    if not path.is_file():
        raise argparse.ArgumentTypeError(f"not a file: {value}")
    return path


def profile(args: argparse.Namespace, staging: Path) -> dict:
    dependencies: List[str] = [path.read_text() for path in args.wit_dependency]
    return {
        "builder": {
            "helper_path": str(args.helper),
            "helper_sha256": sha256(args.helper),
            "initrd_path": str(args.initrd),
            "initrd_sha256": sha256(args.initrd),
            "staging_root": str(staging),
            "wit_dependencies": dependencies,
        },
        "allow_build": True,
        "allow_install": True,
        "allow_expose": args.allow_expose,
        "allow_rebuild": args.allow_rebuild,
        "callers": [],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--helper",
        type=existing_file,
        required=True,
        help="signed wassette-builder helper, e.g. the one `just install-generation` installed",
    )
    parser.add_argument(
        "--initrd", type=existing_file, required=True, help="trusted local builder initrd"
    )
    parser.add_argument(
        "--output", type=Path, required=True, help="profile path to create; must not exist"
    )
    parser.add_argument(
        "--staging-root",
        type=Path,
        help="private builder staging directory (default: generation-staging next to the profile)",
    )
    parser.add_argument(
        "--wit-dependency",
        type=existing_file,
        action="append",
        default=[],
        help="complete WIT package file available to requests; repeat in dependency-first order",
    )
    parser.add_argument(
        "--allow-expose", action="store_true", help="also permit ordinary-tool exposure"
    )
    parser.add_argument(
        "--allow-rebuild", action="store_true", help="also permit rebuilding generated components"
    )
    args = parser.parse_args()

    output = args.output.expanduser().resolve()
    staging = (args.staging_root or output.parent / "generation-staging").expanduser().resolve()
    try:
        if output.exists():
            raise FileExistsError(f"refusing to overwrite {output}")
        document = json.dumps(profile(args, staging), indent=2) + "\n"
        output.parent.mkdir(parents=True, exist_ok=True)
        staging.mkdir(mode=0o700, parents=True, exist_ok=True)
        descriptor = os.open(output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "w") as file:
            file.write(document)
    except (OSError, UnicodeDecodeError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    print(f"Wrote {output}", file=sys.stderr)
    print(f"Pass `--generation-config {output}` to `wassette acp`.", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
