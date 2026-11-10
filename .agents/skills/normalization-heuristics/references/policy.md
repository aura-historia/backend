# Heuristic decision policy

A heuristic is accepted only when it is **generic**, **evidenced by the corpus** and **cannot
write a confidently wrong value**. Abstaining is always safe: an unresolved field is preserved,
and a wrong value is silently published. When in doubt, leave the case unresolved, pin it with a
negative test and name it in the PR.

Apply these rules to every distinct raw value behind a recorded issue, largest first.

## Classify each raw value

| Class | Examples | Action |
|---|---|---|
| Generic label or format the contract already models | purchase buttons, `Sold`, `Reserved`, `円`, `12 € 50` | Implement, with positive and negative tests |
| Plausible, but meaning depends on the dealer | enquiry/contact buttons, `Reservieren`, `POA` | Implement only with per-dealer evidence (below); otherwise a negative test |
| Crawler extraction artefact | image URLs, `.`, `javascript:…`, a price in the availability field | Not normalization. List it under crawler follow-ups |
| Source configuration | bare amounts without a currency, dual-currency displays | Not code. List the source and the needed fallback under operator follow-ups |
| Sentence prose or quantity defaults | `1 d'articles en stock, dépêchez-vous…`, bare `1`/`0` | Leave unresolved; there is already a pinned negative |
| Not expressible by the model | region-scoped shipping prices, a domestic-only offer | Leave it. Abstaining is correct |

A rule must be keyed on the label or format, never on a source. A label that one dealer uses and
that other shops would mean differently stays unresolved.

## Availability

- Exact labels live in the lists in `availability.rs`, matched after layout normalization
  (case, separators, icon glyphs, enclosing brackets, a leading field caption, terminal `.`/`…`/`!`).
  Quantity evidence goes through the regex sets, which resolve only a whole label and diagnose
  qualified matches (`not 2 available`, `Buy now £195 from March`).
- **Purchase actions** (add to cart/bag/basket, buy, order, bestellen…) mean the item can be
  bought and map to `AVAILABLE` in any language. Site navigation and form buttons are not
  purchase actions for the listing (`Acheter un livre ancien`, `Valider`).
- **Enquiry and contact actions** map to `AVAILABLE` only when the same dealer demonstrably marks
  sold items differently. The corpus holds only cases with issues, so sold items that resolved
  cleanly are missing from it. Check canonical state with an aggregate query:

  ```sql
  SELECT substring(s.url from '^https?://([^/]+)') AS host, coalesce(p.availability, '<null>'), count(*)
  FROM product_listings p JOIN listing_sources s USING (listing_source_id)
  WHERE substring(s.url from '^https?://([^/]+)') IN ('www.dealer-a.example', 'www.dealer-b.example')
  GROUP BY 1, 2 ORDER BY 1, 3 DESC;
  ```

  Evidence is `SOLD_OUT`/`RESERVED` listings for that dealer, or a sold label beside the enquiry
  label in the corpus (`analyze.py labels`). Generic phrases (`contact us`, `E-Mail us`) stay
  unresolved even with evidence, because other shops use them on sold items too.
- The crawler reuses `quick_check_availability` for plausibility checks
  (`src/crawler/src/scraper/plausibility.rs`, `src/crawler/src/review/page_content_checks.rs`).
  Run `cargo test -p crawler --lib` after every availability change.

## Price

- A currency marker must be unambiguous. `円` is JPY, `원` is KRW; bare `¥`, `R` and words such as
  `euro` stay ambiguous. The symbol table has no word-boundary check, so a word would match inside
  `europe`.
- A layout rule must fix the whole string's shape (for example, exactly two cent digits after the
  euro sign) and reject near misses in tests (`12 € 500`, `€ 12 50`, `-12 € 50`).
- Dual-currency displays resolve only through a source fallback currency. They are configuration,
  not code.
- Check whether web-crawl `fallbackCurrency` reaches the normalization context before you
  recommend setting one:

  ```sh
  grep -n fallbackCurrency src/crawler/src/scraper/raw_input.rs
  ```

  On 2026-11-10 it did not (#2858), so configuring a fallback changed nothing. If it is still
  unwired, say so in the PR instead of recommending configuration.

## Shipping

- Statements are whole clauses from the lexicon (`src/product-listing-normalization/src/shipping/lexicon/`).
  Consumers act only on `Resolved`; `Unsupported` and `NoAssertion` both write nothing, and only
  `Unsupported` is recorded as a diagnostic.
- Before you change a shipping rule, measure what the vetoes hide. Temporarily disable them (or add
  an `eprintln!` of the vetoing clause) in `check_shipping`, replay locally to a local file, then
  restore the file with `git checkout` and confirm `git diff` shows no instrumentation. On
  2026-11-10, with every veto disabled, 19,430 of 19,431 shipping-issue cases still produced no
  assertion.
- A veto must keep blocking every statement it qualifies. Never trade safety for fewer diagnostics.
- A phrase that is already in the lexicon but fails because of decoration or a neighbouring
  clause is worth a closer look. Accept the change only if the replay actually resolves cases;
  drop it otherwise (decorated worldwide statements resolved zero cases in #2858, because each sat
  next to a regional price table).

## Acceptance gates

1. `analyze.py diff BASELINE AFTER` exits 0, or each flagged item is explained in the PR as
   intended (for example, a reason re-attributed on a case that already abstained).
2. Every newly resolved label or amount is reviewed by hand (`analyze.py tally` on the baseline,
   then compare the replayed field values).
3. A change that resolves no corpus case is removed.
4. Every new rule has unit tests for positive cases and for the near misses it must reject.
5. Reviewed fixtures cover each change with at least one `REPAIRED` and, where a boundary exists,
   one `NEGATIVE` case.
6. `PRODUCT_LISTING_RAW_VALUES_NORMALIZER_VERSION` is raised once per PR whenever a decision for
   an existing input can change. It is provenance only.
