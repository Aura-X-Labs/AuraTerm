---
title: Commit Message Standard
applyTo: ["*"]
description: |
  This skill defines the requirements for commit messages in this repository. It enforces English commit messages and requires that the message accurately describes the code changes. The commit message should be generated based on the actual code modifications.
---

# Commit Message Skill

## Requirements

- All commit messages **must be in English**.
- The commit message **must accurately describe the code changes**.
- The commit message should be **generated after code modifications are complete**.
- Use the following format for commit messages:

  1. **Short summary** (max 72 characters, imperative mood)
  2. **Detailed description** (if necessary, after a blank line)
  3. (Optional) **Reference to issues or PRs**

## Examples

- `Fix: resolve SSH connection timeout issue`
- `Refactor: extract pane layout logic into separate hook`
- `Docs: update README with new features`

## Workflow

1. Complete your code changes.
2. Review the changes and summarize them in a short, clear sentence (imperative mood, e.g., "Add", "Fix", "Update").
3. Write the commit message in English, following the format above.
4. If the change is complex, add a detailed description after a blank line.
5. Reference related issues or PRs if applicable.

## Enforcement

- PR reviewers should reject commits that do not follow this standard.
- Automated tools may be used to check commit message format.
