#!/usr/bin/env python3
"""
Release script for AuraTerm
Handles version generation, file operations, and JSON updates
"""

import json
import os
import re
import shutil
import sys
import hashlib
import platform
from datetime import datetime
from pathlib import Path


def get_version_from_config():
    """Extract version from tauri.conf.json"""
    config_path = Path("src-tauri/tauri.conf.json")
    if not config_path.exists():
        print("Error: tauri.conf.json not found")
        sys.exit(1)
    
    with open(config_path, 'r', encoding='utf-8') as f:
        config = json.load(f)
        version = config.get('version', '0.1.0')
        # Extract major.minor only
        parts = version.split('.')
        return f"{parts[0]}.{parts[1]}"


def get_latest_patch_version(version_base, mmdd, ext):
    """Get the latest patch version for today's date"""
    releases_dir = Path("releases")
    if not releases_dir.exists():
        return 0
    
    pattern = f"AuraTerm-{version_base}.[0-9]*.{mmdd}-*.{ext}"
    existing_files = list(releases_dir.glob(pattern))
    
    if not existing_files:
        return 0
    
    patches = []
    for f in existing_files:
        match = re.search(rf'{version_base}\.(\d+)\.{mmdd}', f.name)
        if match:
            patches.append(int(match.group(1)))
    
    return max(patches) + 1 if patches else 0


def get_architecture():
    """Get system architecture"""
    system = platform.system()
    machine = platform.machine().lower()
    
    if system == 'Windows':
        return 'x64'
    elif machine == 'arm64':
        return 'arm64'
    elif machine in ['amd64', 'x86_64']:
        return 'x64'
    else:
        return machine


def find_artifact():
    """Find the built artifact"""
    system = platform.system()
    arch = get_architecture()
    arch_tauri = 'aarch64' if arch == 'arm64' else arch
    
    if system == 'Windows':
        pattern = f"src-tauri/target/release/bundle/nsis/AuraTerm_*_{arch_tauri}-setup.exe"
        ext = 'exe'
    elif system == 'Darwin':
        pattern = f"src-tauri/target/release/bundle/dmg/AuraTerm_*_{arch_tauri}.dmg"
        ext = 'dmg'
    else:
        pattern = f"src-tauri/target/release/bundle/appimage/AuraTerm_*_{arch_tauri}.AppImage"
        ext = 'appimage'
    
    import glob
    files = glob.glob(pattern)
    
    if not files:
        print(f"Error: Artifact not found at {pattern}")
        sys.exit(1)
    
    return files[0], ext, arch


def calculate_sha256(filepath):
    """Calculate SHA256 hash of file"""
    sha256_hash = hashlib.sha256()
    with open(filepath, "rb") as f:
        for byte_block in iter(lambda: f.read(4096), b""):
            sha256_hash.update(byte_block)
    return sha256_hash.hexdigest()


def update_releases_json(version, filename, platform_name, sha256):
    """Update or create releases JSON file"""
    releases_dir = Path("releases")
    releases_dir.mkdir(exist_ok=True)
    
    json_path = releases_dir / "auraterm-releases.json"
    publish_date = datetime.now().strftime('%Y-%m-%d')
    
    release_info = {
        "version": version,
        "filename": filename,
        "platform": platform_name,
        "published_at": publish_date,
        "sha256": sha256,
        "notes": "automated release"
    }
    
    if json_path.exists():
        try:
            with open(json_path, 'r', encoding='utf-8') as f:
                data = json.load(f)
        except:
            data = {"product": "AuraTerm", "latest": "", "releases": []}
        
        data["latest"] = version
        data["releases"] = [release_info] + data["releases"]
    else:
        data = {
            "product": "AuraTerm",
            "latest": version,
            "releases": [release_info]
        }
    
    with open(json_path, 'w', encoding='utf-8') as f:
        json.dump(data, f, indent=2)
    
    return json_path


def main():
    """Main release process"""
    print("Processing artifacts...")
    
    # Get version info
    version_base = get_version_from_config()
    mmdd = datetime.now().strftime('%m%d')
    
    # Find artifact
    artifact_path, ext, arch = find_artifact()
    
    # Calculate patch version
    patch = get_latest_patch_version(version_base, mmdd, ext)
    full_version = f"{version_base}.{patch}.{mmdd}"
    
    # Prepare destination
    releases_dir = Path("releases")
    releases_dir.mkdir(exist_ok=True)
    
    dest_name = f"AuraTerm-{full_version}-{arch}.{ext}"
    dest_path = releases_dir / dest_name
    
    # Copy files
    shutil.copy2(artifact_path, dest_path)
    
    latest_path = releases_dir / f"AuraTerm-latest-{arch}.{ext}"
    shutil.copy2(artifact_path, latest_path)
    
    # Calculate hash
    sha256 = calculate_sha256(artifact_path)
    
    # Determine platform
    system = platform.system()
    if system == 'Windows':
        platform_name = 'windows-x64'
    elif system == 'Darwin':
        platform_name = f'macos-{arch}'
    else:
        platform_name = f'linux-{arch}'
    
    # Update JSON
    json_path = update_releases_json(full_version, dest_name, platform_name, sha256)
    
    print(f"Local release prepared: releases/{dest_name}")
    print(f"JSON updated: {json_path}")


if __name__ == "__main__":
    main()
