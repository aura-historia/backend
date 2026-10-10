# Documentation map

Read the document that owns the boundary you are changing. These guides explain durable
choices, safety rules and operator procedures—not every type, setting or implementation.
[`AGENTS.md`](../AGENTS.md) owns repository workflow and validation commands.

| Need | Owner |
| --- | --- |
| Architecture and dependency rules | [Architecture](arch.md) |
| Persistence integrity, concurrency and authority | [Storage](storage.md) |
| Listing semantics and ingestion guarantees | [ProductListing](product-listing.md) |
| Durable ID format and prefixes | [Object IDs](object-ids.md) |
| Geographic assertions, reference releases and comparison | [Geography](geography.md) |
| Email consent, provider eligibility and evidence | [Marketing consent](marketing-consent.md) |
| Event sources, routes and completion semantics | [Event flow](events/flow.md) |
| Worker activation, failure custody and replay | [Worker runbook](durable-worker-runbook.md) |
| AWS releases, migration, initialization and rollback | [Infrastructure](infra.md) |
| HTTP routing, authentication and cache isolation | [API front door](http-api-front-door.md) |
| External search host setup, security and recovery | [OpenSearch stage](opensearch-stage.md) |
| Public API requests/responses and change history | [OpenAPI](swagger.yaml), [changelog](CHANGELOG.md) |
| Crawler | [Crawler guide](crawler/README.md) |

## What belongs here

- Keep a rule in one owning document; link to it from adjacent guides.
- Keep schemas, payload fields, algorithms, exact resource settings and test cases in
  code, migrations, configuration and tests. Explain only the guarantees and failure
  behavior a reader needs to design or operate safely. Explicit architectural rules,
  rationale and illustrative code patterns belong in the architecture guide; do not
  remove useful explanations merely to shorten it.
- Put public API changes in OpenAPI/the changelog, not release diaries in design guides.
- Keep inspection results, rollout checklists and acceptance evidence with the release
  or issue. Documentation and synth are not proof of current deployed state.
- Amend an existing owner when possible; add a document only for a distinct boundary
  and give it an entry in this map. Do not grow general guides into implementation inventories.

Local CDK commands live in [`infra/README.md`](../infra/README.md); deployment operations
live here. Swagger hosting files and crawler documentation have separate ownership.
