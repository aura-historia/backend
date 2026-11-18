# Host process startup

`platform-process` starts and stops the long-running processes (`api`, `mcp`, `worker`) and the
one-shot `ops` commands on the host. It owns their configuration, PostgreSQL pool, termination
signal, bounded panic output, failure diagnostics and Google credentials.

## Configuration

Every setting is an environment variable. A secret may instead come from a file readable only by
the process user (uid 10001 in the image): when `NAME_FILE` is set, startup reads that file into
`NAME` once, before any thread starts, and removes a single trailing line break. Other tools'
own `_FILE` variables (`AWS_*`, `SSL_*`, `GOOGLE_*`, `LLVM_*` and similar) keep their meaning. Setting both `NAME` and `NAME_FILE`, an unreadable file,
an empty file or a non-UTF-8 file stops startup. Values never appear in logs or errors; errors
name only the setting.

## PostgreSQL

| Variable | Meaning |
| --- | --- |
| `POSTGRES_HOST` | Host name on the internal network, or the directory of the Unix socket when it starts with `/` |
| `POSTGRES_PORT` | Default `5432` |
| `POSTGRES_DATABASE` | Database name |
| `POSTGRES_USERNAME`, `POSTGRES_PASSWORD` | The process role; usually `POSTGRES_PASSWORD_FILE` |
| `POSTGRES_MAX_CONNECTIONS` | The process's fixed pool size; each process type has a default |

PostgreSQL is reachable only on internal networks, so connections use no TLS. Pools keep the
statement (5 s), lock (1 s) and idle-in-transaction (10 s) bounds the Lambda runtime had, wait up
to 5 s for a pooled connection and connect lazily. Each process has a fixed pool, so the sum over
all processes must stay below the server's `max_connections` with headroom for operators,
migrations and the crawler. The defaults are: `api` and `mcp` 10 each, `worker` one connection per
concurrent job plus four (see the [worker](../aura-historia-worker/README.md#postgresql-pool)), and
`ops` 2 (periodic matching 1).

## Shutdown and panics

Processes register SIGTERM and SIGINT before slow startup work. The container stop timeout
must exceed the longest drain: 35 seconds or more covers the API's 25-second request budget and
the worker's default 25-second grace. The API and MCP stop accepting
connections and drain in-flight requests; the worker stops claiming and drains or releases its
jobs. The panic hook prints only the component and location, never the panic payload, which can
carry provider responses or persisted values.

## Google credentials

`google_application_default_credentials()` reads the credentials file named by
`GOOGLE_APPLICATION_CREDENTIALS` and fails at startup when it is missing; it never falls back to
a developer's local credentials or a metadata server.
