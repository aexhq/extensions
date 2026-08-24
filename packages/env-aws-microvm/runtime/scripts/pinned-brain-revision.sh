#!/usr/bin/env bash
set -euo pipefail

manifest=${1:-Cargo.toml}
mapfile -t revisions < <(
  sed -nE 's/^brain(-protocol)? = \{ git = "https:\/\/github\.com\/aexhq\/brain", rev = "([0-9a-f]{40})" \}$/\2/p' "$manifest"
)

if [ "${#revisions[@]}" -ne 2 ] || [ "${revisions[0]}" != "${revisions[1]}" ]; then
  echo "Brain and brain-protocol must use one exact 40-character aexhq/brain revision" >&2
  exit 1
fi

printf '%s\n' "${revisions[0]}"
