# Storage Contracts

## FX snapshots

PostgreSQL is authoritative for canonical FX data.

- `fx_rates` stores immutable snapshot identity, database-assigned monotonic `generation`, capture time, provider source, and idempotent provider event ID.
- `fx_rate_quotes` stores one positive scaled `units_per_eur` quote for every supported currency, including `EUR = 1_000_000`.
- Quote rows are not arbitrary pairwise rates. They are inserted with their parent in one PostgreSQL transaction.
- OpenSearch does not store authoritative FX data. Its Product and saved-filter documents are rebuildable projections only.
- Product reads, writes, search, and projections must use persisted snapshots. They must not call the FX provider.
- Product source pricing never stores an FX ID. A `SOLD` Product stores one immutable `sale_fx_rate_id` and `sold_at`; both are null or both present. The product write transaction reads the latest persisted snapshot and rejects sale creation or transition when no valid snapshot exists.
- Product detail, watchlist, and saved-match readers return source pricing plus the optional sale valuation. Their use cases choose exactly one persisted snapshot per product: the latest snapshot for a current valuation, or the stored sale snapshot for a sale valuation. They use scaled-integer HalfUp conversion to construct display pricing; missing, invalid, or mismatched snapshots fail explicitly.
- Product OpenSearch documents store only optional native `sourcePrice { amount, currency }`. Sold documents additionally store all supported-currency HalfUp `salePrices`, `saleFxRateId`, and `soldAt`; these three fields are all present or all absent. Estimates never enter Product OpenSearch.
- Product search pins one latest snapshot for its request. A requested display range compiles to exact native `sourcePrice` intervals for active products and to the original target-currency range on immutable sold `salePrices`. Price sorting is not supported.
- Saved-filter documents store `compiledFxRateId`. Their percolator query is compiled from the same pinned plan as ordinary search; a later FX generation rebuild is a projection concern.

`fx_rates.source_event_id` is the scheduled-capture idempotency key. `generation` orders persisted snapshots; `captured_at` supports historical selection.
