# DMS Kinesis fixture custody

All JSON here is a synthetic parser contract vector, not a live AWS DMS capture. DMS data vectors use `{ data, metadata }`; control vectors use the documented top-level `{ control, metadata }` form.

`synthetic-*` names make that explicit. Existing unprefixed vectors are also synthetic.

`synthetic-product-listing-event-jsonb-string.json` models the serialized JSONB
payload shape observed during dev ingestion. Its identifiers and content are
synthetic; the corresponding object-payload vector must produce identical jobs.
