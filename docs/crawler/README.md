# Crawler Crate — Documentation

The `crawler` crate is an async, LLM-assisted web crawler for antiques e-commerce shops. It discovers product URLs across shop websites (spider) and extracts structured product data from each product page (scraper). Shops are automatically kept in sync with the upstream shop service via a periodic registration loop.

## Contents

| File | What it covers |
|------|---------------|
| [dev-setup.md](dev-setup.md) | Local dev setup — Docker, demo binary, migrations, day-to-day workflows |
| [architecture.md](architecture.md) | CronJob, Shop Registration, Spider, Scraper — how they work and how they connect |
| [database.md](database.md) | PostgreSQL schema, table roles, key query patterns |
| [llm-integration.md](llm-integration.md) | The three LLM usages: URL pattern, CSS schema, state mapping |
| [domain.md](domain.md) | URL classification, UrlState lifecycle, product normalization |
| [configuration.md](configuration.md) | All config structs and their defaults |

## One-line summary of each subsystem

- **CrawlerCronJob** — drives all three loops; selects candidates from DB; fans out work with bounded concurrency
- **Shop Registration** — periodically syncs shops from the upstream shop service (OpenSearch) into the local `shops` and `shop_domains` tables
- **Spider** — crawls a shop's entire website, classifies every URL, batch-upserts into `shop_urls`
- **Scraper** — reads product URLs written by the spider, fetches their pages, extracts and normalises product data
