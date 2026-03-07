#!/usr/bin/env python3
"""
Upload script for AuraTerm
Uploads the latest version and releases JSON to remote server
"""

import re
import sys
import subprocess
from pathlib import Path


def get_latest_version_file(ext):
    """Find the file with the highest version number"""
    releases_dir = Path("releases")
    if not releases_dir.exists():
        print("Error: releases/ directory not found")
        sys.exit(1)
    
    # Pattern: AuraTerm-0.1.2.0305-x64.exe
    pattern = f"AuraTerm-*-x64.{ext}"
    files = list(releases_dir.glob(pattern))
    
    if not files:
        return None
    
    # Parse version numbers and find the highest
    def extract_version(filepath):
        match = re.search(r'AuraTerm-(\d+\.\d+\.\d+\.\d+)-', filepath.name)
        if match:
            version_str = match.group(1)
            # Convert to tuple for comparison (0.1.2.0305 -> (0, 1, 2, 305))
            parts = version_str.split('.')
            return tuple(int(p) for p in parts)
        return (0, 0, 0, 0)
    
    latest_file = max(files, key=extract_version)
    return latest_file


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
    files_to_upload = []
    
    # Find latest exe and add both versioned and latest files
    latest_exe = get_latest_version_file("exe")
    if latest_exe:
        files_to_upload.append(latest_exe)
        # Add latest-x64.exe file
        latest_link = Path("releases/AuraTerm-latest-x64.exe")
        if latest_link.exists():
            files_to_upload.append(latest_link)
    else:
        print("Warning: No exe files found")
    
    # Find latest dmg and add both versioned and latest files
    latest_dmg = get_latest_version_file("dmg")
    if latest_dmg:
        files_to_upload.append(latest_dmg)
        # Add latest-x64.dmg file
        latest_link = Path("releases/AuraTerm-latest-x64.dmg")
        if latest_link.exists():
            files_to_upload.append(latest_link)
    else:
        print("Warning: No dmg files found")
    
    # Add JSON file
    json_file = Path("releases/auraterm-releases.json")
    if json_file.exists():
        files_to_upload.append(json_file)
    else:
        print("Error: auraterm-releases.json not found")
        sys.exit(1)
    
    if not files_to_upload:
        print("Error: No files to upload")
        sys.exit(1)
    
    upload_files(files_to_upload)


if __name__ == "__main__":
    main()
