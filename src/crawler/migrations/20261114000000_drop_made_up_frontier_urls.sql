-- Drops frontier URLs that only exist because found links were resolved against the page URL
-- instead of the page's <base href> (#2956). Such a URL repeats its first path segment further
-- down the path, as /en-GB/latest-items_pg_904/en-GB/<category>/<slug>/prod_<id> does, and every
-- crawl that seeded one nested it deeper. The frontier only extends discovery from the crawl root
-- (docs/crawler/scheduling-and-retries.md, discovery coverage), so a real page that happens to
-- repeat its first segment is found again from the root by a later crawl.
DELETE FROM listing_source_crawl_frontier
WHERE url ~ '^https?://[^/]+/([^/?#]+)/([^?#]*/)?\1(/|[?#]|$)';
