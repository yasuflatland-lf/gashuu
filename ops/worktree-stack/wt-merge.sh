#!/usr/bin/env bash
# wt-merge.sh <pr> [pr...] — land PRs bottom-up, always with a merge commit.
#
# `main` in this repository has NO branch protection and NO required status
# checks, so `gh pr merge` will cheerfully land a PR whose CI is red. Every
# guard below is therefore load-bearing rather than belt-and-braces:
#
#   preflight   every integration path still produces a merge commit
#               (repo settings can be changed by anyone at any time — this was
#               observed flipping mid-run once, which broke a stack halfway)
#   drift       origin/main is EXACTLY the sha we last saw. Counting commits
#               does not work once merge commits are involved; compare the sha.
#               Drift means something landed from outside the stack, and the
#               next branch must be wt-sync.sh'd before it can be trusted.
#   retarget    GitHub repoints each stacked PR at main as its base lands.
#               Until that has happened, base != main and merging is wrong.
#   green       every check SUCCESS. This is the only thing standing between a
#               red PR and main.
#
# Already-merged PRs are skipped, so the script is safe to re-run after a stop.
# It stops at the FIRST problem rather than skipping ahead: in a stack, later
# links are built on earlier ones, so continuing past a bad link is meaningless.
set -uo pipefail

[ "$#" -ge 1 ] || { echo "usage: wt-merge.sh <pr> [pr...]"; exit 64; }

# Resolve the sibling script from THIS script's directory, not from the main
# checkout: the harness ships as a unit, and the main checkout's working tree may
# be on a branch that predates it (or be mid-edit) while this copy is the one the
# caller actually invoked.
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

echo "=== preflight ==="
if ! bash "$here/preflight.sh"; then
  echo
  echo ">>> refusing to merge: the merge-commit invariant is not currently held."
  exit 1
fi
echo

# No `cd` to the main checkout: refs are shared across worktrees, so origin/main
# resolves identically from any of them — and a worktree-isolated agent session
# is not permitted to redirect git at the shared checkout anyway.
git fetch origin --quiet
last_main=$(git rev-parse origin/main)
echo "origin/main = $(git log --oneline -1 origin/main)"
echo

for n in "$@"; do
  state=$(gh pr view "$n" --json state -q .state)
  if [ "$state" = "MERGED" ]; then
    echo "#$n already merged — skipping"
    continue
  fi

  git fetch origin --quiet
  now_main=$(git rev-parse origin/main)
  if [ "$now_main" != "$last_main" ]; then
    echo ">>> DRIFT before #$n: origin/main is now $(git log --oneline -1 origin/main)"
    echo ">>> something landed from outside this stack."
    echo ">>> run wt-sync.sh on #$n's branch, re-gate, push, then re-run this script."
    exit 2
  fi

  st=$(gh pr view "$n" --json baseRefName,mergeable,mergeStateStatus \
        -q '"\(.baseRefName)|\(.mergeable)|\(.mergeStateStatus)"')
  chk=$(gh pr view "$n" --json statusCheckRollup \
        -q '[.statusCheckRollup[]?|.conclusion//.state]|group_by(.)|map("\(.[0]):\(length)")|join(",")')
  echo "#$n  $st  checks=$chk"

  case "$st" in
    main\|MERGEABLE\|CLEAN) ;;
    main\|UNKNOWN\|UNKNOWN)
      echo ">>> #$n mergeability is still being computed by GitHub (normal for ~10s"
      echo ">>> right after the previous PR retargeted this one). Re-run shortly."
      exit 3 ;;
    main\|CONFLICTING\|*)
      echo ">>> #$n conflicts with main. Fix it on the branch:"
      echo ">>>   bash ops/worktree-stack/wt-sync.sh <its worktree>"
      exit 4 ;;
    *)
      echo ">>> #$n is not ready ($st). Base must be 'main' — has GitHub retargeted it yet?"
      exit 5 ;;
  esac

  case "$chk" in
    SUCCESS:*) ;;
    *) echo ">>> #$n CI is not fully green ($chk) — refusing"; exit 6 ;;
  esac

  if gh pr merge "$n" --merge; then
    sleep 5
    git fetch origin --quiet
    last_main=$(git rev-parse origin/main)
    echo "  MERGED #$n -> $(git log --oneline -1 origin/main)"
  else
    echo ">>> gh pr merge failed for #$n."
    echo ">>> If it said 'Merge commits are not allowed', the repository settings"
    echo ">>> changed under you — re-run preflight."
    exit 7
  fi
  echo
done

echo "=== main after the run ==="
git log --oneline -5 origin/main | cat
