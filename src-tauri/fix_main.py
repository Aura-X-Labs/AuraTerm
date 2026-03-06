import re

with open("/Users/bill/workspace/aura/AuraTerm/src-tauri/src/main.rs", "r") as f:
    text = f.read()

text = re.sub(
    r"\.manage\(app_state\)\n\s*\.manage\(ssh::SshState::default\(\)\)\n\s*\.manage\(ssh::SshState::default\(\)\)",
    ".manage(app_state)\n        .manage(ssh::SshState::default())",
    text
)

with open("/Users/bill/workspace/aura/AuraTerm/src-tauri/src/main.rs", "w") as f:
    f.write(text)

print("Done")
