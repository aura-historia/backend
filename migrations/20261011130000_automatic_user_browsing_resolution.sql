-- Private browsing changes acceptance, not business-location review or provider rights.
ALTER TABLE geo_resolutions DROP CONSTRAINT geo_resolutions_policy_version_check;
ALTER TABLE geo_resolutions ADD CONSTRAINT geo_resolutions_policy_version_check
    CHECK (policy_version IN ('PRECISION_FIRST_V1','AUTOMATIC_USER_BROWSING_V1'));

-- G8's existing user-address inputs have no ranking bias/language/hard constraints.
-- Stop on an unexpected legacy format rather than silently computing a wrong identity.
DO $$ BEGIN
    IF EXISTS (
        SELECT 1 FROM geo_resolutions
        WHERE NOT erased AND input->>'purpose'='USER_LOCATION'
          AND policy_version='PRECISION_FIRST_V1'
          AND (jsonb_typeof(input) IS DISTINCT FROM 'object'
            OR input - ARRAY['query','limit','submitted','constraint_country',
                'constraint_subdivision','reference_release','bias','language',
                'scope','owner','purpose','retention_context'] <> '{}'::jsonb
            OR jsonb_typeof(input->'query') IS DISTINCT FROM 'string'
            OR jsonb_typeof(input->'owner') IS DISTINCT FROM 'string'
            OR (input->>'owner' ~ '^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$') IS NOT TRUE
            OR jsonb_typeof(input->'limit') IS DISTINCT FROM 'number'
            OR (input->>'limit' ~ '^([2-9]|10)$') IS NOT TRUE
            OR input->>'scope' IS DISTINCT FROM 'USER'
            OR input->>'retention_context' IS DISTINCT FROM 'PRIVATE'
            OR input->>'reference_release' IS DISTINCT FROM 'iso-codes-4.20.1'
            OR input->>'bias' IS NOT NULL OR input->>'language' IS NOT NULL
            OR input->>'constraint_country' IS NOT NULL
            OR input->>'constraint_subdivision' IS NOT NULL
            OR (input->>'submitted' IS NOT NULL AND (
                jsonb_typeof(input->'submitted') IS DISTINCT FROM 'object'
                OR (input->'submitted') - ARRAY['reference_release','address_text',
                    'country','subdivision','postal_code'] <> '{}'::jsonb
                OR input->'submitted'->>'reference_release' IS DISTINCT FROM 'iso-codes-4.20.1'
                OR EXISTS (
                    SELECT 1 FROM jsonb_each(input->'submitted') AS component
                    WHERE jsonb_typeof(component.value) NOT IN ('string','null')
                )
            )))
    ) THEN
        RAISE EXCEPTION 'Unsupported legacy private resolution input';
    END IF;
    IF EXISTS (
        SELECT 1 FROM geo_resolutions
        WHERE NOT erased AND input->>'purpose'='USER_LOCATION'
          AND policy_version='PRECISION_FIRST_V1' AND generation=9223372036854775807
    ) THEN
        RAISE EXCEPTION 'Legacy private resolution generation is exhausted';
    END IF;
END $$;

-- Match the exact UTF-8, length-prefixed ResolutionInput fingerprint. This helper
-- exists only in the migration session and never exports private input.
CREATE OR REPLACE FUNCTION pg_temp.user_resolution_fingerprint(fields text[]) RETURNS text
LANGUAGE sql IMMUTABLE STRICT AS $$
    SELECT encode(sha256(string_agg(
        int8send(octet_length(convert_to(value,'UTF8'))::bigint) || convert_to(value,'UTF8'),
        ''::bytea ORDER BY ordinal
    )),'hex') FROM unnest(fields) WITH ORDINALITY AS f(value,ordinal)
$$;

-- Existing legacy user evidence is invalidated, not relabeled as newly accepted.
-- IDs/owners/bindings stay stable. Spent-call budgets and outstanding reservations
-- survive; stale claims cannot apply evidence under the new policy/generation.
WITH upgraded AS (
    UPDATE geo_resolutions r SET
        policy_version='AUTOMATIC_USER_BROWSING_V1',
        fingerprint=pg_temp.user_resolution_fingerprint(ARRAY[
            'RESOLUTION_INPUT_V1',r.profile,'AUTOMATIC_USER_BROWSING_V1','iso-codes-4.20.1',
            r.input->>'scope',coalesce(r.input->>'owner',''),r.input->>'purpose',
            r.input->>'retention_context',r.input->>'query',r.input->>'limit'
        ] || CASE WHEN r.input->>'submitted' IS NOT NULL THEN ARRAY[
            'SUBMITTED',r.input->'submitted'->>'reference_release',
            coalesce(r.input->'submitted'->>'address_text',''),
            coalesce(r.input->'submitted'->>'country',''),
            coalesce(r.input->'submitted'->>'subdivision',''),
            coalesce(r.input->'submitted'->>'postal_code','')
        ] ELSE ARRAY['NO_SUBMITTED'] END || ARRAY['NO_CONSTRAINT','NO_BIAS','']),
        generation=r.generation+1,
        status='PENDING',attempts=0,evidence=NULL,valid_until=NULL,last_failure=NULL,
        next_attempt_at=clock_timestamp(),pending_since=clock_timestamp()
    WHERE NOT r.erased AND r.input->>'purpose'='USER_LOCATION'
        AND r.policy_version='PRECISION_FIRST_V1'
    RETURNING r.resolution_id,r.generation
), cleared AS (
    UPDATE geo_resolution_bindings b SET
        selected_candidate=NULL,selected_generation=NULL,reviewed_actor=NULL
    FROM upgraded u WHERE b.resolution_id=u.resolution_id RETURNING b.resolution_id
), invalidated AS (
    INSERT INTO geo_resolution_changes(change_id,resolution_id,generation,change_kind)
    SELECT (
        lpad(to_hex(floor(extract(epoch FROM clock_timestamp())*1000)::bigint),12,'0') ||
        '7' || substr(replace(gen_random_uuid()::text,'-',''),14,3) || '8' ||
        substr(replace(gen_random_uuid()::text,'-',''),18,15)
    )::uuid,u.resolution_id,u.generation,'INVALIDATED' FROM upgraded u
)
INSERT INTO geo_resolution_wakeups(resolution_id,generation)
SELECT resolution_id,generation FROM upgraded ON CONFLICT DO NOTHING;
