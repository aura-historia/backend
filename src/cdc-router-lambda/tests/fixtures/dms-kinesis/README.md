# DMS Kinesis fixture custody

All JSON here is a synthetic parser contract vector, not a live AWS DMS capture. DMS data vectors use `{ data, metadata }`; control vectors use the documented top-level `{ control, metadata }` form.

`synthetic-*` names make that explicit. Existing unprefixed vectors are also synthetic.
Do not add a `capture-*` fixture unless an approved R6 exercise produced the
sanitized, unchanged DMS Kinesis JSON representation. R6 evidence must prove
that a committed `search_filters` DELETE carries OLD filter ID, user ID, and
version as specified in `docs/migration-f7-dms.md`.
