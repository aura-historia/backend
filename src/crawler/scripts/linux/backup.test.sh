#!/usr/bin/env bash
# Exercises backup-to-s3.sh against a fake aws (objects are files under $FAKE_S3/<bucket>/) and pg_dump.
# Run: bash src/crawler/scripts/linux/backup.test.sh
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
fail() { echo "FAIL: $*" >&2; cat "$tmp/out" >&2 2>/dev/null || true; exit 1; }

mkdir -p "$tmp/fakebin" "$tmp/s3/bucket"
cat > "$tmp/fakebin/pg_dump" <<'T'
#!/usr/bin/env bash
echo "pg_dump $*" >> "$FAKE_LOG"
echo "PGPASSWORD=${PGPASSWORD-unset}" >> "$FAKE_LOG"
echo "first half of the dump"
[[ -z "${FAKE_PG_FAIL:-}" ]] || exit 1
echo "second half of the dump"
T
cat > "$tmp/fakebin/aws" <<'T'
#!/usr/bin/env bash
echo "aws $*" >> "$FAKE_LOG"
object_file() { echo "$FAKE_S3/${1#s3://}"; }
case "$1 $2" in
  "s3 cp")
    [[ "$3" == - ]] || exit 2
    file="$(object_file "$4")"; mkdir -p "$(dirname "$file")"; cat > "$file"
    args="$*"
    [[ "$args" != *--storage-class* ]] || { class="${args#*--storage-class }"; echo "${class%% *}" > "$file.class"; } ;;
  "s3 rm") rm -f "$(object_file "$3")" "$(object_file "$3").class" ;;
  "s3api head-object")
    while [[ $# -gt 0 ]]; do case "$1" in --bucket) b="$2"; shift 2 ;; --key) k="$2"; shift 2 ;; *) shift ;; esac; done
    [[ -f "$FAKE_S3/$b/$k" ]] || exit 254
    wc -c < "$FAKE_S3/$b/$k" | tr -d ' ' ;;
  "s3api list-objects-v2")
    while [[ $# -gt 0 ]]; do case "$1" in --bucket) b="$2"; shift 2 ;; --prefix) p="$2"; shift 2 ;; *) shift ;; esac; done
    (cd "$FAKE_S3/$b" && find . -type f ! -name '*.class' -printf '%P\n' | LC_ALL=C sort | grep -F -- "$p" || true) | paste -sd '\t' - ;;
  *) echo "unexpected: $*" >&2; exit 2 ;;
esac
T
chmod +x "$tmp"/fakebin/*
export PATH="$tmp/fakebin:$PATH" FAKE_LOG="$tmp/log" FAKE_S3="$tmp/s3"
echo "LOCAL_DB_URL=\"postgres://crawler@127.0.0.1/crawler\"" > "$tmp/crawler.env"
export CRAWLER_ENV_FILE="$tmp/crawler.env" CRAWLER_BACKUP_LOCK="$tmp/lock" CRAWLER_BACKUP_S3_URI="s3://bucket/crawler/"
backup() { bash "$SCRIPT_DIR/backup-to-s3.sh" > "$tmp/out" 2>&1; }
dumps() { find "$FAKE_S3/bucket" -type f -name '*.dump' | wc -l | tr -d ' '; }
markers() { find "$FAKE_S3/bucket" -type f -name '*.dump.complete' | wc -l | tr -d ' '; }
# The fake keys differ by the second; the real script's names sort by time in the same way.
run_at() { sleep 1.1; backup; }

backup || fail "a backup failed"
[[ "$(dumps)" == 1 && "$(markers)" == 1 ]] || fail "a backup leaves one dump and its marker"
dump="$(find "$FAKE_S3/bucket/crawler" -type f -name '*.dump')"
[[ "$(cat "$dump")" == $'first half of the dump\nsecond half of the dump' ]] || fail "the dump was not streamed whole"
[[ "$(cat "$dump.class")" == STANDARD_IA ]] || fail "the dump is not stored in STANDARD_IA"
grep -q -- '--dbname=postgres://crawler@127.0.0.1/crawler' "$FAKE_LOG" || fail "pg_dump got another database"
grep -q -- '--format=custom --no-owner --compress=6' "$FAKE_LOG" || fail "pg_dump got other options"
grep -qx 'PGPASSWORD=unset' "$FAKE_LOG" || fail "a URL without a password set PGPASSWORD"
grep -q -- '--expected-size 107374182400' "$FAKE_LOG" || fail "a stream needs an expected size"

# Retention keeps the newest five.
for _ in 1 2 3 4 5; do run_at || fail "a backup failed"; done
[[ "$(dumps)" == 5 && "$(markers)" == 5 ]] || fail "retention kept $(dumps) dumps and $(markers) markers instead of 5"
[[ ! -e "$dump" ]] || fail "retention removed a newer dump instead of the oldest"

# A failed pg_dump leaves nothing behind and keeps the earlier dumps.
before="$(find "$FAKE_S3/bucket" -type f | sort)"
sleep 1.1
if FAKE_PG_FAIL=1 backup; then fail "a failed pg_dump was reported as a backup"; fi
[[ "$(find "$FAKE_S3/bucket" -type f | sort)" == "$before" ]] || fail "a failed pg_dump left an object or removed a dump"

# A truncated dump that could not be deleted is removed by the next run: it has no marker.
echo partial > "$FAKE_S3/bucket/crawler/crawler-19990101T000000Z.dump"
run_at || fail "a backup failed"
[[ ! -e "$FAKE_S3/bucket/crawler/crawler-19990101T000000Z.dump" ]] || fail "a dump without a marker was kept"
[[ "$(dumps)" == 5 ]] || fail "retention left $(dumps) dumps"

# Other keys under the prefix are never touched.
echo keep > "$FAKE_S3/bucket/crawler/notes.txt"
run_at || fail "a backup failed"
[[ -f "$FAKE_S3/bucket/crawler/notes.txt" ]] || fail "retention removed a file that is not a dump"

# A password in the URL reaches pg_dump through PGPASSWORD, decoded, never in its arguments.
echo 'LOCAL_DB_URL="postgresql://crawler:s3cr%40t%2Fpw@127.0.0.1:5432/crawler"' > "$tmp/crawler.env"
: > "$FAKE_LOG"; run_at || fail "a backup with a password failed"
grep -q 's3cr' <(grep '^pg_dump ' "$FAKE_LOG") && fail "the password is in pg_dump's arguments"
grep -q -- '--dbname=postgresql://crawler@127.0.0.1:5432/crawler' "$FAKE_LOG" || fail "pg_dump got another database without the password"
grep -qx 'PGPASSWORD=s3cr@t/pw' "$FAKE_LOG" || fail "pg_dump did not get the decoded password"
echo 'LOCAL_DB_URL="postgres://crawler@127.0.0.1/crawler"' > "$tmp/crawler.env"

# Settings are checked before anything is uploaded.
: > "$FAKE_LOG"
CRAWLER_BACKUP_S3_URI="bucket/crawler" backup && fail "a bad S3 URI was accepted"
CRAWLER_BACKUP_KEEP=0 backup && fail "keeping no dump was accepted"
CRAWLER_BACKUP_COMPRESS=12 backup && fail "a bad compression level was accepted"
CRAWLER_ENV_FILE="$tmp/missing.env" backup && fail "a missing environment file was accepted"
! grep -q . "$FAKE_LOG" || fail "bad settings still ran pg_dump or aws"
echo "backup tests passed"
