---
name: amux2
description: Park this worktree on the `amux2` branch synced to latest main. Use when the user says "switch to amux2 and update from main", "sync amux2", "park amux2", or similar after finishing PR follow-up work. Fetches origin, checks out (or creates) the `amux2` branch, and fast-forwards it to origin/main.
user-invocable: true
---

# amux2 — park worktree on `amux2` branch at latest main

## How this worktree is used

The workspace at `/Users/daveowen/RiderProjects/amux2` is the
**PR-follow-up worktree** for the amux repo. Its sibling at
`/Users/daveowen/RiderProjects/amux` is the **main feature-work
worktree** and usually holds whatever branch the user is actively
building on.

The typical lifecycle here:
1. Park on branch `amux2` (sitting at latest `main`).
2. When a PR needs follow-up review fixes / tweaks, `git checkout`
   the PR's feature branch in this worktree and do the work here.
3. Merge the PR from this worktree.
4. Switch back to branch `amux2` and update it to the new `main`.

**Why park on `amux2`, not `main`:** git worktrees can only have a
given branch checked out in one place at a time. If this worktree sat
on `main`, the sibling worktree couldn't check out `main` (e.g., to
pull or fast-forward after a merge). The `amux2` branch is a
throwaway parking spot that nothing else touches, so leaving it
checked out here doesn't lock anything for the sibling worktree.

## What this skill does

Puts this worktree back on branch `amux2` with that branch pointing at
`origin/main`'s current tip.

## Steps

1. Confirm we're in the right worktree (`pwd` should end in `amux2`).
2. Fetch the remote:
   ```
   git fetch origin
   ```
3. Check out the `amux2` branch. If it doesn't exist yet, create it
   tracking `origin/main`:
   ```
   # if branch exists locally:
   git checkout amux2
   # if it doesn't:
   git checkout -b amux2 origin/main
   ```
4. Reset `amux2` to `origin/main`. Since `amux2` is a parking branch
   with no independent commits, `reset --hard` is safe and preferred
   over merge/rebase — it keeps the branch pointer exactly at
   `origin/main` every time:
   ```
   git reset --hard origin/main
   ```
5. Report the before/after SHAs. If nothing changed, say "already at
   origin/main".

## Guardrails

- Never create commits on the `amux2` branch. It is a parking
  pointer, not a working branch.
- Never push the `amux2` branch to origin — it's local-only.
- If `git status` shows uncommitted changes before this skill runs,
  **stop and ask the user**. `reset --hard` would discard them.
- If the user has a feature branch currently checked out here (i.e.,
  they were in the middle of PR follow-up), ask before switching
  away — they may have unpushed work.
