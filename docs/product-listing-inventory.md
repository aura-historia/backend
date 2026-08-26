# ProductListing rewrite inventory

## Status

The ProductListing bounded-context rewrite is complete.

## Classification rules

| Class | Action |
| --- | --- |
| Aura-owned listing contract | Use `ProductListing` vocabulary only. |
| Provider-native contract | May retain provider wording at its boundary. |
| Human-facing copy | May say “product” when natural. |
| Dated history | Keep only in historical changelog entries. |

## Final scan checklist

- [x] Crate family, workspace dependencies, and composition use `product-listing-*`.
- [x] Domain types use `ProductListing`, optional `ListingAvailability`, `ListingLifecycle`, and `ListingSaleObservation`.
- [x] Initial business and crawler schemas use listing table, column, and event names.
- [x] API routes, JSON IDs, error codes, and OpenSearch documents use listing vocabulary.
- [x] Crawler and provider boundaries keep uncertainty local and map explicit availability/presence observations.
- [x] Search, watchlist, notification, translation, worker, and templates use listing vocabulary.
- [x] Public API docs describe explicit nullable availability and availability/orderability filtering.

## Residual-term policy

Remaining `product` text must be classified as provider-native data, source-page fixture content, natural-language copy, unrelated intrinsic-product scope, or dated changelog history. It must not define an active Aura-owned listing contract.
