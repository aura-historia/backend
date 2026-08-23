# Storage Contracts

## Notifications

PostgreSQL is the sole production owner of notifications and external-delivery intent. DynamoDB stores no notification rows, and notification storage has no TTL.

- `notifications` stores one immutable typed content snapshot per semantic reason. `origin_event_id` is provenance only and is absent for partner-application notifications.
- Watchlist idempotency is `(user_id, origin_event_id, kind)`. Search-filter idempotency is `(user_id, user_search_filter_id, product_id, origin_event_id)`. Partner-application idempotency is `(user_id, partner_shop_application_id)`.
- A single Product event may create a watchlist notification and one distinct notification for every matching search filter.
- `notification_deliveries` owns durable external-delivery state, separate from a Notification. One Notification may have several delivery rows. Each row is unique per `(notification_id, channel, target_key)` and contains no copied notification payload or target value. The application planner selects channels and inserts requested rows in the same PostgreSQL transaction as newly inserted notifications; each channel adapter resolves its own target. EMAIL/PRIMARY is the sole production plan.
- The generic delivery worker consumes committed `notification_deliveries` INSERT rows through one Sequin subscription, claims a lease, dispatches by channel, and finalizes that same lease. EMAIL is the only valid stored channel now; a future channel needs a core enum and schema migration plus its sender. EMAIL resolves its current target and performs S3/SES I/O after the claim transaction commits. A send/finalize crash can duplicate an external delivery, so delivery is at-least-once, not exactly-once. The bounded in-memory worker queue remains non-durable by design; its post-ack loss window is tracked by #1558.
- Read models and REST mutations use `notification_id`; they never expose `origin_event_id`.
- Corrupt persisted payload/version/source-shape state is an operation error. It is never silently treated as a missing notification.

## Watchlist concurrency and intervals

- `product_watchlist.version` is internal optimistic-concurrency metadata. Ordinary aggregate updates compare the loaded version and increment it once; a mismatch is a conflict, never not-found or a silent retry. REST never exposes it.
- Tier reconciliation locks the user row first, then affected watchlist rows. It increments `version` for every changed row, so concurrent stale user writes fail.
- `product_watchlist.active_since` is non-null only for `ACTIVE` rows and marks the beginning of the current active interval.
- `product_watchlist.notifications_enabled_since` is non-null exactly when `notifications = true` and marks the beginning of the current email-enabled interval.
- Watchlist notification readers compare both interval starts with immutable `product_events.event_time`; deactivation/reactivation and email disable/re-enable start new intervals. These fields are repository-owned persistence metadata, not REST payload fields.

## FX snapshots

PostgreSQL is authoritative for canonical FX data.

- `fx_rates` stores immutable snapshot identity, database-assigned monotonic `generation`, capture time, provider source, and idempotent provider event ID.
- `fx_rate_quotes` stores one positive scaled `units_per_eur` quote for every supported currency, including `EUR = 1_000_000`.
- Quote rows are not arbitrary pairwise rates. They are inserted with their parent in one PostgreSQL transaction.
- OpenSearch does not store authoritative FX data. Its Product and saved-filter documents are rebuildable projections only.
- Product reads, writes, search, and projections must use persisted snapshots. They must not call the FX provider.
- Product source pricing never stores an FX ID. A `SOLD` Product stores one immutable `sale_fx_rate_id` and `sold_at`; both are null or both present. The product write transaction captures `sold_at` before lookup and selects latest `captured_at <= sold_at`; it rejects sale creation or transition when no valid snapshot exists.
- Product detail, watchlist, and saved-match readers return source pricing plus the optional sale valuation. Their use cases capture one request valuation instant and choose exactly one persisted snapshot per product: latest `captured_at <= valuation_at` for a current valuation, or the stored sale snapshot for a sale valuation. They use scaled-integer HalfUp conversion to construct display pricing; missing, invalid, or mismatched snapshots fail explicitly.
- Product OpenSearch documents store only optional native `sourcePrice { amount, currency }`. Their sale shape is exactly one of: no `saleFxRateId`, `soldAt`, or `salePrices`; complete `saleFxRateId` plus `soldAt` with no `salePrices` when no main source price exists; or complete metadata plus every supported-currency HalfUp `salePrices` when a main source price exists. Partial metadata or partial `salePrices` is invalid. Estimates never enter Product OpenSearch.
- Product search first pages capture `valuation_at` and pin latest persisted `captured_at <= valuation_at`; each continuation loads the cursor's exact `fx_rate_id`, never a newer snapshot. A requested display range compiles to exact native `sourcePrice` intervals for active products and to the original target-currency range on immutable sold `salePrices`; a sold Product without `salePrices` never matches a price range. Price sorting is not supported.
- `products.projection_version` is the positive monotonic source version for rebuildable Product OpenSearch writes. It advances for Product aggregate, embedding, and translation changes; the projection uses OpenSearch external versioning.
- Saved-filter documents retain the requested price range and compile it directly against private temporary `priceByCurrency.<currency>` fields. They store no FX ID or generation. For an accepted current Product event, percolation selects the immutable sale snapshot when present; otherwise it selects latest `captured_at <= product_events.event_time` ordered by `captured_at DESC, generation DESC`. It converts the native main price into every supported currency with checked HalfUp arithmetic. FX capture alone does not rewrite saved filters, reproject Products, create matches, or send notifications.
- `search_filter_matches.price_valuation_basis` and `price_fx_rate_id` are null together for non-price matches. Price matches persist `CURRENT` with the periodic run snapshot, `EVENT` with the event-effective snapshot, or `SALE` with the immutable sale snapshot. The FX ID references immutable `fx_rates`.
- `search_filter_periodic_match_state` holds durable per-filter periodic matching progress. Missing rows start at the authoritative Search Filter creation timestamp; this operational state is not part of ordinary Search Filter read views.

`fx_rates.source_event_id` is the scheduled or deployment-bootstrap capture idempotency key. Canonical captures serialize and require `captured_at` to be strictly newer than the prior capture, except duplicate source IDs; historical import needs a separate workflow. `generation` orders persisted snapshots; `captured_at` supports historical selection.
