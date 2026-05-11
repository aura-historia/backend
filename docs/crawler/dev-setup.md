# Crawler — Local Development Setup

This guide covers everything you need to run the crawler locally: database setup, running the demo
binary, adding migrations, and understanding how production deployments work.

---

## Prerequisites

| Tool | Notes |
|------|-------|
| **Docker Desktop** | Provides `docker compose`. Must be running. |
| **Rust / Cargo** | Standard toolchain — `cargo run` / `cargo build`. |
| **sqlx-cli** | Only needed for `db-status.ps1` (inspect migration state). Not needed for `cargo run --bin demo`. |

Install sqlx-cli if you need it:

```powershell
cargo install sqlx-cli --no-default-features --features rustls,postgres
```

---

## How it all fits together

```
src/crawler/
  docker-compose.yml          # Postgres 16, port 5432, named volume crawler_pgdata
  scripts/
    db-down.ps1               # docker compose down  (volume preserved)
    db-reset.ps1              # docker compose down -v + up -d  (wipes data)
    db-status.ps1             # cargo sqlx migrate info
  migrations/
    20260101000000_initial_schema.sql   # baseline — full schema
    YYYYMMDDHHMMSS_<desc>.sql           # future versioned migrations go here
  sql/
    schema.sql                # REFERENCE ONLY — not executed by anything
```

The container uses a **persistent named volume** (`crawler_pgdata`) so data survives
`db-down.ps1` / Docker Desktop restarts. Only `db-reset.ps1` destroys the volume.

---

## First-time setup for `demo-scraper` / `demo-spider`

These binaries connect via `DATABASE_URL` but do not start Docker or apply migrations
automatically. You need a running, migrated database first.

Run `demo` once — it handles everything:

```powershell
$env:GEMINI_API_KEY = "your-key-here"
$env:GEMINI_FLEX = "true"   # optional
cargo run -p crawler --bin demo
```

Or start the container and apply migrations manually via `docker compose`:

```powershell
cd src\crawler
docker compose up -d
$env:DATABASE_URL = "postgres://postgres:postgres@localhost:5432/postgres"
cargo sqlx migrate run --source migrations
```

Then run the individual binaries:

```powershell
$env:DATABASE_URL = "postgres://postgres:postgres@localhost:5432/postgres"
$env:GEMINI_API_KEY = "your-key-here"
$env:GEMINI_FLEX = "true"   # optional
cargo run -p crawler --bin demo-scraper
cargo run -p crawler --bin demo-spider
```

---

## Running the full demo (zero manual setup)

The `demo` binary is fully self-contained. It:

1. Runs `docker compose up -d` automatically (idempotent — reuses an already-running container).
2. Waits for Postgres to accept connections (retry loop, up to 30 attempts with exponential back-off).
3. Applies any pending migrations via `sqlx::migrate!`.
4. Wires all crawler dependencies and starts the cron loop.

Just set the API key and run:

```powershell
$env:GEMINI_API_KEY = "your-key-here"
$env:GEMINI_FLEX = "true"   # optional
cargo run -p crawler --bin demo
```

### Environment variables for `demo`

| Variable | Default | Required |
|----------|---------|----------|
| `GEMINI_API_KEY` | — | **Yes** |
| `GEMINI_MODEL` | `gemini-3.1-pro-preview` | No |
| `GEMINI_FLEX` | unset / `false` | No |
| `DATABASE_URL` | `postgres://postgres:postgres@localhost:5432/postgres` | No |
| `LOG_LEVEL` | `info` | No |

Output is written to `scraped_products.json` in the working directory (no DynamoDB needed).

---

## Day-to-day workflows

### Stop the database

```powershell
cd src\crawler
.\scripts\db-down.ps1   # stop container (data volume is preserved)
```

### Wipe and start fresh

```powershell
cd src\crawler
.\scripts\db-reset.ps1  # destroys the volume and starts a clean container
```

Migrations are re-applied automatically next time `demo` runs — or manually:

```powershell
cargo sqlx migrate run --source migrations
```

This replaces the old "delete container, rebuild, re-run schema.sql" manual loop.

### Check migration status

```powershell
cd src\crawler
.\scripts\db-status.ps1   # lists applied and pending migrations
```

---

## Adding a new migration

1. Create a file in `src/crawler/migrations/` following the naming convention:

   ```
   YYYYMMDDHHMMSS_short_description.sql
   ```

   Example: `20260201120000_add_shop_currency.sql`

2. Write the migration SQL. Use `IF NOT EXISTS` / `IF EXISTS` guards where appropriate
   so the migration is safe to inspect even if re-applied.

3. Apply locally — just run `demo` (it applies migrations on startup), or manually:

   ```powershell
   cd src\crawler
   cargo sqlx migrate run --source migrations
   ```

4. Deploy. The production `server` binary calls `sqlx::migrate!("./migrations")` immediately
   after connecting to Postgres — migrations apply automatically, no manual step needed.

> **Never modify an already-applied migration file.** sqlx checksums every file and will
> refuse to run if a previously-applied file has changed. Add a new migration instead.

---

## How `demo` locates docker-compose.yml

`start_db()` in `src/demo.rs` uses `env!("CARGO_MANIFEST_DIR")` — a compile-time constant
that always points to `src/crawler/` regardless of the directory you run `cargo run` from.
This means the command works correctly from the workspace root, from `src/crawler/`, or from
anywhere else.

---

## Integration tests

Integration tests under `src/crawler/tests/` use **testcontainers** — they spin up their own
ephemeral Postgres container per test run and do not interact with the docker-compose container
at all. No setup is required to run them:

```powershell
cargo test -p crawler
```

---

## Production

The `server` binary (`src/bin/server.rs`) applies migrations automatically on startup:

```rust
sqlx::migrate!("./migrations").run(&pool).await?;
```

Deploying a new binary that includes additional migration files is the only step needed to
update the production schema. There is no manual SQL execution, no schema.sql to run.
