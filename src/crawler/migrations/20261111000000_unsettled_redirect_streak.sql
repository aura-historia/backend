-- The target a product URL's last scrapes redirected to without settling the URL, and how many
-- consecutive scrapes ended there, so its recheck delay grows while the redirect stays unsettled
-- (docs/crawler/networking.md, redirect removal evidence). Meaningful only while
-- `last_error_kind` is `RedirectTargetUnverified` or `RedirectDestinationUnsettled`.
ALTER TABLE listing_source_urls
    ADD COLUMN unsettled_redirect_target TEXT,
    ADD COLUMN unsettled_redirect_streak INT NOT NULL DEFAULT 0,
    ADD CONSTRAINT listing_source_urls_unsettled_redirect_shape_check CHECK (
        (unsettled_redirect_target IS NULL AND unsettled_redirect_streak = 0)
        OR (char_length(unsettled_redirect_target) > 0 AND unsettled_redirect_streak > 0)
    );
