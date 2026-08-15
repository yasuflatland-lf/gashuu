#!/usr/bin/env bash
# wt-gates.sh [worktree] — run the five blocking gates. THE EXIT CODE IS THE VERDICT.
#
# `mise run gates` fans the five gates out in parallel, so its output ends with
# whichever gate finished last — a run with a failing clippy still prints a wall
# of green `PASS` test lines afterwards. Never judge the tail; read $?. This
# script exists so callers cannot get that wrong, and so the failure excerpt is
# extracted the same way every time (worth feeding straight back to an agent).
#
# The full log is left at <worktree>/.gates.log (untracked, see .gitignore below).
set -uo pipefail

wt="${1:-$(git rev-parse --show-toplevel)}"
cd "$wt"

mise run gates >".gates.log" 2>&1
rc=$?

if [ "$rc" -ne 0 ]; then
  echo "GATES FAILED (exit $rc) in $wt"
  grep -nE "error(\[|:)|^error|FAIL|panicked|Diff in|FAILED|failures:|warning: unused" ".gates.log" | head -60
  echo "--- tail ---"
  tail -40 ".gates.log"
else
  echo "GATES PASSED in $wt"
  grep -E "Summary|tests run" ".gates.log" | tail -2
fi

exit "$rc"
