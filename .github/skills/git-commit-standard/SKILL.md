---
name: git-commit-standard
description: 'Use when committing code, submitting changes, creating a git commit, or when the user says "提交代码". Review the actual diff, stage only intended files, and write an English Conventional Commit message with a required body.'
argument-hint: 'Describe what should be committed if the request is ambiguous.'
user-invocable: true
---

# Git Commit Standard

## When to Use

- User asks to commit code
- User asks to submit changes
- User says "提交代码" or similar
- The task ends with a request to create a Git commit

## Requirements

- Review the actual changes before committing
- Stage only files related to the requested work
- Leave unrelated dirty files untouched
- Write the commit message in English
- Use Conventional Commits format
- Always include a body with bullet points that explain what changed and why
- Do not amend or push unless the user explicitly asks

## Procedure

### 1. Inspect the working tree

Check the current state with Git before staging or committing:

```bash
git status
git diff --staged
git diff
```

### 2. Stage only the intended files

Add only the files that belong to the requested change:

```bash
git add <files>
```

If unrelated files are dirty, keep them out of the commit.

### 3. Choose the commit message

Use Conventional Commits:

```text
<type>(<scope>): <description>

- Bullet explaining the main change
- Bullet explaining the reason or impact
```

Rules:
- Keep the summary under 72 characters
- Use imperative mood
- Prefer a scope when it adds clarity, such as `frontend`, `terminal`, `ssh`, `docs`, or `build`
- The body is required for every commit

### 4. Run relevant checks when feasible

For AuraTerm:
- Run `npm run build` for Vue and TypeScript changes when feasible
- Run `cd src-tauri && cargo check` for Rust and Tauri changes when feasible

If a check is skipped, say so explicitly in the final response.

### 5. Commit and report back

Create the commit only after reviewing the diff and staging the right files. Then report:
- The commit hash
- The commit summary
- Whether validation ran

## Important Rules

- Never ask the user to write the commit message for you
- Never commit without reviewing the diff first
- Never create an empty commit unless the user explicitly requests it
- Never stage unrelated files just to clear the working tree
- Never amend an existing commit unless the user explicitly requests it

## Examples

- `docs(copilot): add workspace instruction bootstrap`
- `fix(ssh): avoid per-keystroke invoke flooding`
- `refactor(terminal): split search state handling`