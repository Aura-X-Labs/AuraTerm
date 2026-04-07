---
name: git-commit-standard
description: Use this skill when committing code, submitting changes, creating git commits, or when the user says "提交代码". It requires reviewing the actual diff, staging only intended files, and writing English Conventional Commit messages with a required body.
---

# Git Commit Standard

## Overview

Provide standardized Git commit workflow ensuring:
- Commit messages in English
- Messages generated from actual code changes
- Follow conventional commit format
- Only intended files are staged and committed
- Repository validation is run when feasible before commit

## When to Use

Use this skill when:
- User requests to commit code
- User asks to submit changes
- User says "提交代码" or similar

## Commit Message Format

Follow Conventional Commits specification:

```
<type>(<scope>): <description>

<body>

[optional footer(s)]
```

**IMPORTANT: Body is REQUIRED for every commit.**

### Types

| Type | Description |
|------|-------------|
| `feat` | New feature |
| `fix` | Bug fix |
| `docs` | Documentation changes |
| `style` | Code style changes (formatting, semicolons, etc.) |
| `refactor` | Code refactoring without feature changes |
| `perf` | Performance improvements |
| `test` | Adding or modifying tests |
| `chore` | Build process, dependencies, tooling |
| `ci` | CI/CD configuration changes |
| `revert` | Revert previous commit |

## Workflow

### Step 1: Analyze Changes

Before committing, analyze the actual code changes:

```bash
git status
git diff --staged
git diff
```

Also check whether unrelated files are dirty. Do not include unrelated changes in the same commit.

### Step 2: Stage Changes

Stage relevant files:

```bash
git add <files>
```

Rules:
- Stage only files that belong to the requested change
- If unrelated changes exist, leave them unstaged
- Do not create empty commits

### Step 3: Generate Commit Message

Generate commit message based on actual changes:
- Review all modified files
- Identify the primary change type
- Pick a scope when it improves clarity, such as `frontend`, `ssh`, `terminal`, `docs`, or `build`
- Write concise description in English
- Keep first line under 72 characters
- **ALWAYS include a body** explaining what and why (not how)
- Body should list key changes as bullet points

### Step 4: Commit

```bash
git commit -m "<type>(<scope>): <description>" -m "<body>"
```

Or use multi-line format:
```bash
git commit -m "<type>(<scope>): <description>" -m "" -m "- Change 1" -m "- Change 2"
```

### Step 5: Verify Result

After committing:
- Confirm the commit succeeded
- Report the commit hash and summary to the user
- Do not amend or push unless the user explicitly asks

### Step 6: Run Relevant Checks When Feasible

For this repository:
- Run `npm run build` for Vue and TypeScript changes when feasible
- Run `cd src-tauri && cargo check` for Rust and Tauri changes when feasible
- If checks are skipped, say so explicitly in the response

## Examples

| Changes | Commit Message |
|---------|---------------|
| Add user authentication | `feat(auth): add user authentication module`<br><br>- Implement JWT token validation<br>- Add login/logout endpoints<br>- Create auth middleware |
| Fix login validation | `fix(login): correct password validation logic`<br><br>- Fix regex pattern for password strength<br>- Add missing null check<br>- Update error messages |
| Update README | `docs: update installation instructions`<br><br>- Add Windows setup guide<br>- Update Node.js version requirement<br>- Fix broken links |
| Refactor database module | `refactor(db): simplify connection pooling`<br><br>- Extract connection logic to separate module<br>- Remove redundant error handling<br>- Add connection timeout config |
| Upgrade dependencies | `chore(deps): upgrade xterm to v5.5.0`<br><br>- Migrate to @xterm scoped packages<br>- Update all addon imports<br>- Fix deprecated API usage |
| Add input history feature | `feat(input): add PageUp/PageDown history navigation`<br><br>- Store last 100 input commands<br>- Add keyboard shortcut handling<br>- Display history in input bar |

## Best Practices

1. **One logical change per commit** - Don't mix unrelated changes
2. **Use imperative mood** - "add feature" not "added feature"
3. **No period at end** - Keep first line clean
4. **Reference issues** - Add `#123` for issue references
5. **Breaking changes** - Use `!` after type: `feat!: breaking API change`

## Important Rules

- NEVER ask user for commit message content
- ALWAYS analyze git diff to understand changes
- ALWAYS write commit message in English
- ALWAYS include a body with bullet points explaining changes
- NEVER commit without reviewing changes first
- Body should answer "what" and "why", not "how"
- NEVER stage or commit unrelated files just to get a clean working tree
- NEVER amend an existing commit unless the user explicitly requests it
- ALWAYS tell the user what was committed and whether validation ran
