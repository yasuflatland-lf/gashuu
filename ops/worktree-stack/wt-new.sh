#!/usr/bin/env bash
# wt-new.sh <slug> [parent-ref] — create a worktree that EnterWorktree can enter.
#
# WHY THIS EXISTS RATHER THAN `EnterWorktree {name}`:
#   EnterWorktree's `name:` form always branches from origin/<default-branch>
#   (`worktree.baseRef` defaults to `fresh`). That is correct for a standalone
#   change and USELESS for a stack, where link N must be cut from link N-1.
#   So: create the branch here with the parent you want, then switch the session
#   in with `EnterWorktree {path: "<printed path>"}`. That form accepts any
#   worktree already in `git worktree list`, as long as it lives under
#   .claude/worktrees/ of the same repository — which is why the path below is
#   fixed rather than configurable.
#
# Also does the three things a fresh worktree needs before it is usable here:
#   - `mise trust`, or every `mise exec -- cargo ...` silently no-ops
#   - `target` symlinked to the main checkout's warm build dir, which takes
#     `mise run gates` from ~50s to a couple of seconds. `.gitignore`'s
#     `/target/` has a trailing slash and matches directories only, so the
#     SYMLINK needs `/target` in .git/info/exclude — asserted below.
#   - `merge.ff false`, so `wt-sync.sh` records a merge commit even when the
#     merge could fast-forward.
#
# Prints the absolute worktree path as its last stdout line.
set -euo pipefail

slug="${1:?usage: wt-new.sh <slug> [parent-ref]}"
parent="${2:-origin/main}"

main_checkout=$(cd "$(dirname "$(git rev-parse --git-common-dir)")" && pwd)
branch="wt/${slug}"
wt="${main_checkout}/.claude/worktrees/${slug}"

cd "$main_checkout"
git fetch origin --quiet

if [ ! -d "$wt" ]; then
  if git show-ref --verify --quiet "refs/heads/${branch}"; then
    git worktree add "$wt" "$branch" >&2
  else
    git worktree add -b "$branch" "$wt" "$parent" >&2
  fi
fi

# Shared warm target. Replace a real directory left by an earlier run.
if [ -e "$wt/target" ] && [ ! -L "$wt/target" ]; then
  rm -rf "$wt/target"
fi
[ -L "$wt/target" ] || ln -s "${main_checkout}/target" "$wt/target"

if ! grep -qx '/target' "${main_checkout}/.git/info/exclude" 2>/dev/null; then
  {
    echo ""
    echo "# local-only: shared cargo target symlink in worktrees (gitignore's /target/ misses symlinks)"
    echo "/target"
  } >> "${main_checkout}/.git/info/exclude"
  echo "note: appended /target to .git/info/exclude so the symlink is not reported as untracked" >&2
fi

mise trust "$wt" >&2 2>/dev/null || (cd "$wt" && mise trust >&2)
git -C "$wt" config merge.ff false
git -C "$wt" config pull.rebase false

{
  echo "worktree : $wt"
  echo "branch   : $branch"
  echo "parent   : $parent -> $(git -C "$wt" rev-parse --short "$parent" 2>/dev/null || echo '?')"
  echo "head     : $(git -C "$wt" log --oneline -1)"
  echo
  echo "next: EnterWorktree {path: \"$wt\"}"
} >&2

echo "$wt"
