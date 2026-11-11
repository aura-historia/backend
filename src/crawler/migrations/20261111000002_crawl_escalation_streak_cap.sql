-- Every classified crawl failure kind now escalates to NEEDS_OPERATOR (#2892), a transient kind
-- on its fifth consecutive failure. Transient streaks built before, when these kinds never
-- escalated, can be far longer, so every such source would stop on its next failure at once.
-- Cap them at two failures: the next failure is the third, which takes the long cooldown as
-- before, and the source escalates only after it has failed three more times under the new
-- policy, with its own crawls. The cap applies whatever the last run's outcome: a stopped,
-- truncated, rate-limited or incremental run, or one that waits for a URL-pattern review, keeps
-- the failure streak, and the next failure continues it.
--
-- Durable-block streaks (AccessDenied, JavascriptRequired) keep their count: those sources
-- escalate on their next failure, which their existing cooldowns of up to a month spread out,
-- and a shop that refuses the crawler is what an operator has to see. BotProtection and the
-- recoverable kinds already escalated before and keep their streaks too.
UPDATE listing_sources
SET crawl_failure_count = 2
WHERE last_crawl_outcome IS DISTINCT FROM 'NEEDS_OPERATOR'
  AND crawl_failure_count > 2
  AND last_crawl_error_kind IN (
      'EmptyCrawl',
      'TinyCrawl',
      'NoAcceptedPages',
      'RateLimited',
      'CloudflareChallenge',
      'ConnectError',
      'ServerError'
  );
