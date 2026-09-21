# Migration F7 — DMS to Kinesis CDC contract

**Baseline SHA:** `6068a768b2856e03a212c1f25d335f510f6bbbfa`.

This is the #1781 DMS CDC configuration and operator contract. It is not live-AWS evidence. A checked-in configuration, `cdk synth`, or a passing local test proves only source configuration. Call an AWS test successful only after an approved operator executes it and records its safe result.

## Scope and target

- Real stages: private, single-AZ RDS PostgreSQL `16.13`; DMS `3.6.1` on single-AZ `dms.t3.small` (2 vCPU, 2 GiB); one provisioned Kinesis shard with seven-day retention.
- DMS is CDC-only. It does not full-load, backfill, create an outbox, dual-write, or redesign Sequin/custom CDC targets.
- `ephemeral` does not declare this path. `dev` and `prod` declarations, if synthesized, are still not proof that AWS accepted or deployed them.
- #1787 owns Kinesis router/Lambda consumption. #1788 owns production handoff. Neither is complete through this document.

Check regional engine/class availability before a change set or deploy:

```bash
aws dms describe-orderable-replication-instances --engine-name postgres --engine-version 3.6.1 --replication-instance-class dms.t3.small --region eu-central-1
```

The command must return an orderable matching item in the approved account/region. Its presence here is not execution evidence.

## DMS, network, and credentials

The target real-stage `DmsCdc` declaration owns one replication subnet group in private application subnets, the replication instance, a CDC-only PostgreSQL source endpoint, a Kinesis target endpoint, one stopped CDC task, and the two interface endpoints below.

| Boundary | Contract |
| --- | --- |
| RDS source | Private, single-AZ RDS PostgreSQL `16.13`; DMS reaches the database security group on TCP 5432 only. |
| DMS instance | `dms.t3.small`, DMS `3.6.1`, CDC-only. No public route or broad egress. |
| Kinesis interface endpoint | Private DNS enabled for `kinesis.eu-central-1.amazonaws.com`. |
| Secrets Manager interface endpoint | Private DNS enabled for `secretsmanager.eu-central-1.amazonaws.com`. |
| Security groups | DMS security group egress is TCP 5432 to the database security group and TCP 443 to the endpoint security group only. The endpoint security group admits TCP 443 from the DMS security group only. |

The existing NAT is not a DMS egress path. Do not add `0.0.0.0/0`, an internet gateway route, or another broad DMS egress rule. The account must already have the DMS service role `dms-vpc-role` with the AWS-managed `service-role/AmazonDMSVPCManagementRole`; this stack does not create a global account role that would collide between stages.

DMS reads the stage secret only at:

```text
/aura-historia/<stage>/postgres/replication
```

Never print the secret value, connection string, or password in a command, template output, log, issue, or test evidence. `aura_replication` has only table-scoped `SELECT` plus `rds_replication`; it has no broad application SQL, DML, DDL, ownership, or administrator grant.

## Start, slots, checkpoint, and recovery

The task is created stopped. The only control defined now is an approved, explicit initial CDC start after the source, target, slot, and mapping checks pass. It must not start during deployment or synthesis.

Before that start, an approved operator manually checks and, only when absent, provisions the named `test_decoding` logical slot through a TLS-verified PostgreSQL session. The slot name is an operator-approved non-secret value.

```sql
-- Check before creating. Do not recreate an existing slot.
SELECT slot_name, plugin, slot_type, active, restart_lsn, confirmed_flush_lsn
FROM pg_replication_slots
WHERE slot_name = '<approved-slot-name>';

-- Run only if the check returns no row.
SELECT *
FROM pg_create_logical_replication_slot('<approved-slot-name>', 'test_decoding');

-- Confirm the slot before DMS starts.
SELECT slot_name, plugin, slot_type, active, restart_lsn, confirmed_flush_lsn
FROM pg_replication_slots
WHERE slot_name = '<approved-slot-name>';
```

The source endpoint and the manually checked slot must name the same slot. Normal restart resumes only from the DMS checkpoint. It does not set a new start time or recreate a slot. A lost, invalid, or unexpectedly advanced slot requires an explicitly approved new fenced replay/rebuild plan: new generation, consumer fence, source/checkpoint decision, verification, and activation. Never auto-create a replacement slot.

## Source mapping and numeric wire contract

DMS table selection is explicit. The router receives DMS/Kinesis records, not application jobs, and fails closed for an unlisted table or operation. The mapping removes every other current source column; unknown additive columns are tolerated only after ingress-size, table/operation, and required-field checks.

| Source table | Accepted CDC operations | Kinesis `data` fields | Router action |
| --- | --- | --- | --- |
| `product_listing_events` | `INSERT` | `event_id`, `product_listing_id`, `event_type`, `event_group`, `event_type_schema_version`, `payload`, `event_time` | Build the existing ProductListing event route union. |
| `product_listing_raw_revisions` | `INSERT` | `product_listing_raw_stream_id`, `product_listing_raw_revision_id`, `revision` | Wake `product-listing-normalization` only. |
| `search_filters` | `INSERT`, `UPDATE`, `DELETE` | `user_search_filter_id`, `user_id`, `version` | Build search-filter projection work from the authoritative row/delete image. |
| `search_filter_matches` | `INSERT` | `user_id`, `user_search_filter_id`, `product_listing_id`, `origin_event_id` | Build search-filter match notification work. |
| `notification_deliveries` | `INSERT` | `notification_delivery_id` | Build notification-delivery work. |
| `product_listings`, `users`, `product_listing_watchlist`, `partnership_applications`, `auction_events` | none | none | Exclude from this DMS task. |

DMS `UPDATE` is the target equivalent of the current Sequin `MODIFY` label. A malformed required identifier, operation, delete image, or type remains failed; it is never silently accepted.

Do not parse PostgreSQL `bigint` revisions through an IEEE-754 number. Preserve these values as canonical base-10 decimal strings, without exponent or fractional form:

| Source field | Target field | Rule |
| --- | --- | --- |
| `product_listing_raw_revisions.revision` | raw-revision job `revision` | Decimal string; never JavaScript `Number`/JSON number. |
| `search_filters.version` | search-filter job `version` | Decimal string; never JavaScript `Number`/JSON number. |

## Record and LOB bounds

- One Kinesis record is at most **1 MiB**. Size the serialized DMS record, not only a source column.
- Full-LOB target mode is disabled. The maximum DMS LOB is **1 MiB** (`1,024 KiB`).
- This task has a stricter **512 KiB** source/task cutoff. A value beyond that cutoff fails on truncation; it is not trimmed, omitted, or replaced.
- DMS and the router must surface oversized or truncated data as failure. They must never silently omit it or acknowledge it as processed.

## Fixture corpus and execution protocol

This corpus is required for an isolated approved stage. It is not a claim that an AWS test has run.

| Fixture | Expected check |
| --- | --- |
| One committed `INSERT` for each selected insert-only table | Kinesis record has the mapped table/operation and router sees the intended route. |
| Committed `search_filters` insert, update, and delete | All three operations map; delete retains required identity/version data. |
| Raw revision and search-filter version greater than `9007199254740991` | Both arrive byte-for-byte as decimal strings. |
| 512 KiB boundary and 512 KiB-plus-one LOB/source value | Boundary policy is explicit; over-cutoff input fails rather than truncating or disappearing. |
| 1 MiB Kinesis-record boundary | Oversize serialization fails visibly and is not acknowledged. |
| Mutation to each excluded table | No DMS record/route is produced. |

For every selected-table operation, use a unique fixture identity twice:

1. In one transaction, write the fixture and `COMMIT`. Capture the source identity, DMS checkpoint/LSN, Kinesis record, and router result without secrets or row payloads.
2. In a second transaction, write the matching rollback fixture and `ROLLBACK`. After the configured observation window, prove there is no source row and no Kinesis/router record for that rollback identity.

Run the checked-in configuration checks before an AWS exercise:

```bash
npm --prefix infra test
npm --prefix infra run synth -- --context stage=dev
npm --prefix infra run synth -- --context stage=prod
```

These are test/synth commands only. The manual slot check, availability command, task start, and committed/rolled-back fixture run are optional operator actions, not an added script or CI requirement. Record account, region, task/stream identifiers, timestamps, checkpoints, and pass/fail outcome; never record secret values or complete source rows.

## Cost delta

Look up prices on the AWS price pages at change-set approval or live-test execution. Do not record fixed dollar amounts here: AWS prices, tenancy, transfer, retention, and tax conditions can change.

| Increment | Required price-page lookup |
| --- | --- |
| One single-AZ `dms.t3.small` | DMS instance-hour rate in `eu-central-1`. |
| One provisioned Kinesis shard | Shard-hour rate and seven-day extended-retention rate in `eu-central-1`. |
| Two interface endpoints | PrivateLink endpoint-hour and data-processing rates in `eu-central-1`. |
| Existing NAT | No increment caused by this DMS design; it remains an existing cost. |

Sources: [DMS pricing](https://aws.amazon.com/dms/pricing/), [Kinesis Data Streams pricing](https://aws.amazon.com/kinesis/data-streams/pricing/), and [AWS PrivateLink pricing](https://aws.amazon.com/privatelink/pricing/). A price lookup is planning input, not billing or live-resource evidence.

## Evidence and handoff

`cdk synth`, configuration review, and the fixture list do not prove AWS availability, CloudFormation deployment, endpoint DNS, security-group reachability, secret access, slot health, DMS checkpointing, or Kinesis delivery. Mark each AWS test only after execution.

#1787 takes the DMS/Kinesis router and Lambda consumption contract. #1788 takes production handoff, controlled cutover, and recorded AWS evidence. This task does not retire Sequin, introduce an outbox or custom CDC transport, or redesign downstream targets.
