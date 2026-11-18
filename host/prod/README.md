# Prod Compose project

[`compose.yaml`](compose.yaml) runs the prod runtime on the host as one Compose project,
`aura-prod`, from images built from this repository:

| Service | Tier | Role |
| --- | --- | --- |
| `postgres` | serving | PostgreSQL 16 with data checksums and pgBackRest ([image](postgres/Dockerfile)) |
| `api-a`, `api-b` | serving | Two API replicas behind Caddy's health checks |
| `mcp` | serving | MCP server |
| `caddy` | serving | The only public listener, 443 ([image](caddy/Dockerfile), [config](caddy/Caddyfile)) |
| `worker` | pipeline | Outbox dispatcher and queue consumers |
| `ops`, `migrate`, `bootstrap`, `restore-check` | pipeline | One-shot commands, never started by `up` |

OpenSearch keeps its own project ([compose](../../opensearch/prod/compose.yaml)) and volumes;
the application containers and Caddy join its `search` network, so it must be running first.
The crawler stack stays in its own project in the batch tier. Tiers, memory limits and the
firewall are in the [host guide](../README.md).

Every command goes through [`compose.sh`](compose.sh) as root, from the stable checkout:
`sudo host/prod/compose.sh ps`, `... logs -f worker`, `... run --rm ops inspect`.

## Networks

- `database` is internal: PostgreSQL and the application containers, no route out.
- `backup` is PostgreSQL's only route out, for pgBackRest's S3 traffic.
- `default` gives the application containers and Caddy their egress, and Caddy its upstreams.
- `search` is the OpenSearch project's network. The application processes connect to
  `https://opensearch:9200` and trust the node's private CA (`OPENSEARCH_CA_CERT_FILE`).

Nothing but Caddy publishes a port.

## Secrets

`/etc/aura-historia/prod/` holds everything the project reads that is not in the repository.
It is root-owned and mode 0700; Compose reads its env files as root and passes each service
only the files it lists.

| File | Content |
| --- | --- |
| `compose.env` | Compose interpolation: application image, its revision, backup bucket |
| `settings.env` | Non-secret deployment settings: Cognito, Vertex AI, Stripe IDs, email senders |
| `secrets/<credential>.env` | One credential per file, mode 0600 |
| `opensearch-root-ca.pem` | The OpenSearch node's CA certificate (public), mode 0644 |
| `google-credentials.json` | The keyless [Vertex AI configuration](../../docs/infra.md#vertex-ai-credentials), mode 0644 |
| `backup-recipient.asc` | The operators' OpenPGP public key for the encrypted copy |

[`templates/`](templates) lists every file and key. `sudo host/prod/secrets.sh init` creates
the missing files from it, generating the PostgreSQL role passwords and the pgBackRest
passphrase so that each value agrees across the files that share it; it never overwrites a
file. Fill in the rest, then `sudo host/prod/secrets.sh check` must pass. Sources:

- `aws.env` and `pgbackrest.env`: the host IAM user's access key ([host access](../../docs/infra.md#host-access)); rotate both files together.
- `caddy.env`: a Cloudflare API token limited to *Zone.DNS edit* on the zone, and the
  [origin secret](../../docs/infra.md#cloudfront-origin-lock).
- `cognito-trigger.env`: the [trigger signing key](../../docs/infra.md#host-access).
- `opensearch-*.env` and `opensearch-root-ca.pem`: the OpenSearch stable checkout's
  `.private/` directory.
- The provider keys and `settings.env`: the former Secrets Manager secrets and SSM parameters
  under `/aura-historia/prod/` and `/<provider>/prod/`.

Changing a file takes effect when the services that list it are recreated
(`compose.sh up -d <service>`).

**Encrypted copy.** `secrets.sh copy` (daily timer) encrypts the whole directory to
`backup-recipient.asc` and uploads it to `secrets/` in the backup bucket, which keeps earlier
versions. The private key never touches the host; without it and the pgBackRest passphrase
inside the copy, no backup can be restored. Restore the directory from the copy instead of
running `init` again on a host that had data.

## PostgreSQL

The database is `aura`. [`pg_hba.conf`](postgres/pg_hba.conf) lets the superuser in only as
the container's own `postgres` user (`compose.sh exec -u postgres postgres psql -d aura`);
every other role uses its password. On first initialization the image creates
`aura_admin`, a non-superuser with `CREATEROLE` that owns the database, as the RDS master user
was. `bootstrap` connects as it and creates the roles:

- `aura_runtime`: `api`, `mcp`, `worker` and the `ops` jobs; applications never connect as
  superuser.
- `aura_migrator`: the `migrate` service only, which reads the reviewed plan from stdin:
  `compose.sh run --rm -T migrate migrate --plan /dev/stdin < plan.json`.
- `aura_inspector`: read-only, for `ops inspect-schema`.
- `aura_crawler_ingestion`: the crawler, which may only call the ingestion enqueue function.

The settings and connection budget are in [`postgresql.conf`](postgres/postgresql.conf); keep
`max_connections` above the sum of the pools.

### Crawler

The crawler runs on the host, outside Compose. PostgreSQL also listens on a socket in
`/run/aura-historia/postgresql`, which only the database container can write and only the
`crawler` group can enter. Peer authentication maps uid 999, the image's `postgres` user, to the
superuser on this socket too, so no host process may run as uid 999. Set in
`/etc/crawler/crawler.env`, with the password from `postgres-bootstrap.env`:

```
PRODUCT_LISTING_INGESTION_DATABASE_URL=postgres://aura_crawler_ingestion:<password>@localhost/aura?host=/run/aura-historia/postgresql
```

## Caddy

CloudFront forwards the viewer `Host`, so Caddy serves `api.` and `mcp.` under their public
names and holds their certificates. Those names resolve to CloudFront, so certificates come
through the Cloudflare DNS challenge; port 80 stays closed. A request reaches an upstream only
with the current origin secret in `X-Origin-Verify` (or, while rotating, `ORIGIN_VERIFY_PREVIOUS`);
everything else, including any request while the secret is unset, gets 403. The header is
removed before the request is forwarded. The API is balanced over both replicas, and a replica
that fails `/v1/health` is taken out until it passes again.

Until the cutover (#3037), Caddy also serves `opensearch.` with the read paths the AWS runtime
uses. Remove that site block afterwards.

## First start

On the host, from the stable checkout of a reviewed ref, with the OpenSearch project running:

1. `sudo host/install.sh prod`: socket directory, external volumes, units.
2. `sudo host/prod/secrets.sh init`, fill in the files, `sudo host/prod/secrets.sh check`.
3. `sudo host/prod/compose.sh build postgres caddy`.
4. `sudo host/prod/compose.sh up -d --wait postgres`, then `sudo host/prod/backup.sh stanza`:
   WAL archiving fails until the stanza exists.
5. `sudo host/prod/compose.sh run --rm bootstrap`: creates the roles and applies every
   migration.
6. `sudo host/prod/compose.sh up -d`, then `sudo host/prod/backup.sh full`.
7. `sudo host/prod/backup.sh restore-check` once.
8. Install the operators' OpenPGP public key as `backup-recipient.asc`, then
   `sudo host/install.sh prod-timers`.

The first time Caddy starts, the old OpenSearch gateway's certificates can be carried over so
the `opensearch.` site has no gap: stop the gateway, copy the volume
`aura-historia-opensearch-prod-gateway` into `aura-historia-prod-caddy-data`, start `caddy`.

## Scheduled jobs

systemd timers ([units](systemd)) run on the AWS schedules, in UTC; `install.sh prod` installs
them with the checkout's path and `install.sh prod-timers` enables them. A job never starts the stack (`run --no-deps`), and a run still
going when the next is due is not started twice.

| Timer | Runs |
| --- | --- |
| `aura-prod-backend-cleanup` | `ops backend-cleanup`, hourly |
| `aura-prod-fx-refresh` | `ops fx-refresh`, 06:00 and 18:00 |
| `aura-prod-periodic-match` | `ops periodic-match`, 15:00 |
| `aura-prod-backup-full` | full backup, Sundays 02:30 |
| `aura-prod-backup-diff` | differential backup, Monday to Saturday 02:30 |
| `aura-prod-secrets-copy` | encrypted copy of `/etc/aura-historia/prod`, 04:00 |

`systemctl list-timers 'aura-prod-*'` shows the next and last runs; failed runs show in
`systemctl --failed 'aura-prod-*'` and `journalctl -u 'aura-prod-*'`.

## Backups

pgBackRest ([config](postgres/pgbackrest.conf)) archives WAL continuously and keeps its
repository under `pgbackrest/` in the backup bucket, encrypted with the passphrase in
`pgbackrest.env` before upload. Weekly full and daily differential backups keep every point of
the last 14 days restorable; pgBackRest expires older backups and WAL itself. If S3 is
unreachable, WAL queues up to 16 GiB and is then dropped rather than filling the disk, which
breaks point-in-time recovery until the next backup. `sudo host/prod/backup.sh info` shows
the backups and the WAL range. OpenSearch is not backed up; `ops reproject-all` rebuilds it.

### Restore

`sudo host/prod/backup.sh restore-check` restores the newest backup and the archived WAL after
it into a throwaway container, never into the running database: it replays to the end of
the archive without archiving, prints the migration and table counts, stops the copy and
verifies its data checksums. To restore the database itself, stop the stack, move the data
volume aside, and run pgBackRest's `restore` into an empty `postgres-data` volume with the
same image (`compose.sh run --rm --no-deps -u postgres --entrypoint pgbackrest postgres --stanza=aura restore`
as user `postgres`), optionally with `--type=time --target=<timestamp>`.
