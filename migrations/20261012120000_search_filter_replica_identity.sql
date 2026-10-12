-- Saved filters are no longer captured by CDC; full OLD rows are unnecessary.
ALTER TABLE search_filters REPLICA IDENTITY DEFAULT;
