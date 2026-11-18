-- One-off re-qualification under the e-shop rule of #3008. Before it, the review agents judged a
-- dealer that sells by enquiry, priced or "price on request", to sell nothing online
-- (NOT_AN_ESHOP), and read pages whose streamed or `aside` content their projection hid, so they
-- excluded real shops. This puts back in the qualification queue what the agents decided under
-- the old rule, so they decide it again under the new one (docs/crawler/README.md, ListingSource
-- qualification):
--
-- - an agent's NOT_AN_ESHOP or OTHER label is cleared, with the crawl failure state the labelled
--   shop left, as clearing a label does;
-- - an agent's qualification of such a source, and an agent's NEEDS_HUMAN without a label, is
--   cleared, which keeps the source uncrawled until it is decided again.
--
-- What a human decided and still stands stays: a label a human set is kept, and so is a human's
-- qualification, whose source is crawled again once the scope refresh sees it without the agent's
-- label. A human's label that an agent's decision already replaced in the qualification backfill
-- is not restored; it stays in the label history. Every clear is appended to the source's label or
-- qualification history as SYSTEM, so the decision it replaced stays readable. A SYSTEM clear does
-- not reset the agents' NEEDS_HUMAN retries, which count since a human's last decision: a source
-- that already spent them gets this one decision and then waits for a human.
WITH requalified AS (
    SELECT listing_source_id,
           COALESCE(source_label IN ('NOT_AN_ESHOP', 'OTHER')
               AND source_labelled_by = 'AGENT_AUTHOR', FALSE) AS clears_label,
           COALESCE(qualified_by = 'AGENT_AUTHOR', FALSE) AS clears_qualification
    FROM listing_sources
    WHERE (source_label IN ('NOT_AN_ESHOP', 'OTHER') AND source_labelled_by = 'AGENT_AUTHOR')
       OR (qualification = 'NEEDS_HUMAN' AND qualified_by = 'AGENT_AUTHOR'
           AND source_label IS NULL)
    FOR UPDATE
),
cleared AS (
    UPDATE listing_sources s
    SET source_label = CASE WHEN r.clears_label THEN NULL ELSE s.source_label END,
        source_label_note = CASE WHEN r.clears_label THEN NULL ELSE s.source_label_note END,
        source_labelled_at = CASE WHEN r.clears_label THEN NULL ELSE s.source_labelled_at END,
        source_labelled_by = CASE WHEN r.clears_label THEN NULL ELSE s.source_labelled_by END,
        last_crawl_outcome = CASE WHEN r.clears_label THEN NULL ELSE s.last_crawl_outcome END,
        last_crawl_error_kind =
            CASE WHEN r.clears_label THEN NULL ELSE s.last_crawl_error_kind END,
        crawl_failure_count = CASE WHEN r.clears_label THEN 0 ELSE s.crawl_failure_count END,
        next_crawl_at = CASE WHEN r.clears_label THEN NULL ELSE s.next_crawl_at END,
        qualification = CASE WHEN r.clears_qualification THEN NULL ELSE s.qualification END,
        qualification_note =
            CASE WHEN r.clears_qualification THEN NULL ELSE s.qualification_note END,
        qualified_at = CASE WHEN r.clears_qualification THEN NULL ELSE s.qualified_at END,
        qualified_by = CASE WHEN r.clears_qualification THEN NULL ELSE s.qualified_by END,
        crawl_enabled = s.crawl_enabled AND NOT r.clears_qualification,
        updated = NOW()
    FROM requalified r
    WHERE s.listing_source_id = r.listing_source_id
    RETURNING r.listing_source_id, r.clears_label, r.clears_qualification
),
label_events AS (
    INSERT INTO listing_source_label_events (listing_source_id, event_kind, actor)
    SELECT listing_source_id, 'CLEARED', 'SYSTEM' FROM cleared WHERE clears_label
)
INSERT INTO listing_source_qualification_events (listing_source_id, actor)
SELECT listing_source_id, 'SYSTEM' FROM cleared WHERE clears_qualification;
