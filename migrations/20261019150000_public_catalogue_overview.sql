-- Rebuildable derived statistics: one row, replaced by the scheduled refresh and never
-- moved back to an older calculation. Public reads never aggregate listings directly.
CREATE TABLE public_catalogue_overview (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    as_of timestamptz NOT NULL,
    sources_indexed bigint NOT NULL CHECK (sources_indexed >= 0),
    live_objects bigint NOT NULL CHECK (live_objects >= 0),
    countries bigint NOT NULL CHECK (countries >= 0),
    languages bigint NOT NULL CHECK (languages >= 0),
    new_objects_last_7_days bigint NOT NULL
        CHECK (new_objects_last_7_days BETWEEN 0 AND live_objects),
    CHECK (sources_indexed <= live_objects AND countries <= live_objects)
);
