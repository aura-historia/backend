-- Listing text assessment also flags nudity, erotica and pornography as sensitive content.
ALTER TABLE product_listing_content_assessments
    DROP CONSTRAINT product_listing_content_assessments_category_check,
    ADD CONSTRAINT product_listing_content_assessments_category_check
        CHECK (category IS NULL OR category IN ('NAZI_GERMANY', 'NUDITY'));
