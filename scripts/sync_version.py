#!/usr/bin/env python3
"""
Sync version from package.json to Tauri config, Cargo.toml, and package-lock.json.
"""

import json
import os
import re
from pathlib import Path


def read_json(file_path: Path) -> dict:
    with open(file_path, "r", encoding="utf-8") as f:
        return json.load(f)


def write_json(file_path: Path, data: dict) -> None:
    with open(file_path, "w", encoding="utf-8") as f:
        json.dump(data, f, indent=2)
        f.write("\n")


def read_text(file_path: Path) -> str:
    with open(file_path, "r", encoding="utf-8") as f:
        return f.read()


def write_text(file_path: Path, content: str) -> None:
    with open(file_path, "w", encoding="utf-8") as f:
        f.write(content)


def update_tauri_version(tauri_config_path: Path, version: str) -> bool:
    config = read_json(tauri_config_path)
    if config.get("version") == version:
        return False

    config["version"] = version
    write_json(tauri_config_path, config)
    return True


def update_cargo_version(cargo_toml_path: Path, version: str) -> bool:
    cargo_toml = read_text(cargo_toml_path)
    updated = re.sub(
        r'^version\s*=\s*"[^"]*"\s*$',
        f'version = "{version}"',
        cargo_toml,
        count=1,
        flags=re.MULTILINE,
    )

    if updated == cargo_toml:
        return False

    write_text(cargo_toml_path, updated)
    return True


def update_package_lock_version(package_lock_path: Path, version: str) -> bool:
    if not package_lock_path.exists():
        return False

    package_lock = read_text(package_lock_path)
    updated = package_lock

    # Top-level lock file version.
    updated = re.sub(
        r'("version"\s*:\s*")([^"]+)(")',
        f'\\g<1>{version}\\g<3>',
        updated,
        count=1,
    )

    # Root package entry under packages[""].
    updated = re.sub(
        r'("packages"\s*:\s*\{\s*""\s*:\s*\{\s*"name"\s*:\s*"[^"]+",\s*"version"\s*:\s*")([^"]+)(")',
        f'\\g<1>{version}\\g<3>',
        updated,
        count=1,
        flags=re.DOTALL,
    )

    if updated == package_lock:
        return False

    write_text(package_lock_path, updated)
    return True


def main() -> None:
    root = Path.cwd()
    package_json_path = root / "package.json"
    tauri_config_path = root / "src-tauri" / "tauri.conf.json"
    cargo_toml_path = root / "src-tauri" / "Cargo.toml"
    package_lock_path = root / "package-lock.json"

    pkg = read_json(package_json_path)
    version = pkg.get("version", "")

    if not version or not version.strip():
        print("Error: package.json version is missing.")
        exit(1)

    changed_files = []

    if update_tauri_version(tauri_config_path, version):
        changed_files.append("src-tauri/tauri.conf.json")

    if update_cargo_version(cargo_toml_path, version):
        changed_files.append("src-tauri/Cargo.toml")

    if update_package_lock_version(package_lock_path, version):
        changed_files.append("package-lock.json")

    if not changed_files:
        print(f"Version already synced: {version}")
        return

    print(f"Synced version {version} to:")
    for file in changed_files:
        print(f"- {file}")


if __name__ == "__main__":
    main()
