---
name: aura-rust-review-architecture
description: Use before completing meaningful Aura Historia backend code changes or when reviewing Rust architecture, dependency boundaries, visibility, forbidden patterns, tests, logging, auth, or persistence design.
---

# Aura Rust Review Architecture

Use as final architecture gate for meaningful backend changes.

## Must read

- `backend/AGENTS.md` and path `AGENTS.md` files.
- Relevant `docs/arch.md` sections for touched code.
- Always scan `docs/arch.md` §21-24.

## Review questions

- Which layer owns each new type?
- Is each public type intentionally public for a real production crate boundary?
- Does each use case express business intent?
- Does each handler depend only on capabilities it uses?
- Are domain fields private and invariants enforced by behavior?
- Is aggregate persistence separated from read-model construction?
- Are storage rows/documents/items confined to adapters?
- Does transaction scope contain exactly invariant-critical PostgreSQL work?
- Are cross-source reads composed in a handler, not a controller?
- Is trusted identity carried through `OperationContext`, not request input?
- Do protected mutations reject anonymous/unpermitted principals?
- Do relevant mutations record actor, target, and committed outcome safely?
- Are errors translated at layer boundaries with source causes preserved?
- Are logs safe and free of secrets/sensitive payloads?
- Are private/real-infrastructure tests beside code and `/tests` black-box only?
- Are important rules covered by tests?

## Forbidden pattern scan

Reject or fix unless an approved architecture change exists:

- Generic cross-store repository.
- Repository used for presentation reads.
- Storage row/document/item escaping adapter.
- Controller orchestration of data sources.
- N+1 hydration.
- Domain depending on infrastructure.
- God service for unrelated use cases.
- Hidden distributed transaction.
- Sensitive payload logging.
- Silent persisted-state corruption through unchecked mapping.

## Dependency scan

Allowed direction:

```text
core <- service <- adapters <- runtime/transport
```

Forbidden:

```text
core -> service/adapters
service -> adapters/REST DTOs
controller -> database/repository/storage type
adapter A -> private types from adapter B
```

## Before final answer

- Run or recommend the narrowest useful validation.
- Do not claim validation passed unless it ran and passed.
- Mention any skipped validation and why.
- Mention architecture deviations explicitly.
