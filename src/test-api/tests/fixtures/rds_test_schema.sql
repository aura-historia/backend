CREATE TABLE IF NOT EXISTS test_items (
    id          SERIAL PRIMARY KEY,
    name        TEXT    NOT NULL,
    value       INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS test_tags (
    id          SERIAL PRIMARY KEY,
    item_id     INTEGER NOT NULL REFERENCES test_items (id) ON DELETE CASCADE,
    tag         TEXT    NOT NULL
);
