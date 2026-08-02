---
name: aura-rust-reader
description: Use when adding or changing Aura Historia Rust readers, read models, query use cases, joined presentation reads, hydration, user-state enrichment, search reads, or read-side adapter mappings.
---

# Aura Rust Reader

Use for purpose-specific reads and read models.

## Must read

- `backend/AGENTS.md` and path `AGENTS.md` files.
- `docs/arch.md` §3, §6.2, §7, §9, §10.2, §11.7-11.8, §13-15, §20.4-20.6, §21-23.

## Before coding

- Identify final read use case and result shape.
- Decide which partial read models each port owns.
- Decide required vs optional enrichment and failure behavior.
- Decide if any read is invariant-critical and must share a transaction.

## Hard rules

- One file per reader, grouped in mod `readers` of the specific entity.
- Readers provide purpose-specific read capabilities.
- Reader ports belong to service crates and are named by application capability.
- Reader returns application-owned read models.
- Reader MUST NOT return aggregates for display, SQL rows, search documents, key-value items, or external-client response types.
- Relational joins for presentation belong in readers, not repositories.
- Handler composes readers and owns final result/view.
- Controller MUST NOT compose data sources.
- User-specific hydration MUST be batched.
- Never call a reader once per search hit.
- Preserve search/source ordering after hydration.
- Missing user-state rows should map to explicit defaults.
- If user state affects filtering or ranking, include it in query strategy, not post-filtering one returned page.
- Required vs optional enrichment must be explicit in result model.
- Do not represent source failure as absence unless product semantics explicitly allow it.
- Readers should receive narrow application data. They MUST NOT receive `OperationContext` or transport DTOs.
- Use a transaction-bound reader factory when the read influences invariant-critical writes.
- Ordinary presentation readers may own a pool/client when no application transaction is needed.

## Avoid

- Repository used for presentation reads.
- N+1 hydration.
- Adapter document/row/item leaking through service or API.
- Controller-side merging of search, database, and user-state data.
- Technology-named ports like `PostgresReader` or `OpenSearchPort`.

## Tests

- Service tests cover batching, optional enrichment, fallback behavior, ordering, and error translation.
- Adapter tests cover joined readers, mapping to application types, timeout/error mapping, and stale-version behavior where relevant.
