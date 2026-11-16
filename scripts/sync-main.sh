#!/usr/bin/env bash
# sync-main.sh — bring origin/main into this isolation branch, safely.
#
# This lane works in its own worktree (briev-intel-xpu / intel-xpu-harvest)
# because `main` is a shared trunk with concurrent agent sessions. The shared
# trunk is normally checked out at ../briev-lang; never touch that worktree.
#
# Usage (from the lane worktree):
#   bash scripts/sync-main.sh            # fetch + merge if needed
#   bash scripts/sync-main.sh --check    # report only, change nothing
#   bash scripts/sync-main.sh --abort    # abandon an in-progress merge
#
# Exit codes:
#   0  already up to date, or merge completed with no conflicts
#   2  merge stopped on conflicts (recipe printed; working tree left for you)
#   3  refused: not on the lane branch, or uncommitted changes present
#
# After a merge, run the gates (the script prints them):
#   cargo build --release && cargo test --lib && bash scripts/gate.sh quick
set -euo pipefail

LANE_BRANCH="${BRIEV_LANE_BRANCH:-intel-xpu-harvest}"
TRUNK="${BRIEV_TRUNK:-main}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

mode="sync"
case "${1:-}" in
    --check) mode="check" ;;
    --abort) mode="abort" ;;
    "") ;;
    *) echo "usage: $0 [--check|--abort]" >&2; exit 2 ;;
esac

if [ "$mode" = "abort" ]; then
    git merge --abort 2>/dev/null && echo "sync-main: in-progress merge aborted" || echo "sync-main: no merge to abort"
    exit 0
fi

branch="$(git branch --show-current)"
if [ "$branch" != "$LANE_BRANCH" ]; then
    echo "sync-main: refusing — on '$branch', expected '$LANE_BRANCH'." >&2
    echo "  This lane works on its own branch; 'main' is a shared trunk" >&2
    echo "  (usually checked out at ../briev-lang by another session)." >&2
    exit 3
fi

if [ -n "$(git status --porcelain)" ]; then
    echo "sync-main: refusing — uncommitted changes present. Commit or stash them first" >&2
    echo "  (never 'git checkout --' / 'git restore': they destroy work)." >&2
    exit 3
fi

echo "sync-main: fetching origin/$TRUNK"
git fetch --quiet origin "$TRUNK"

ahead=$(git rev-list --count "HEAD..origin/$TRUNK")
if [ "$ahead" -eq 0 ]; then
    echo "sync-main: already up to date with origin/$TRUNK ($(git rev-parse --short HEAD))"
    exit 0
fi

if git merge-base --is-ancestor "origin/$TRUNK" HEAD; then
    echo "sync-main: origin/$TRUNK is already an ancestor of $LANE_BRANCH — nothing to merge"
    exit 0
fi

echo "sync-main: $ahead commit(s) to merge from origin/$TRUNK:"
git log --oneline "HEAD..origin/$TRUNK" | sed 's/^/  /'
echo "sync-main: files touched:"
git diff --name-only "HEAD...origin/$TRUNK" | sed 's/^/  /'

if [ "$mode" = "check" ]; then
    echo "sync-main: --check, nothing changed"
    exit 0
fi

if git merge --no-edit "origin/$TRUNK" >/tmp/sync-main-merge.log 2>&1; then
    echo "sync-main: merged cleanly -> $(git rev-parse --short HEAD)"
    echo "sync-main: now run: cargo build --release && cargo test --lib && bash scripts/gate.sh quick"
    exit 0
fi

echo "sync-main: merge stopped on conflicts:"
git status --porcelain | grep -E '^(UU|AA|DU|UD)' | sed 's/^/  /' || true
cat <<'RECIPE'

  Resolution recipes (both have occurred on every merge so far):

  BUGS.md — both sides prepend dated entries. Keep BOTH, newest first.
    Resolution by hand: delete the conflict markers, put this branch's new
    entries first, then main's.

  src/hir/owned_storage.rs — if main only ran `cargo fmt --all`, the hunks are
    formatting, not logic. Verify with:
      git diff -w --stat <merge-base> origin/main -- src/hir/owned_storage.rs
    (or compare with whitespace stripped: the two differ only by trailing
    commas). Then take this branch's side and re-run rustfmt:
      git checkout --ours src/hir/owned_storage.rs && rustfmt src/hir/owned_storage.rs

  After resolving:  git add <files> && git commit
  Or abandon:       bash scripts/sync-main.sh --abort
RECIPE
exit 2
