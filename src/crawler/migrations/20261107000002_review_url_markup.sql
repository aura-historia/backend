-- Whether the page of a URL-pattern review's sample described exactly one schema.org Product when
-- the author or reviewer last inspected it; NULL for a URL nobody inspected. The approval guard
-- compares the share of such pages with what the candidate pattern takes
-- (docs/crawler/extraction-and-review.md, approval guard).
ALTER TABLE crawler_review_urls ADD COLUMN describes_one_product BOOLEAN;
