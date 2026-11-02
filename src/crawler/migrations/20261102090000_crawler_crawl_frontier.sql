-- When the domain's last spider run of any kind ended. A domain whose last full crawl is recent is
-- crawled incrementally once this is a day old (docs/crawler/scheduling-and-retries.md, discovery
-- coverage).
ALTER TABLE listing_source_domains
    ADD COLUMN last_crawl_run_at TIMESTAMPTZ;

-- The URLs a cut-short crawl found or was seeded with but did not visit. The domain's next crawl
-- visits them after its link discovery from the root, so a truncated crawl continues where it
-- stopped. Each run replaces the set; a crawl that reached everything leaves it empty.
CREATE TABLE listing_source_domain_crawl_frontier (
    domain_id UUID NOT NULL REFERENCES listing_source_domains(domain_id) ON DELETE CASCADE,
    url       TEXT NOT NULL,
    CHECK (char_length(url) > 0),
    PRIMARY KEY (domain_id, url)
);
