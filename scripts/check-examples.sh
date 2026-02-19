#!/usr/bin/env bash
set -euo pipefail

cd /Users/bradofrim/git/super_yaml

if ! command -v jq >/dev/null 2>&1; then
  echo "Error: 'jq' is required but was not found in PATH." >&2
  exit 2
fi

for syaml in examples/*.syaml; do
  base="$(basename "$syaml" .syaml)"
  expected="examples/${base}.expected.json"

  if [[ ! -f "$expected" ]]; then
    echo "Skipping ${base} (missing ${expected})"
    continue
  fi

  actual="$(cargo run --quiet --bin super-yaml -- compile "$syaml" --pretty)"

  if ! diff -u <(echo "$actual" | jq -S .) <(jq -S . "$expected"); then
    echo "Mismatch: ${base}"
    exit 1
  fi

  echo "OK: ${base}"
done

echo "All example outputs match."
