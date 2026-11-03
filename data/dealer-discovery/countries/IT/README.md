# Italy dealer discovery

Ongoing assignment: **Italy (IT)**. Continue until the user stops it. This run owns only this directory and its newly claimed Italian API records. Root and other-country records retain their owners. Read every discovery ledger and current API identities before claiming a dealer; queue foreign leads or other owners’ enrichment here without changing their files or resources.

Latest checkpoint: `2026-11-02-it-107` — **183 Parties, 183 listing sources, 216 locations, 153 original logos, 138 identified platforms and 114 delivery profiles**. All 183 ingestion lists are empty. Excluded leads: **17**; deferred dealers: **51**. Paid research spend: **$0**. No product listings were ingested and no dealer was contacted. No pending writes.

[Ledger](dealers.jsonl), [frontier](frontier.jsonl), [index](index.tsv), [verification](verification.json), [resumption checkpoint](checkpoint.json), [public legal-identity checks](identity-verification.jsonl), [post-write identity audit](post-write-identity-audit107.json).

Runs 103–107 added Antichità Martini, Galleria Roberto Ducci, Aquafortis, Galleria d’Arte del Caminetto and Libreria Docet after first-party identity/catalogue checks, VIES legal-identity confirmation and source-specific API readbacks. Original logos were inspected for each. Ducci was identified as WooCommerce; Aquafortis and Caminetto platforms remain unknown. Only Fiorillo and Martini among the newest additions have advertised delivery terms; no shipping was inferred for Ducci, Aquafortis or Caminetto. All onboarding ingestion remains empty. Gallori Turchi remains deferred for unresolved operator relation/current stock; Arcadia was excluded because its native site identifies an auction-house operation.

Latest verification is incremental and checks all Party/source identities plus new resources; the last full readback was `2026-11-03T08:38:06.429705Z`, with the next due in run 108. Raw-journal dedupe found no ownership collision in other-country dealer ledgers. Run 103's shared-root verification path mistake and recovery are documented in its post-write audit; later runs use explicit country paths.

Next batch: `2026-11-02-it-108` — **Novarino Fine Art**. Run the due full resource readback before reviewing this queued independent dealer lead.

Pass explicit country ledger and output paths to every script. Credentials remain solely in the external owner-only `/tmp/aura-dealer-credential`. Public research, API snapshots and asset previews stay outside the repository under `/tmp/aura-dealer-discovery/IT/`. Preserve returned IDs, journal intents and reconcile uncertain writes before retrying. Never enable ingestion during this workflow.

Only facts assessed strictly above 95% confidence are adopted. Use current first-party identity and inspectable original stock; separate ordinary fashion, reprints, reproductions and sold items from qualifying antiques/vintage. A copyright year, accessible website, template or search snippet is not sufficient qualification. Keep personal founders, managers and agency credits separate from the operator; retain a published business name when no formal suffix is verified.

Research and interpretation rules:

- The country ledger is the source for exclusions, deferred cases, ownership evidence and dated follow-ups. Do not infer closure from temporary HTTP failures or similar business names.
- Add only a verified independent dealer with inspectable current own-stock antiques, vintage, art or design. Exclude marketplaces and businesses that operate auctions or auction houses. Keep exact legal operators separate from trading names and related family businesses.
- Use first-party contact, catalogue and legal identity evidence. A public VIES result verifies the tax ID/name/address relationship, not stock ownership or premises use. Keep registered addresses separate from actual business premises and never infer stock, dispatch or pickup from an address alone.
- Inspect original brand assets and native dimensions; omit unverified marks, ICO files, generic imagery and generated substitutes. Identify platforms only from direct technical fingerprints.
- Model advertised delivery geography, prices, restrictions and collection separately. Keep incomplete destinations partial, unknown values unknown, and item-specific shipping limitations in the ledger. No product listings are ingested and all onboarding ingestion arrays remain empty.
- Keep API credentials outside the repository in the owner-only credential file. Pass explicit Italy ledger and output paths to every helper; never mutate root or other-country records.

