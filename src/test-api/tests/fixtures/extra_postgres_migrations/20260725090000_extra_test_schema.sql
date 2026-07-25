CREATE TABLE IF NOT EXISTS extra_test_items (
    id integer PRIMARY KEY,
    source_item_id integer NOT NULL REFERENCES test_items(id) ON DELETE CASCADE,
    name text NOT NULL
);
