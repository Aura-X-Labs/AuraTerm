#!/usr/bin/env python3
"""
Upload script for AuraTerm
Uploads target bundle artifacts and releases JSON to remote server
"""

import sys
import subprocess
from pathlib import Path


def latest_file(pattern):
    """Return the latest file that matches a glob pattern."""
    files = sorted(Path().glob(pattern), key=lambda p: p.stat().st_mtime, reverse=True)
    return files[0] if files else None


def get_target_artifacts():
    """Collect latest target artifacts to upload from bundle output."""
    patterns = [
        "src-tauri/target/release/bundle/nsis/AuraTerm_*_*-setup.exe",
        "src-tauri/target/release/bundle/dmg/AuraTerm_*_*.dmg",
    ]

    artifacts = []
    for pattern in patterns:
        artifact = latest_file(pattern)
        if artifact:
            artifacts.append(artifact)

    return artifacts


def upload_files(files_to_upload):
    """Upload files to remote server"""
    import shutil
    
    # Check if scp and ssh are available
    if not shutil.which("scp") or not shutil.which("ssh"):
        print("Error: scp and ssh commands not found")
        print("Please ensure OpenSSH is installed and in PATH")
        sys.exit(1)
    
    # Build scp command
    scp_args = ["scp"]
    for f in files_to_upload:
        scp_args.append(str(f))
    scp_args.append("william@alithon.com:Downloads/AuraTerm/")
    
    print(f"Uploading files...")
    for f in files_to_upload:
        print(f"  - {f.name}")
    
    try:
        # Upload files
        subprocess.run(scp_args, check=True)
        
        # Copy to release directory on server
        filenames = [f.name for f in files_to_upload if f.name != "auraterm-releases.json"]
        cp_cmds = [f"cp Downloads/AuraTerm/{name} /home/william/workspace/release/aurax/releases/" for name in filenames]
        cp_cmds.append("cp Downloads/AuraTerm/auraterm-releases.json /home/william/workspace/release/aurax/releases/")
        
        ssh_cmd = " && ".join(cp_cmds)
        subprocess.run(["ssh", "william@alithon.com", ssh_cmd], check=True)
        
        print("Upload complete.")
        
    except subprocess.CalledProcessError as e:
        print(f"Error during upload: {e}")
        sys.exit(1)


def main():
    """Main upload process"""
    files_to_upload = get_target_artifacts()
    if not files_to_upload:
        print("Error: No target exe/dmg artifacts found. Run build first.")
        sys.exit(1)

    # Add JSON file
    json_file = Path("releases/auraterm-releases.json")
    if json_file.exists():
        files_to_upload.append(json_file)
    else:
        print("Error: auraterm-releases.json not found")
        sys.exit(1)
    
    upload_files(files_to_upload)


if __name__ == "__main__":
    main()
