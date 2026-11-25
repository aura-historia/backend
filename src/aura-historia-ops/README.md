# Ops commands

`aura-historia-ops` (the image's `ops` entrypoint) holds the host's one-shot operator and
scheduled commands. Each command runs to completion, prints a JSON report to stdout and exits
non-zero on failure (`2` for invalid arguments). Host timers run the scheduled ones
([schedule](../../host/prod/README.md#scheduled-jobs)). Settings and secrets come from the environment or `NAME_FILE`
files as in [host process startup](../platform-process/README.md).

| Command | Purpose |
| --- | --- |
| `migrate --plan <file>` | Applies a reviewed migration plan as the migrator role (`POSTGRES_MIGRATOR_USERNAME`/`_PASSWORD`, `DATABASE_TARGET`) |
| `inspect-schema` | Prints the migration ledger and schema fingerprint as the read-only inspector role (`POSTGRES_INSPECTOR_USERNAME`/`_PASSWORD`) |
| `bootstrap` | On a new database, creates the runtime, migrator and inspector roles as an administrator (`POSTGRES_ADMIN_USERNAME`/`_PASSWORD`, `POSTGRES_RUNTIME_PASSWORD`, `POSTGRES_MIGRATOR_PASSWORD`, `POSTGRES_INSPECTOR_PASSWORD`, and optionally `POSTGRES_CRAWLER_INGESTION_PASSWORD` so the crawler's enqueue-only role can log in) from [the host role SQL](../../infra/sql/host-bootstrap-roles-core.sql) and applies every migration |
| `inspect` | Per job scope: depth, leased count, oldest waiting age and custody; outbox pending count, oldest age and custody |
| `redrive <scope> --all \| <job-id>...` | Moves custody jobs back with their identity and order; `<scope>` `delivery-outbox` takes outbox IDs |
| `reproject-all (--live \| --generation <g>) [--page-size <n>] [--start-after <listing-id>]` | Enqueues one projector job per listing; the target is never implicit. Pass the report's `last` as `--start-after` to resume |
| `renormalize-raw (--dry-run \| --apply) [--listing-source <ls-id>] [--page-size <n>] [--limit <n>] [--after <prs-id>] [--max-backlog <n>]` | Normalizes the latest raw revision of caught-up streams again with this build's normalizer ([reapplication](../../docs/normalization-replay.md#production-reapplication)); writing needs `--apply`. Pass the report's `last` as `--after` to resume |
| `backend-cleanup` | Expired-data cleanup and recovery maintenance (`EXPIRY_CLEANUP_BATCH_SIZE`); every step runs and the command fails if any did |
| `fx-refresh` | Captures the FX rate snapshot (`FXRATES_API_TOKEN`); a rerun in the same hour is a duplicate |
| `periodic-match` | Periodic saved-search matching; stops cleanly on SIGTERM |

Arguments are validated before any connection is opened. Custody is never deleted by any
command. The [worker runbook](../../docs/durable-worker-runbook.md) owns when to inspect and
redrive, and the [projection runbook](../../docs/durable-worker-runbook.md#projection-fences-and-rebuild)
owns rebuilds.
