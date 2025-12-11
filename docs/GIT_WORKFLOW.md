# Multi-Device Git Workflow Guide

This guide describes the workflow for developing `hdr-analyze` across multiple machines (e.g., MacBook and VPS).

## Core Principles

1.  **Main Branch as Source of Truth**: The `main` branch on GitHub (`origin`) is the synchronization point.
2.  **Pull Before You Start**: Always pull changes from GitHub before you start working on a machine.
3.  **Push When Done**: Always commit and push your changes when you are finished with a session.

## Daily Workflow

### 1. Starting a Session (Any Machine)
Before writing any code, update your local repository to ensure you have the latest work from your other machines.

```bash
git checkout main
git pull origin main
```

### 2. Making Changes
Edit files, run tests, and verify your changes locally.

```bash
# Add your changes
git add .

# Commit with a descriptive message
git commit -m "feat: description of changes"
```

### 3. Ending a Session
Push your work to GitHub so it's available on your other machines.

```bash
# Push to the central repository
git push origin main
```

## Handling Conflicts
If you forget to pull and change the same file on two machines, `git pull` may result in a conflict.

1.  Git will tell you which files are conflicted.
2.  Open those files and look for the conflict markers (`<<<<<<<`, `=======`, `>>>>>>>`).
3.  Edit the file to resolve the conflict (choose which version to keep or combine them).
4.  Run `git add <file>` to mark it as resolved.
5.  Run `git commit` to finish the merge.
6.  Run `git push origin main`.
