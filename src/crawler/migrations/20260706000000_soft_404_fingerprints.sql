ALTER TABLE shop_domains
    ADD COLUMN IF NOT EXISTS soft_404_fingerprint TEXT,
    ADD COLUMN IF NOT EXISTS soft_404_probe_url TEXT,
    ADD COLUMN IF NOT EXISTS soft_404_checked_at TIMESTAMPTZ;
