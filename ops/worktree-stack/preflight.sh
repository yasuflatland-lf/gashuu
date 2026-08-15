#!/usr/bin/env bash
# preflight.sh — assert the "every integration is a merge commit" invariant.
#
# Four independent actors can put a commit on `main`, and each has its own way
# of collapsing history. This checks all four; a green run is what makes
# `wt-merge.sh` willing to merge anything.
#
#   1. GitHub's merge button / `gh pr merge`  -> repository merge settings
#   2. Renovate's automerge                   -> renovate.json automergeStrategy
#   3. A local `git merge` that could fast-forward -> merge.ff
#   4. This harness itself                    -> wt-merge.sh only ever passes --merge
#
# Usage:
#   bash ops/worktree-stack/preflight.sh              # check, exit 1 on any violation
#   bash ops/worktree-stack/preflight.sh --fix-local  # also set the LOCAL git config
#
# `--fix-local` deliberately touches nothing but this clone's git config. Repository
# settings and renovate.json are printed as the exact command / edit to make, because
# both are shared state: one is an API write other people see immediately, the other
# needs review like any tracked file.
set -uo pipefail

fix_local=0
[ "${1:-}" = "--fix-local" ] && fix_local=1

repo_root=$(git rev-parse --show-toplevel)
# --git-common-dir points at the MAIN checkout's .git even from inside a worktree.
main_checkout=$(cd "$(dirname "$(git rev-parse --git-common-dir)")" && pwd)
slug=$(gh repo view --json nameWithOwner -q .nameWithOwner 2>/dev/null || echo "")

fail=0
ok()   { printf '  OK   %s\n' "$*"; }
bad()  { printf '  FAIL %s\n' "$*"; fail=$((fail + 1)); }
info() { printf '       %s\n' "$*"; }

echo "=== 1. repository merge settings (${slug:-unknown repo}) ==="
if [ -z "$slug" ]; then
  bad "gh could not resolve the repository — is gh authenticated?"
else
  settings=$(gh api "repos/$slug" -q '"\(.allow_merge_commit) \(.allow_squash_merge) \(.allow_rebase_merge)"')
  read -r allow_merge allow_squash allow_rebase <<<"$settings"

  [ "$allow_merge" = "true" ]   && ok "allow_merge_commit  = true"  || bad "allow_merge_commit  = $allow_merge (merge commits are the ONLY method this repo should use)"
  [ "$allow_squash" = "false" ] && ok "allow_squash_merge  = false" || bad "allow_squash_merge  = $allow_squash (squash collapses a branch into one commit)"
  [ "$allow_rebase" = "false" ] && ok "allow_rebase_merge  = false" || bad "allow_rebase_merge  = $allow_rebase (rebase replays commits with no merge commit)"

  if [ "$allow_merge" != "true" ] || [ "$allow_squash" != "false" ] || [ "$allow_rebase" != "false" ]; then
    info "fix (shared state — run it yourself, deliberately):"
    info "  gh api -X PATCH repos/$slug -F allow_merge_commit=true -F allow_squash_merge=false -F allow_rebase_merge=false"
  fi
fi

echo
echo "=== 2. renovate.json automerge strategy ==="
renovate="$repo_root/renovate.json"
if [ ! -f "$renovate" ]; then
  ok "no renovate.json — nothing to reconcile"
else
  strategy=$(python3 -c "import json,sys; print(json.load(open(sys.argv[1])).get('automergeStrategy','<unset>'))" "$renovate" 2>/dev/null || echo "<unparseable>")
  case "$strategy" in
    merge)
      ok "automergeStrategy = \"merge\"" ;;
    "<unset>")
      bad "automergeStrategy is unset — Renovate defaults to the platform default, which can squash"
      info "fix: add  \"automergeStrategy\": \"merge\"  to renovate.json" ;;
    *)
      bad "automergeStrategy = \"$strategy\" but the repo forbids that method"
      info "This is not cosmetic: Renovate's merge call is REJECTED by GitHub, so"
      info "automerge silently stops working and its PRs pile up open."
      info "fix: set  \"automergeStrategy\": \"merge\"  in renovate.json" ;;
  esac
fi

echo
echo "=== 3. local git config (this clone: $repo_root) ==="
ff=$(git config --get merge.ff || echo "<unset>")
if [ "$ff" = "false" ]; then
  ok "merge.ff = false (a fast-forwardable merge still records a merge commit)"
elif [ "$fix_local" -eq 1 ]; then
  git config merge.ff false
  ok "merge.ff set to false"
else
  bad "merge.ff = $ff — \`git merge\` will fast-forward and leave NO merge commit"
  info "fix: bash ops/worktree-stack/preflight.sh --fix-local"
fi

pull_rebase=$(git config --get pull.rebase || echo "<unset>")
if [ "$pull_rebase" = "true" ]; then
  bad "pull.rebase = true — \`git pull\` would rewrite local commits instead of merging"
  info "fix: git config pull.rebase false"
else
  ok "pull.rebase = $pull_rebase"
fi

echo
echo "=== 4. worktree layout ==="
if [ "$repo_root" = "$main_checkout" ]; then
  ok "running in the main checkout ($main_checkout)"
else
  case "$repo_root" in
    "$main_checkout"/.claude/worktrees/*)
      ok "worktree under .claude/worktrees/ — EnterWorktree{path} can switch into it" ;;
    *)
      bad "worktree at $repo_root is outside $main_checkout/.claude/worktrees/"
      info "EnterWorktree{path} refuses to switch into it; use ops/worktree-stack/wt-new.sh" ;;
  esac
fi

echo
if [ "$fail" -eq 0 ]; then
  echo "preflight: PASS — every integration path on this repo produces a merge commit."
  exit 0
fi
echo "preflight: $fail violation(s) — refusing to certify the merge-commit invariant."
exit 1
