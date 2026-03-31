# Event Flow

All events originate from DynamoDB Stream changes on `table_1`. An EventBridge Pipe
(`TableOneStreamToEventBusPipe`) filters relevant stream records and forwards them to the
`DynamoDbEventBus`. EventBridge rules on that bus fan out to dedicated SQS queues, each
consumed by a Lambda function.

---

## Components

| Component | Type | Purpose |
|-----------|------|---------|
| `table_1` | DynamoDB Table | Single event-store and read-model table |
| `DynamoDbEventBus` | EventBridge Bus | Central routing bus for all DynamoDB stream events |
| `TableOneStreamToEventBusPipe` | EventBridge Pipe | Filters DynamoDB stream records and publishes to the event bus |
| `ProductMaterializeDynamoDbQ` | SQS + Lambda | Writes materialized product view to DynamoDB |
| `ProductMaterializeOpenSearchQ` | SQS + Lambda | Indexes products in OpenSearch |
| `ProductUpdateNotifyUserQ` | SQS + Lambda | Notifies watchlist users on price/state changes |
| `SearchFilterPercolateProductQ` | SQS + Lambda | Matches products against saved search filters, notifies users |
| `ProductPipelineTranslateQ` | SQS + EC2 ASG | Translates product titles and descriptions (ML, GPU) |
| `ProductPipelineEmbedTextQ` | SQS + Lambda | Creates vector embeddings via Gemini Embedding API |
| `ProductPipelineExtractAttributeQ` | SQS + EC2 ASG | Extracts product attributes (year, condition, …) (ML, GPU) |
| `ProductPipelineClassifyQ` | SQS + Lambda | Classifies products into categories and periods via Gemini |
| `ShopOpenSearchIndexQ` | SQS + Lambda | Indexes shop records in OpenSearch |
| `SearchFilterOpenSearchSyncQ` | SQS + Lambda | Syncs search filters to OpenSearch percolation queries |
| `NotificationSendQ` | SQS + Lambda | Sends external notifications via SES |
| `ProductPipelineScaleControlLambda` | Lambda (scheduled) | Scales EC2 ASGs based on SQS queue depth (every 15 min) |
| `FxRateSyncLambda` | Lambda (scheduled) | Updates foreign exchange rates (every 12 h) |

---

## Event Routing Diagram

```mermaid
flowchart TD
    API["Partner API\n(POST/PATCH/PUT /api/v1/shops/{shopId}/products)"]
    DB[("DynamoDB\ntable_1")]
    PIPE["EventBridge Pipe\nTableOneStreamToEventBusPipe"]
    BUS["EventBridge Bus\nDynamoDbEventBus"]

    API -->|"write event record"| DB
    DB -->|"DynamoDB Stream\n(NEW_IMAGE)"| PIPE
    PIPE -->|"filtered INSERT/MODIFY/REMOVE"| BUS

    %% Materialization
    BUS -->|"DOMAIN_* / ENRICHMENT_* / POLICY_* (INSERT)"| MatDDB["ProductMaterializeDynamoDbQ\n→ Lambda\n(write materialized view)"]
    BUS -->|"DOMAIN_* / ENRICHMENT_* / POLICY_* (INSERT)"| MatOS["ProductMaterializeOpenSearchQ\n→ Lambda\n(index in OpenSearch)"]

    %% User notifications (only price & state changes)
    BUS -->|"DOMAIN_PRICE_* / DOMAIN_STATE_* (INSERT)"| NotifyUser["ProductUpdateNotifyUserQ\n→ Lambda\n(notify watchlist users)"]
    BUS -->|"DOMAIN_* / ENRICHMENT_* (INSERT)"| Percolate["SearchFilterPercolateProductQ\n→ Lambda\n(match saved search filters)"]

    %% Enrichment pipeline
    BUS -->|"DOMAIN_CREATED (INSERT)"| Translate["ProductPipelineTranslateQ\n→ EC2 ASG (GPU)\n(translate title & description)"]
    BUS -->|"DOMAIN_CREATED (INSERT)"| EmbedText["ProductPipelineEmbedTextQ\n→ Lambda\n(vector embedding via Gemini)"]
    BUS -->|"ENRICHMENT_EMBEDDED (INSERT)"| Extract["ProductPipelineExtractAttributeQ\n→ EC2 ASG (GPU)\n(extract attributes)"]
    BUS -->|"ENRICHMENT_EMBEDDED (INSERT)"| Classify["ProductPipelineClassifyQ\n→ Lambda\n(classify category & period via Gemini)"]

    %% Enrichment pipeline writes back to DynamoDB
    Translate -->|"write ENRICHMENT_TRANSLATED_TITLE\n/ ENRICHMENT_TRANSLATED_DESCRIPTION"| DB
    EmbedText -->|"write ENRICHMENT_EMBEDDED"| DB
    Extract -->|"write ENRICHMENT_EXTRACTED_ATTRIBUTES"| DB
    Classify -->|"write ENRICHMENT_CLASSIFY_CATEGORY\n/ ENRICHMENT_CLASSIFY_PERIOD"| DB

    %% Shop & search filter sync
    BUS -->|"shop#details (INSERT/MODIFY)"| ShopOS["ShopOpenSearchIndexQ\n→ Lambda\n(index shop in OpenSearch)"]
    BUS -->|"search_filter#* (INSERT/MODIFY/REMOVE)"| SFSync["SearchFilterOpenSearchSyncQ\n→ Lambda\n(sync percolation query)"]

    %% External notification send
    BUS -->|"user#notification#* + external=true (INSERT)"| SendNotif["NotificationSendQ\n→ Lambda\n(send email via SES)"]

    %% Scheduled
    SCHED1["Scheduler (every 15 min)"] --> ScaleCtrl["ProductPipelineScaleControlLambda\n(scale Translate & ExtractAttribute ASGs)"]
    SCHED2["Scheduler (every 12 h)"] --> FxRate["FxRateSyncLambda\n(update FX rates in DynamoDB)"]
```

---

## Stream Filter Details

The EventBridge Pipe applies the following DynamoDB Filter Criteria before publishing to the bus:

| Filter | `pk` pattern | `sk` pattern | Operation |
|--------|-------------|-------------|-----------|
| Product events | `product#shop_id#*` | `product#event#*` | INSERT |
| User details | `user#*` | `user#details` | MODIFY |
| Shop details | `shop#shop_id#*` | `shop#details` | INSERT, MODIFY |
| User notifications | `user#*` | `user#notification#origin_event_id#*` | INSERT |
| Search filters | `user#*` | `search_filter#*` | INSERT, MODIFY, REMOVE |

---

## Dead Letter Queues

Every SQS queue has a corresponding DLQ. Retry limits:

| Queue | Max Retries |
|-------|-------------|
| Materialization queues | 5 |
| Notification queues | 5 |
| Pipeline queues | 3 |
