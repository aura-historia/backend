-- The pages of a pending product-schema review's ListingSource that the console projected for the
-- review (an agent's inspect_page), in the spelling the projection fetched, with when it last did.
-- An agent's record that the ListingSource has no sold page names only pages projected since a
-- human last acted on the review (docs/crawler/extraction-and-review.md, approval guard).
CREATE TABLE crawler_review_inspected_pages (
    review_id UUID        NOT NULL REFERENCES crawler_reviews(review_id) ON DELETE CASCADE,
    url       TEXT        NOT NULL,
    inspected TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (review_id, url)
);
