---
name: stacking-branches
description: Use when work spans several dependent branches, when a PR must be based on another branch instead of main, when landing more than one PR in sequence, or before any merge in this repository
---

# Stacking Branches

## Overview

Dependent work lands as a stack: link N is branched from link N-1, its PR is based on
link N-1's branch, and the stack lands bottom-up. **Every integration onto `main` is a
merge commit** — never a squash, never a rebase, never a fast-forward.

Not a style preference. Squashing link N-1 rewrites it into a commit link N does not have
as an ancestor; git loses the accurate merge base, and link N then conflicts against
changes it already contains.

Mechanics and failure playbook: **`ops/worktree-stack/README.md`. Read it before improvising.**

## The tool calls scripts cannot make

`EnterWorktree` is an agent tool, so these two steps are yours:

```
bash ops/worktree-stack/wt-new.sh <slug> <parent-branch>   # prints a path
```
```
EnterWorktree {path: "<that path>"}
```

**`EnterWorktree {name:}` cannot stack** — it always branches from `origin/<default>`. Use
it only for a standalone change off `main`. `{path:}` enters an existing worktree and works
even from inside another one.

## Quick reference

| Need | Run |
| --- | --- |
| Is a merge commit still guaranteed? | `ops/worktree-stack/preflight.sh` |
| New link in the stack | `ops/worktree-stack/wt-new.sh <slug> <parent>` |
| Are the gates green? | `ops/worktree-stack/wt-gates.sh` — **read `$?`, not the tail** |
| `main` moved under me | `ops/worktree-stack/wt-sync.sh` |
| Land the stack | `ops/worktree-stack/wt-merge.sh <pr> <pr> …` (lowest first) |

## Red flags — stop

- About to pass `--squash` or `--rebase` to `gh pr merge`
- Plain `git merge` without knowing `merge.ff` is `false`
- Merging without reading the check rollup — **`main` has no required checks, so
  `gh pr merge` will land a red PR**
- Judging a gates run by its tail instead of its exit code
- `git checkout --ours <file>` on a conflicted file
- Trusting merge settings observed earlier in the session

## Rationalizations

| Excuse | Reality |
| --- | --- |
| "Squash keeps main tidy" | It desynchronises the rest of the stack. Tidy main, broken link 4. |
| "I checked the merge settings already" | They flipped mid-run once. `preflight.sh` re-reads them every time. |
| "CI was green when I opened the PR" | It reruns on every push, and `wt-sync.sh` pushes. Re-read the rollup. |
| "`--ours` resolves this fastest" | It takes your whole file and drops what main added elsewhere in it. Resolve hunk by hunk. |
| "`EnterWorktree {name}` is the native tool" | Right for one branch, wrong for a stack. |
| "The gates printed a wall of PASS" | Gates run in parallel; a failing one prints before the green ones. |

## Common mistakes

**Renovate is a merge actor too.** If `renovate.json`'s `automergeStrategy` names a method
the repo forbids, GitHub rejects its merge call and automerge dies silently — the only
symptom is its PRs sitting open. `preflight.sh` checks that pairing.
