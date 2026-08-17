ALTER TABLE crawler_review_pages
    DROP CONSTRAINT IF EXISTS crawler_review_pages_role_check;

ALTER TABLE crawler_review_pages
    ADD CONSTRAINT crawler_review_pages_role_check
    CHECK (
        role IN (
            'PRIMARY',
            'SEED',
            'TRIGGERING_REPAIR_PAGE',
            'TRIGGERING_GENERATION_PAGE'
        )
    );
