#!/usr/bin/env bash
set -euo pipefail
# Record repository cache entries (ref, key, size, access time) and total usage.
output="${1:?usage: inventory.sh OUTPUT}"
mkdir -p "$(dirname "$output")"
gh api --paginate "/repos/$GITHUB_REPOSITORY/actions/caches?per_page=100&sort=last_accessed_at" \
  --jq '.actions_caches[] | {ref, key, size_in_bytes, created_at, last_accessed_at}' > "$output"
gh api "/repos/$GITHUB_REPOSITORY/actions/cache/usage" | tee "$output.usage"
printf '\n'
python3 - "$output" <<'EOF'
import json, sys
entries = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]
total = sum(entry["size_in_bytes"] for entry in entries)
print(f"cache_entries={len(entries)} cache_bytes={total}")
for entry in entries:
    print(f'{entry["last_accessed_at"]} {entry["size_in_bytes"]:>12} {entry["ref"]:<40} {entry["key"]}')
EOF
