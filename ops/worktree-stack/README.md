# worktree-stack

Harness for taking a set of issues through **worktree → branch → PR → merge**, where
every integration onto `main` is a **merge commit** and nothing is ever squashed or
rebased away.

This is an **ops workflow harness**, not part of the build or the CI gates. Nothing here
runs automatically; `scripts/` holds the blocking gates, `ops/` holds tools you invoke.

## Why merge commits, enforced in four places

A squash is not just a stylistic preference to be re-decided per PR. In a *stack* — where
link N is branched from link N-1 — squashing an earlier link rewrites it into a commit
that later links do not have as an ancestor. Git then loses the accurate merge base, and
a later link that touches the same lines conflicts against changes it already contains.
The first stack landed here hit exactly that: a link whose whole job was deleting a
module conflicted with the squash of an earlier link that had introduced a caller of it.

So the invariant is *every* integration is a merge commit, and it has to hold for every
actor that can write to `main`, not just for the person running this harness:

| # | actor | how it could collapse history | held by |
| --- | --- | --- | --- |
| 1 | GitHub merge button / `gh pr merge` | squash or rebase merge | repository settings: `allow_merge_commit=true`, `allow_squash_merge=false`, `allow_rebase_merge=false` |
| 2 | Renovate automerge | `automergeStrategy` | `renovate.json` must ask for `"merge"` |
| 3 | a local `git merge` that happens to fast-forward | no merge commit is created at all | `merge.ff=false` (and `pull.rebase=false`) |
| 4 | this harness | `--squash` | `wt-merge.sh` only ever passes `--merge` |

`preflight.sh` checks all four. **Row 2 is the one that bites silently**: if the repo
forbids squash while `renovate.json` still asks for it, GitHub rejects Renovate's merge
call, automerge stops working, and the only symptom is Renovate PRs quietly piling up
open.

## The scripts

| script | what it does |
| --- | --- |
| `preflight.sh [--fix-local]` | asserts the four rows above. Exit 1 on any violation. `--fix-local` sets only this clone's git config; the repo-settings and `renovate.json` fixes are printed, not applied, because both are shared state. |
| `wt-new.sh <slug> [parent-ref]` | creates `.claude/worktrees/<slug>` on branch `wt/<slug>`, cut from `parent-ref` (default `origin/main`); symlinks the warm `target`, runs `mise trust`, sets `merge.ff=false`. Prints the path. |
| `wt-gates.sh [worktree]` | runs `mise run gates`. **The exit code is the verdict.** |
| `wt-sync.sh [worktree]` | merges the latest `origin/main` in as a merge commit. Stops on conflict with the paths listed. |
| `wt-merge.sh <pr> [pr...]` | lands PRs bottom-up with `gh pr merge --merge`, behind preflight + drift + retarget + all-checks-green guards. |

## Using it with EnterWorktree

`EnterWorktree` is an agent tool, not a shell command, so it cannot live inside these
scripts — the two are used together:

```
EnterWorktree {name: "<slug>"}          # standalone change, branches from origin/main
```

**For a stack, `name:` is the wrong form.** It always branches from
`origin/<default-branch>`, so it cannot cut link N from link N-1. Create the branch with
the right parent first, then switch in by path:

```sh
bash ops/worktree-stack/wt-new.sh 601-my-change wt/600-previous-link
```
```
EnterWorktree {path: "<the path wt-new.sh printed>"}
```

`path:` accepts any worktree already in `git worktree list`, provided it sits under
`.claude/worktrees/` of the same repository — which is why `wt-new.sh` puts it there
rather than making the location configurable. It also works while the session is already
inside another worktree, which is what makes walking a stack possible.

Leave with `ExitWorktree {action: "keep"}`. It will not remove a worktree entered by
`path:`; `git worktree remove` does that once the branch has landed.

> Inside a worktree session, Bash rejects compound commands — `for` loops, `>`
> redirects, several statements joined with `&&`. Issue plain single commands, and
> write files with the editor rather than a heredoc.

## Landing a stack

```sh
bash ops/worktree-stack/preflight.sh                 # once, before anything
bash ops/worktree-stack/wt-merge.sh 601 602 603      # bottom-up, lowest first
```

`wt-merge.sh` stops at the first problem instead of skipping ahead, because in a stack
every later link is built on the one below it. Two stops are routine rather than errors:

- **`UNKNOWN|UNKNOWN`** — GitHub is still recomputing mergeability after retargeting the
  next PR onto `main`. It settles in about ten seconds; re-run.
- **drift** — `origin/main` moved from outside the stack (Renovate landed something).
  `wt-sync.sh` the next branch, re-gate, push, wait for CI, re-run.

After a merge, GitHub retargets the next stacked PR onto `main` on its own, so PR bases
do not need to be edited by hand. Intermediate CI runs on `main` showing `cancelled`
during back-to-back merges are the Actions concurrency group superseding them — only the
tip's verdict counts.

## The shared `target` has one sharp edge

`wt-new.sh` symlinks every worktree's `target` at the main checkout's build directory,
which is what makes the gates fast. The cost: two worktrees are building into one place, so
**cross-branch comparisons through cargo are unreliable** — `cargo nextest list` run in one
worktree can report the other branch's test set. A gate run is unaffected (it rebuilds, and
its exit code is honest); only *comparisons between branches* are.

Compare blobs instead:

```sh
git show <branch>:crates/gashuu/src/viewer_state/tests.rs | grep -c '#\[test\]'
```

## Cleanup

```sh
rm .claude/worktrees/<slug>/target          # the SYMLINK; never the shared build dir
git worktree remove --force .claude/worktrees/<slug>
git branch -D wt/<slug>
```

Remove the symlink explicitly first. Scope any glob to the branches you created — a
`.claude/worktrees/6*-*` sweep will happily take an unrelated worktree with it.
