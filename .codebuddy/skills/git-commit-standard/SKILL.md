---
name: git-commit-standard
description: This skill should be used when committing code changes. It ensures commit messages are written in English and generated based on the actual code modifications, following conventional commit standards.
---

# Git Commit Standard

## Overview

Provide standardized Git commit workflow ensuring:
- Commit messages in English
- Messages generated from actual code changes
- Follow conventional commit format

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

### Step 2: Stage Changes

Stage relevant files:

```bash
git add <files>
```

### Step 3: Generate Commit Message

Generate commit message based on actual changes:
- Review all modified files
- Identify the primary change type
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
