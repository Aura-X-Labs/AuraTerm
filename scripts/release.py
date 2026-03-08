#!/usr/bin/env python3
"""
Release metadata script for AuraTerm.
Updates releases JSON from already-built target artifacts.
"""

import json
import re
import sys
import hashlib
from datetime import datetime
from pathlib import Path
from typing import Optional


def get_version_from_package_json():
    """Extract version from package.json (single source of truth)."""
    package_path = Path("package.json")
    if not package_path.exists():
        print("Error: package.json not found")
        sys.exit(1)

    with open(package_path, 'r', encoding='utf-8') as f:
        package = json.load(f)
        version = package.get('version')

    if not version:
        print("Error: package.json version is missing")
        sys.exit(1)

    parts = version.split('.')
    if len(parts) < 2:
        print(f"Error: invalid package.json version '{version}'")
        sys.exit(1)

    # Keep full semantic version (e.g. 0.1.5)
    return version


def normalize_arch(arch: str) -> str:
    """Normalize architecture values to user-facing labels."""
    mapping = {
        "x86_64": "x64",
        "amd64": "x64",
        "aarch64": "arm64",
    }
    return mapping.get(arch.lower(), arch.lower())


def latest_artifact(pattern: str) -> Optional[Path]:
    """Return the newest artifact that matches a glob pattern."""
    files = sorted(Path().glob(pattern), key=lambda p: p.stat().st_mtime, reverse=True)
    return files[0] if files else None


def find_target_artifacts():
    """Find newest build artifacts in target bundle folders."""
    patterns = [
        "src-tauri/target/release/bundle/nsis/AuraTerm_*_*-setup.exe",
        "src-tauri/target/release/bundle/dmg/AuraTerm_*_*.dmg",
        "src-tauri/target/release/bundle/appimage/AuraTerm_*_*.AppImage",
    ]

    artifacts = []
    for pattern in patterns:
        artifact = latest_artifact(pattern)
        if artifact:
            artifacts.append(artifact)

    if not artifacts:
        print("Error: No target artifacts found. Run build first.")
        sys.exit(1)

    return artifacts


def infer_platform_and_arch(artifact: Path):
    """Infer platform and architecture from artifact filename/path."""
    name = artifact.name
    suffix = artifact.suffix.lower()

    arch = "x64"
    if suffix == ".exe":
        match = re.search(r"_([^-_]+)-setup\.exe$", name)
        if match:
            arch = normalize_arch(match.group(1))
        platform_name = f"windows-{arch}"
    elif suffix == ".dmg":
        match = re.search(r"_([^_]+)\.dmg$", name)
        if match:
            arch = normalize_arch(match.group(1))
        platform_name = f"macos-{arch}"
    else:
        match = re.search(r"_([^_]+)\.AppImage$", name)
        if match:
            arch = normalize_arch(match.group(1))
        platform_name = f"linux-{arch}"

    return platform_name, arch


def extract_version_from_filename(filename: str) -> Optional[str]:
    """Extract semantic version from standard AuraTerm artifact names."""
    match = re.search(r"AuraTerm_(\d+\.\d+\.\d+)", filename)
    if match:
        return match.group(1)
    return None


def calculate_sha256(filepath):
    """Calculate SHA256 hash of file"""
    sha256_hash = hashlib.sha256()
    with open(filepath, "rb") as f:
        for byte_block in iter(lambda: f.read(4096), b""):
            sha256_hash.update(byte_block)
    return sha256_hash.hexdigest()


def update_releases_json(release_infos, latest_version):
    """Update or create releases JSON file using provided release entries."""
    releases_dir = Path("releases")
    releases_dir.mkdir(exist_ok=True)
    
    json_path = releases_dir / "auraterm-releases.json"
    publish_date = datetime.now().strftime('%Y-%m-%d')
    
    for info in release_infos:
        info["published_at"] = publish_date
        info.setdefault("notes", "automated release")

    if json_path.exists():
        try:
            with open(json_path, 'r', encoding='utf-8') as f:
                data = json.load(f)
        except:
            data = {"product": "AuraTerm", "latest": "", "releases": []}

        existing = data.get("releases", [])
        for release_info in release_infos:
            # Upsert by filename + platform to avoid duplicate records.
            replaced = False
            for idx, item in enumerate(existing):
                if item.get("filename") == release_info["filename"] and item.get("platform") == release_info["platform"]:
                    existing[idx] = release_info
                    replaced = True
                    break
            if not replaced:
                existing.insert(0, release_info)

        data["latest"] = latest_version
        data["releases"] = existing
    else:
        data = {
            "product": "AuraTerm",
            "latest": latest_version,
            "releases": release_infos
        }
    
    with open(json_path, 'w', encoding='utf-8') as f:
        json.dump(data, f, indent=2)
    
    return json_path


def main():
    """Update releases JSON metadata from target artifacts."""
    print("Updating release metadata...")

    package_version = get_version_from_package_json()
    artifacts = find_target_artifacts()

    release_infos = []
    for artifact in artifacts:
        version = extract_version_from_filename(artifact.name) or package_version
        platform_name, _ = infer_platform_and_arch(artifact)
        sha256 = calculate_sha256(artifact)

        release_infos.append({
            "version": version,
            "filename": artifact.name,
            "platform": platform_name,
            "sha256": sha256,
            "notes": "automated release"
        })

    json_path = update_releases_json(release_infos, package_version)

    print(f"JSON updated: {json_path}")
    for info in release_infos:
        print(f"  - {info['filename']} ({info['platform']})")


if __name__ == "__main__":
    main()
