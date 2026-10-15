\set ON_ERROR_STOP on

-- One-time ledger adoption of the squashed crawler baseline (#2071). Run with psql as
-- the crawler database owner while no crawler process runs, before starting a binary
-- built from the squashed migrations. It accepts only a ledger that recorded the complete
-- pre-squash chain, whose schema equals the baseline, and rewrites the ledger without
-- touching crawler tables or data. Each pre-squash checksum may come from an LF or a CRLF
-- checkout. Later migrations stay recorded unchanged. An adopted ledger is left unchanged.

BEGIN;
LOCK TABLE _sqlx_migrations IN ACCESS EXCLUSIVE MODE;

DO $$
DECLARE
    baseline_version CONSTANT bigint := 20260101000000;
    last_squashed_version CONSTANT bigint := 20261015120000;
    baseline_checksum CONSTANT text :=
        'f8b91aaf4faad12f7755a9313dd8a2551386e033957d2f6bd8d29cf57924eb872937e76e833f55c5e448f333cb3bf7a3';
    -- Accepted checksums per version: [LF checkout, CRLF checkout].
    pre_squash_chain CONSTANT jsonb := jsonb_build_object(
        '20260101000000', jsonb_build_array('f5afbdf7464128566474ac9151a8e285eb7e47048ceffd8f0aa62ffc31e40da556109c3c1799c9fcb9b96c6032f60960', '0c534930f89d6510958a9a38f30e429691ed59c7d0861dd8afb2de171562007ebb01aff6ef1a19d6881bb442362b4967'),
        '20260514000000', jsonb_build_array('978eb4e2d78188bfccbed6da3f47bf69ada5c2ecd6b30920e52c6e7a78dc410d6d2b0d6daa6f957a64b70d0acf8acb8c', '7e95b31194c911a5176056e14122de330a7beca7d720975c496496aef0843544c218c818bcbbbec7c0d95bad21886faf'),
        '20260711000000', jsonb_build_array('40a400ee33eba01f28dae016eea7e7400c665ced058daae508e0bfa867f0ac9f9c6a539884463df04d91ef1b0b4d2042', '0e32c1fd9c7bedcc8ef345f5d3583847ee912a5b6b8e3e10e1a739f3432cc93d33eb349b5ced4de540bac4729c216a35'),
        '20260817000000', jsonb_build_array('dcc90258002db66a6e3406056a7b73353c7155d250c4f9390def20e2a8f6e51e39aa3df2c7377cf0cbfc5fe29c5dceda', 'c3bd1c6dcc7bcb62073e3b4dddf997739c7cb1862b8d52baab13a6b88493f462be79afc1f5a8839c5bf9faf0e7327da9'),
        '20260831000000', jsonb_build_array('127d31dc34150bc83ce20a5c075d6c8ca71cd79e029501e260f824b8d1e987fe166ab6648fba4f0a8479e8eb62a9eecd', '04b911e1cf2ec14bc80871103ecbcc1b508436dec853459cd22d2d4a4dcee0a7a6df05c14f142f09afc26e7285374ae8'),
        '20260831000001', jsonb_build_array('78b5bc1940f220a5009da2ae9ebc147fff01d6d012e96242a7a8f6af0e603c118bca892b400a7b680aad0ac3aae47073', '9a1cc5290111c04de4cf28ccf3f9ad00ec5386032070aa4aac036ee71e4272cdd0581b84f40bf7836f5984cd8ee64dc9'),
        '20260927000000', jsonb_build_array('ff5441df4de9c8f077ce3edbfcb349512fb7d03debee5c305d3d88ee189cd08b5065fa0d90b1e5f7027f838fde585492', '5cbfedc6ef679f64af300f28f025f34d84e8688945551a843e7de712ed68e318410cfd987768d5a0a4b3c968d052e801'),
        '20260929190000', jsonb_build_array('c62c541e77927e19ff0bf403b5edc7b08875012e57329c6a9fa582f5914764920c469d7cbd51487c8a3b04d4881ce55e', '7e3cbf8e3ad451b43c40e57a70bf78fa4674f1a7c6befe3d9f6a257cf34bad4f934fbfab9a72fc5200f59bff526f6efd'),
        '20260929190100', jsonb_build_array('5346dbada39eede3121187892cad86b5ff6a45e5dabb1ef3d8e6fef15759a09e8675668185972d3d8362edb1b4d92cc5', 'de1758dd35d56665503fcab451a0d307d7aa0f03f9259d03b6f15a037dcdf8670b6868e667e098c3fdbe6799b275b57a'),
        '20261009200000', jsonb_build_array('d434ed9b8433cc7f6fd8b3d7187a708ecc514616687a77d387d05300e9bd5b21d1afeb05999ed2cddfe37cf6e8bad0dd', '198f46cb60e6908d39c2f06d2a789c36c7b6bb6b3f84b0dcb4ec48f0f182f8c6caa89c4ed5a8f65a3e0b3420a03e75cd'),
        '20261015120000', jsonb_build_array('94d7deeb101332edce26cb96f397c99403c25ecae0a2b48297652b0c78e9df25e5c68c06e63f4ec7f638b390dff7a919', '95f09e816e3b95330b54dae123d3491b2f22382bc5214e1bf020215ff3b235bb6c27a758e0dbc611f098318a70ca9ba0')
    );
    squashed_rows jsonb;
BEGIN
    IF EXISTS (SELECT 1 FROM _sqlx_migrations WHERE NOT success) THEN
        RAISE EXCEPTION 'crawler migration ledger contains an unsuccessful migration';
    END IF;

    SELECT coalesce(jsonb_object_agg(version::text, encode(checksum, 'hex')), '{}'::jsonb)
    INTO squashed_rows
    FROM _sqlx_migrations
    WHERE version <= last_squashed_version;

    IF squashed_rows = jsonb_build_object(baseline_version::text, baseline_checksum) THEN
        RAISE NOTICE 'crawler migration ledger already records the squashed baseline';
        RETURN;
    END IF;
    IF (SELECT array_agg(key ORDER BY key) FROM jsonb_object_keys(squashed_rows) AS key)
        IS DISTINCT FROM
        (SELECT array_agg(key ORDER BY key) FROM jsonb_object_keys(pre_squash_chain) AS key)
        OR EXISTS (
            SELECT 1
            FROM jsonb_each_text(squashed_rows) AS recorded(version, checksum)
            WHERE NOT (pre_squash_chain -> recorded.version) ? recorded.checksum
        ) THEN
        RAISE EXCEPTION 'crawler migration ledger is not the complete pre-squash chain';
    END IF;

    DELETE FROM _sqlx_migrations
    WHERE version <> baseline_version AND version <= last_squashed_version;
    UPDATE _sqlx_migrations
    SET checksum = decode(baseline_checksum, 'hex')
    WHERE version = baseline_version;
END
$$;

COMMIT;
