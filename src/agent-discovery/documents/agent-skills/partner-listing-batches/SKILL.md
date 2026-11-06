---
name: partner-listing-batches
description: Create, update, withdraw and synchronize a partner's own listings on Aura Historia in batches through the REST API, synchronously or through asynchronous submission with safe retries. Use when acting for a dealer or auction house that has a partnership with Aura Historia for its listing source.
---

# Submit partner listing batches

Partners whose listing source has an active partnership with Aura Historia maintain its
listings through REST. MCP has no partner tools. The OpenAPI document
`{{documentation_origin}}/swagger.yaml` holds every field and status.

## Credentials

Use a personal access token with the `product-listings:write` scope, sent as
`Authorization: Bearer <token>` to `{{api_origin}}`. The partner creates it while signed in, in
the access-token section of their Aura Historia dashboard; keep it in server-side secret
storage. Reading the source's listings needs `product-listings:read`. See
`{{auth_md_url}}`.

## Synchronous batches

`POST`, `PATCH`, `PUT` and `DELETE` on
`/v1/listing-sources/{listingSourceId}/product-listings` create, update, upsert and withdraw up
to 100 listings, keyed by your own `sourceListingId`. Keep that key stable for each object.

- The whole batch is validated before anything is written: one malformed item means no item is
  written.
- Items then run one after another, each on its own. A failed item does not undo earlier ones.
- `200 []` means every item succeeded. Partial success returns `200` with the failed
  `sourceListingId`s and their error codes. When every item fails, the first item's error is the
  response.
- After a timeout, read the current state with `GET` on the same path before sending anything
  again; there is no idempotency key.

## Asynchronous submission

The same verbs on `/v1/listing-sources/{listingSourceId}/product-listings/async` queue up to 100
items and return as soon as the queue has accepted them.

- `202` means queued, not applied: listings appear later, and business conflicts such as an
  existing `sourceListingId` are decided then and are not reported back. Check the outcome with
  `GET` on the synchronous path.
- The report accounts for every item by its zero-based index as accepted or rejected, with a
  reason and whether a retry can help.
- Send an `Idempotency-Key` of up to 128 visible ASCII characters without commas. To retry an
  uncertain or partially accepted submission, resend the identical batch, in the same order and
  with the same key, including items that were rejected. Never change, reorder or drop items
  under a key; send corrected items in a new request with a new key.

## Limits

Request bodies are limited to 1 MiB. Withdrawn listings keep their history and can be restored.
Aura Historia does not process orders or payments for listed objects.
