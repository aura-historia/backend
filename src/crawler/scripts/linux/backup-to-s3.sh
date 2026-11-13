#!/usr/bin/env bash
# Streams a compressed pg_dump of the crawler-local database straight to S3, without a local file,
# and keeps the newest CRAWLER_BACKUP_KEEP dumps. For crawler-backup.timer (weekly).
# A dump is only trusted once its crawler-<time>.dump.complete marker exists: a stream that broke
# halfway still leaves an object, so the marker is written last and a dump without one is deleted.
# Needs the AWS CLI and pg_dump on the host. Settings (an environment file read by the unit):
#   CRAWLER_BACKUP_S3_URI          s3://<bucket>[/<prefix>] (required)
#   CRAWLER_BACKUP_KEEP            dumps to keep (default 5)
#   CRAWLER_BACKUP_STORAGE_CLASS   storage class of the dump (default STANDARD_IA)
#   CRAWLER_BACKUP_EXPECTED_SIZE   upper bound of the dump in bytes, which the AWS CLI needs to size
#                                  the parts of a stream (default 107374182400, 100 GiB)
#   CRAWLER_BACKUP_COMPRESS        pg_dump compression level 0-9 (default 6)
# Contract: docs/crawler/operations.md#scheduled-s3-backups
set -euo pipefail

ENV_FILE="${CRAWLER_ENV_FILE:-/etc/crawler/crawler.env}"
LOCK_FILE="${CRAWLER_BACKUP_LOCK:-/var/lib/crawler/backup.lock}"
S3_URI="${CRAWLER_BACKUP_S3_URI:-}"
KEEP="${CRAWLER_BACKUP_KEEP:-5}"
STORAGE_CLASS="${CRAWLER_BACKUP_STORAGE_CLASS:-STANDARD_IA}"
EXPECTED_SIZE="${CRAWLER_BACKUP_EXPECTED_SIZE:-107374182400}"
COMPRESS="${CRAWLER_BACKUP_COMPRESS:-6}"
readonly DUMP_SUFFIX=.dump
readonly MARKER_SUFFIX=.dump.complete

if [[ ! "$S3_URI" =~ ^s3://([a-z0-9][a-z0-9.-]{1,61}[a-z0-9])(/(.+))?$ ]]; then
  echo "CRAWLER_BACKUP_S3_URI must look like s3://<bucket>[/<prefix>]" >&2
  exit 2
fi
bucket="${BASH_REMATCH[1]}"
prefix="${BASH_REMATCH[3]:-}"
prefix="${prefix%/}"
[[ -z "$prefix" ]] || prefix="${prefix}/"
for number in "$KEEP" "$EXPECTED_SIZE"; do
  [[ "$number" =~ ^[0-9]+$ ]] || { echo "CRAWLER_BACKUP_KEEP and _EXPECTED_SIZE must be whole numbers" >&2; exit 2; }
done
((KEEP >= 1)) || { echo "CRAWLER_BACKUP_KEEP must keep at least one dump" >&2; exit 2; }
[[ "$COMPRESS" =~ ^[0-9]$ ]] || { echo "CRAWLER_BACKUP_COMPRESS must be a level from 0 to 9" >&2; exit 2; }

url="$(sed -n 's/^LOCAL_DB_URL=//p' "$ENV_FILE" | head -n1 | sed -e 's/^"//' -e 's/"$//' -e "s/^'//" -e "s/'\$//")"
[[ -n "$url" ]] || { echo "LOCAL_DB_URL is not set in ${ENV_FILE}" >&2; exit 1; }
# The password goes to pg_dump through PGPASSWORD, never in its arguments: /proc/<pid>/cmdline is
# readable by every local user for as long as the dump runs, its environment only by root and the
# owner. URLs percent-encode reserved characters, so the password is decoded first, as libpq would.
if [[ "$url" =~ ^(postgres(ql)?://[^:@/]*):([^@/]*)@(.*)$ ]]; then
  password="${BASH_REMATCH[3]}"
  password="$(printf '%b' "${password//%/\\x}")"
  url="${BASH_REMATCH[1]}@${BASH_REMATCH[4]}"
  export PGPASSWORD="$password"
fi

# One backup at a time: a run that outlasts the week must not overlap the next.
mkdir -p "$(dirname -- "$LOCK_FILE")"
exec 9> "$LOCK_FILE"
flock -n 9 || { echo "another backup is running" >&2; exit 1; }

key="${prefix}crawler-$(date -u +%Y%m%dT%H%M%SZ)${DUMP_SUFFIX}"
object="s3://${bucket}/${key}"

# Deletes a dump that did not finish, so a truncated object is never mistaken for a backup. A failed
# delete is left for the cleanup of unmarked dumps below at the next run.
finished=false
cleanup() {
  $finished || aws s3 rm "$object" --only-show-errors >&2 || true
}
trap cleanup EXIT

echo "streaming the crawler database to ${object}"
started=$SECONDS
pg_dump --format=custom --no-owner --compress="$COMPRESS" --dbname="$url" \
  | aws s3 cp - "$object" --storage-class "$STORAGE_CLASS" --expected-size "$EXPECTED_SIZE" --only-show-errors

size="$(aws s3api head-object --bucket "$bucket" --key "$key" --query ContentLength --output text)"
[[ "$size" =~ ^[0-9]+$ && "$size" -gt 0 ]] || { echo "the uploaded dump is empty" >&2; exit 1; }
printf '%s\n' "$size" | aws s3 cp - "s3://${bucket}/${key%"$DUMP_SUFFIX"}${MARKER_SUFFIX}" --only-show-errors
finished=true
echo "backed up ${size} bytes in $((SECONDS - started))s"

# Retention: the newest KEEP marked dumps stay; older ones and dumps without a marker (a stream that
# died and could not be deleted) go. The name sorts by time.
listing="$(aws s3api list-objects-v2 --bucket "$bucket" --prefix "$prefix" --query 'Contents[].Key' --output text | tr '\t' '\n')"
mapfile -t keys < <(grep -E "crawler-[0-9]{8}T[0-9]{6}Z\.dump(\.complete)?\$" <<< "$listing" | LC_ALL=C sort || true)
marked=()
for candidate in "${keys[@]}"; do
  [[ "$candidate" != *"$MARKER_SUFFIX" ]] || marked+=("${candidate%"$MARKER_SUFFIX"}${DUMP_SUFFIX}")
done
keep_from=$((${#marked[@]} > KEEP ? ${#marked[@]} - KEEP : 0))
for candidate in "${keys[@]}"; do
  dump="${candidate%.complete}"
  keep=false
  for ((i = keep_from; i < ${#marked[@]}; i++)); do
    [[ "$dump" != "${marked[i]}" ]] || { keep=true; break; }
  done
  # The dump just written has its marker already, so it is in `marked` and kept.
  $keep || { echo "removing s3://${bucket}/${candidate}"; aws s3 rm "s3://${bucket}/${candidate}" --only-show-errors; }
done
