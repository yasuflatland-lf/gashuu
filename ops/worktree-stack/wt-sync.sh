#!/usr/bin/env bash
# wt-sync.sh [worktree] — bring the latest origin/main into this branch AS A MERGE COMMIT.
#
# Run this before landing any branch that was cut before main last moved. It is
# not ceremony: this repository automerges Renovate PRs, so `main` can advance
# while a stack is being landed, and a branch that has never seen the new main
# has never been tested against what it will actually land on.
#
# `--no-ff` is passed explicitly as well as `merge.ff false` being set by
# wt-new.sh, so the merge commit survives someone cloning without that config.
#
# On conflict this STOPS with the conflicted paths listed and leaves the merge
# in progress. Resolve by hand, then `git add` + `git commit --no-edit`. Do NOT
# reach for `git checkout --ours <file>` on a conflicted file: that takes YOUR
# whole version and silently drops whatever main added to the parts that merged
# cleanly. Resolve hunk by hunk.
set -uo pipefail

wt="${1:-$(git rev-parse --show-toplevel)}"
cd "$wt"

branch=$(git rev-parse --abbrev-ref HEAD)

if [ -n "$(git status --porcelain)" ]; then
  echo "refusing to sync: $wt has uncommitted changes"
  git status --short
  exit 1
fi

git fetch origin --quiet

if git merge-base --is-ancestor origin/main HEAD; then
  echo "already contains origin/main ($(git rev-parse --short origin/main)) — nothing to sync"
  exit 0
fi

echo "merging origin/main ($(git rev-parse --short origin/main)) into $branch"
if git merge --no-ff --no-edit origin/main; then
  echo "synced: $(git log --oneline -1)"
  echo "now re-run the gates before pushing:  bash ops/worktree-stack/wt-gates.sh"
  exit 0
fi

echo
echo "CONFLICT — merge left in progress. Conflicted paths:"
git diff --name-only --diff-filter=U
echo
echo "resolve each hunk, then:  git add <paths> && git commit --no-edit"
echo "then:                     bash ops/worktree-stack/wt-gates.sh"
exit 2
