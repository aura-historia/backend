# Documentation map

Read only the contract for the boundary you are changing. [`arch.md`](arch.md) is the canonical workspace architecture guide; specialized documents own their specific details. [`AGENTS.md`](../AGENTS.md) contains repository workflow rules.

| Change | Start here |
| --- | --- |
| Public API | [OpenAPI](swagger.yaml), [changelog](CHANGELOG.md); [front door](http-api-front-door.md) for deployment/auth routing |
| Domain and identity | [ProductListing](product-listing.md), [object IDs](object-ids.md) |
| PostgreSQL state | [Storage](storage.md) |
| CDC and workers | [Event flow](events/flow.md) for routes/contracts; [worker runbook](durable-worker-runbook.md) for activation, custody and recovery |
| Crawler | [Crawler guide](crawler/README.md) |
| OpenSearch stage | [Stage runbook](opensearch-stage.md) |
| AWS deployment | [Infrastructure guide](../infra/README.md) |

Documentation and CDK declarations are not proof of deployed state. Follow the owning runbook and record approved environment evidence before activation.
