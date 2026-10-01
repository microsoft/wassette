#!/usr/bin/env python3
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT license.

"""Install this checkout's CLI and finalized components from components/."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
from typing import Dict, List


def load_json(path: Path) -> Dict:
    value = json.loads(path.read_text())
    if not isinstance(value, dict):
        raise ValueError(f"{path} must contain a JSON object")
    return value


def declarations(root: Path) -> Dict[str, str]:
    names = load_json(root / "scripts/component-names.json")
    declaration = load_json(root / "scripts/component-outputs.json")
    outputs = declaration.get("install")
    exclusions = declaration.get("exclude")
    if not isinstance(outputs, dict) or not isinstance(exclusions, dict):
        raise ValueError("component-outputs.json requires install and exclude objects")
    classified = set(outputs) | (set(exclusions) & set(names))
    if classified != set(names):
        missing = sorted(set(names) - classified)
        extra = sorted(set(outputs) - set(names))
        raise ValueError(
            f"component install classifications disagree with component names; "
            f"missing={missing}, extra={extra}"
        )
    if set(outputs) & set(exclusions):
        overlap = sorted(set(outputs) & set(exclusions))
        raise ValueError(f"components cannot be both installed and excluded: {overlap}")
    if any(not project.startswith("components/") for project in outputs):
        raise ValueError("install outputs must come from components/")
    routing_fixture = "crates/wassette-acp/tests/fixtures/routing-provider"
    if routing_fixture not in exclusions:
        raise ValueError("the ACP routing fixture must remain explicitly excluded")
    return outputs


def validate_outputs(root: Path) -> List[Path]:
    outputs = declarations(root)
    names = load_json(root / "scripts/component-names.json")
    artifacts = []
    link_names = set()
    for project, relative in outputs.items():
        if not isinstance(relative, str):
            raise ValueError(f"invalid output path for {project}")
        artifact = (root / relative).resolve(strict=True)
        if artifact.suffix != ".wasm":
            raise ValueError(f"component output is not .wasm: {artifact}")
        if artifact.name in link_names:
            raise ValueError(f"duplicate component link filename: {artifact.name}")
        link_names.add(artifact.name)
        result = subprocess.run(
            ["wasm-tools", "metadata", "show", "--json", str(artifact)],
            check=True,
            capture_output=True,
            text=True,
        )
        metadata = json.loads(result.stdout)
        actual = metadata.get("component", {}).get("metadata", {}).get("name")
        if actual != names[project]:
            raise ValueError(
                f"{artifact} declares {actual!r}; expected {names[project]!r}"
            )
        artifacts.append(artifact)
    return artifacts


def check_prerequisites(root: Path) -> None:
    declarations(root)
    if os.name == "nt":
        raise ValueError("stable local-source links are not supported on native Windows")
    required = [
        "cargo",
        "rustup",
        "python3",
        "wasm-tools",
    ]
    missing = [command for command in required if shutil.which(command) is None]
    if missing:
        raise ValueError(f"missing required build tools: {', '.join(missing)}")
    installed = subprocess.run(
        ["rustup", "target", "list", "--installed"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.splitlines()
    if "wasm32-wasip2" not in installed:
        raise ValueError("missing Rust target wasm32-wasip2")


def cargo_target_directory(root: Path) -> Path:
    result = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
    )
    return Path(json.loads(result.stdout)["target_directory"])


def install(root: Path, mode: str) -> None:
    check_prerequisites(root)
    artifacts = validate_outputs(root)
    target_directory = cargo_target_directory(root)
    command = [
        "cargo",
        "install",
        "--path",
        str(root / "crates/wassette-mcp-server"),
        "--bin",
        "wassette",
        "--locked",
        "--force",
        "--target-dir",
        str(target_directory),
    ]
    if mode == "debug":
        command.append("--debug")
    subprocess.run(command, cwd=root, check=True)

    executable = target_directory / mode / (
        "wassette.exe" if os.name == "nt" else "wassette"
    )
    if not executable.is_file():
        raise FileNotFoundError(f"cargo did not leave the built executable at {executable}")
    sync = [
        str(executable),
        "component",
        "sync",
        "--output-format",
        "json",
        "--adopt-explicit-local",
    ]
    for artifact in artifacts:
        sync.extend(["--link", str(artifact)])
    subprocess.run(sync, cwd=root, check=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", choices=("debug", "release"), default="debug")
    parser.add_argument(
        "--check", action="store_true", help="validate declarations and build prerequisites"
    )
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    try:
        if args.check:
            check_prerequisites(root)
        else:
            install(root, args.mode)
    except (
        FileNotFoundError,
        OSError,
        ValueError,
        subprocess.CalledProcessError,
    ) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
