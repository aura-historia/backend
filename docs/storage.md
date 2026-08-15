# Storage Contracts

## FX snapshots

PostgreSQL is authoritative for canonical FX data.

- `fx_rates` stores immutable snapshot identity, database-assigned monotonic `generation`, capture time, provider source, and idempotent provider event ID.
- `fx_rate_quotes` stores one positive scaled `units_per_eur` quote for every supported currency, including `EUR = 1_000_000`.
- Quote rows are not arbitrary pairwise rates. They are inserted with their parent in one PostgreSQL transaction.
- OpenSearch does not store authoritative FX data and is rebuilt separately when later product projection work needs it.
- Product reads, writes, search, and projections must use persisted snapshots. They must not call the FX provider.
- Product source pricing never stores an FX ID. A `SOLD` Product stores one immutable `sale_fx_rate_id` and `sold_at`; both are null or both present. The product write transaction reads the latest persisted snapshot and rejects sale creation or transition when no valid snapshot exists.

`fx_rates.source_event_id` is the scheduled-capture idempotency key. `generation` orders persisted snapshots; `captured_at` supports historical selection.
