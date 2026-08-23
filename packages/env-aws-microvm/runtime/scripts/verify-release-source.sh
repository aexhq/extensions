#!/usr/bin/env bash
set -euo pipefail

expected_commit=${1:-}
case "$expected_commit" in
  *[!0-9a-f]*|'')
    echo "expected_commit must be 40 lowercase hexadecimal characters" >&2
    exit 1
    ;;
esac

test "${#expected_commit}" -eq 40
test "$GITHUB_REF" = "refs/tags/release/sha-$expected_commit"
test "$(git cat-file -t "$GITHUB_REF")" = tag
test "$(git rev-parse HEAD)" = "$expected_commit"
git merge-base --is-ancestor "$expected_commit" origin/main
