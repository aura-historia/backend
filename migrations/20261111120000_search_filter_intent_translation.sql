-- Internal English translation of a saved search's semantic intent (issue #2904). Matching state
-- beside the embedding: never part of the user-visible search, and only current while its
-- source fingerprint matches the search's product queries and enhanced description.
ALTER TABLE search_filters
    ADD COLUMN intent_translation jsonb,
    ADD CONSTRAINT search_filters_intent_translation_object CHECK (
        intent_translation IS NULL OR jsonb_typeof(intent_translation) = 'object'
    );
