# Spain dealer discovery

Owner: Spain (ES) continuous discovery goal, started 2026-11-01. Research and API onboarding are authorized until stopped. Write only this directory and its managed identities; read all other ledgers for duplicates. Existing identities in another ledger stay with their owner. Foreign leads belong in this frontier with their target country.

[Ledger](dealers.jsonl) records evidence and API mutation journals; [index](index.tsv) provides the current decisions; [frontier](frontier.jsonl) preserves queued leads and component follow-ups; [verification](verification.json) records live readbacks. Shared workflow: [discovery skill](../../../../.agents/skills/discover-antique-dealers/SKILL.md).

Checkpoint `2026-11-03-es-221`: **242 Parties**, **243 listing sources**, **315 locations**, **221 inspected original logos**, **215 identified platforms**, **169 shipping profiles/defaults**. All ingestion lists are empty. **41 excluded**, **324 deferred**. Paid-tool spend: **$0**. Discovery remains active and continues into the next batch.

Use explicit `--ledger data/dealer-discovery/countries/ES/dealers.jsonl` and verifier `--output data/dealer-discovery/countries/ES/verification.json`. Credentials remain in the external owner-only file; never copy them here. Raw bounded page and asset caches are outside the repository under `/tmp/aura-dealer-discovery/ES/`.

Lessons and pending enrichment:

- Antiguedades.es has distinct bulky-item transport prices and international restrictions. Do not apply its ordinary free-delivery terms globally without item applicability.
- Cositas Antiguas has explicit registered-address (Santo Domingo 4) and visitor-shop (2) roles. Its Spain/Portugal shipping record preserves geographic scope with service eligibility unspecified; included delivery and pickup remain item-dependent. Existing Party/source and registered-location IDs are retained.
- Montoya advertises domestic quoted delivery and shop collection. International coverage is unspecified, so the imported profile remains partial; ASPX paths alone do not identify a commerce platform.
- Riera has inspectable WooCommerce stock and explicit worldwide delivery, but inspected pages conflict on Aragó 526/528 and do not establish the legal operator. Keep deferred until resolved.
- Stripe Directory returned no antique-dealer results; free search and first-party HTTP are the research sources.
- Keep source business geography separate from registered offices and separately evidenced inventory. Goldstein explicitly keeps some antique stock away from its showroom; there is no inventory inheritance.
- Candelabro's mainland scope is unresolved geographic text, with Portugal separately asserted. ANDANA DV explicitly limits delivery to Spain and France.
- Antiguart's legal and newer privacy notices disagree on tax identity and address. El Patio's linked shop pages lack inspectable stock. Both remain deferred.
- Matusalén and Goldstein have explicit all-item collection terms. Their unscoped shipping or item restrictions remain outside source shipping defaults.
- Repeat a full resource verification after every fifth batch; incremental checks retain original per-resource verification times in between.

- Logo inspection can use installed Cairo/librsvg and contrasting backgrounds without altering imported originals. Goldstein and Anticuable logo gaps were resolved with existing IDs retained; provider marks remain excluded.
- Fernández Nostalgia publishes free delivery from EUR 700 across Spain. Its online shop is the selected source; regional prices and conditional checkout pickup remain scoped observations.
- Z. Sierra has a Madrid shop and Fontioso warehouse/exhibition. Domestic shipping is quoted with a typical total of 2–5 business days; unspecified foreign destinations remain unresolved.
- Soul&Vintage's street/city, legal operator and email agree, but postcode and one phone differ between its PDF and live site. Those disputed components stay omitted.
- Deco for Curious has consistent operator and tax identity but unscoped differing street addresses; only independently advertised Bilbao shop geography is imported. Bolivar's current gallery address is confirmed by contact and product pages; its other legal address and portrait/favicon logo candidate remain follow-ups.
- Merale explicitly restricts delivery to mainland Spain; keep that unresolved geographic assertion rather than all Spain. Masked emails stay omitted.
- Romueble is independently identified as URBECOM from shared platform assets and the store attribution. Its two active contact premises are distinct from an unexplained alternate postal address.
- Margarita de Dios's original transparent PNG is verified and attached to its existing source. Native white-background inspection is sufficient; keep the original URL and dimensions.

- El Desván records its Rastro premises, separate Moraleja pickup point and returns warehouse. Online stock is explicitly not held in the shop, and foreign delivery policies differ between its primary site and merchant storefront; the imported delivery scope is domestic only.
- Unscoped legal addresses can differ from a current contact address and an explicitly stated registered office. Preserve the evidenced roles without treating every address as a current visitor shop. El Coleccionista Ecléctico’s permanent-address restrictions and Huberto’s bulky-item conditions remain shipping follow-ups.

- Frontier entries without an established dealer domain use a distinct `leadId`, `kind: NAME_LEAD` and `domain: null`. Their URL is a research locator, never an asserted dealer domain; they are not onboarding records. Foreign name leads retain `targetCountry` and stay here for country-owner handoff.
- El Pianillo is directly identified as Webador and its legal legacy domain redirects to the current shop. Its international delivery assertion remains unresolved; El Anticuario retains literal mainland coverage with no universal weight-based price.

- Delivery geography can be retained with service eligibility left unspecified when item dimensions, specialist freight or access conditions prevent a faithful source-wide offer. Such profiles preserve coverage evidence without promising delivery, rates or timing. Keep detailed restrictions and component conflicts in the ledger and frontier.

- Martín Ramos retains literal weight-limited shipping geography and a known Burgos collection site with eligibility unspecified. González has conflicting foreign-delivery and fee clauses; only consistent domestic coverage is imported. Girona's current mainland opening promotion is strictly above EUR 50 (minimum 5001 minor units) and has a prompt recheck; unnamed international destinations stay unresolved. Vivar remains deferred for two explicit conflicting corporate identities.

- Hernando Colón preserves strict shipment-value thresholds; lower-value postal bands cannot become unconditional rates. Numivium separates a Madrid correspondence address from public shops. La Dobla has a temporary handling delay and separately managed bullion orders. Baiget's header coin photograph is omitted as an uncertain identity image. Monge's inclusive EUR 150 free-Spain threshold and unspecified 15-day basis/trigger remain literal.

- Batch 037 preserves verified component facts for five deferred dealers. Velvet Art explicitly consigns through external auction houses and does not organize those auctions. Gargollo/Gloria Monasterio cannot yet be assigned one operator. Cebrián and Solares lack an explicit current legal merchant. ANTECO's active accented domain is keyed by IDNA (`xn--antigedadesanteco-62b.es`); the unaccented placeholder is reconciled in the frontier, with no closure finding.

- Escorial's detailed period-stock description is present in product metadata/JSON-LD despite its omission from the text preview. Its two public shops and legal correspondence address remain distinct. La Fábrica has two current shops plus separate legal correspondence; quoted domestic/international delivery remains partial. Aral and Muebles del Mundo retain verified components while specific current antique-stock qualification is unresolved.
- Leave rate charging units unknown unless separately evidenced. Batch 038 cleared inferred `PER_SHIPMENT` bases from five existing profiles, preserving their IDs, amounts, thresholds, coverage and selected defaults; all writes passed readback.

- Llamas's mutually supported mainland/Baleares geography is retained while conflicting foreign/price tables remain unresolved. La Lonja's strict-above EUR 100 free threshold preserves the overlap of its header/product policies, using the explicit legal privacy name. Craven's insured, professionally packed Spain delivery starts at EUR 4.90 with charging unit unknown; other possible destinations require confirmation. Regional timing and collection are not generalized.

- Joieria Vintage identifies CAENADE S.L. and has specific reservable period jewellery for in-store sale. Its conflicting postcodes and malformed VAT field stay omitted; its transposed Gmail spelling is preserved from the published obfuscation markup and CSS. No shipping or independent collection contract is inferred from in-store sales. Ayeres works over ordinary HTTP after HTTPS failures but remains deferred for an explicit complete legal operator.

- Eyrabooks date searches can match author lifespans, internal IDs and historical subjects. Verify the actual edition and condition on its item page; printer terms such as `imprenta` can provide better historical-stock leads. The inspected advanced forms expose no year filter, so do not invent one. Deduplication also includes verified components retained with deferred leads.

- Empty text previews can hide useful original image headers/contacts or a published legal modal. Inspect native assets and follow the exact public legal control. A sold marker on the individual object overrides a homepage card or lingering price; neither sparse text nor old styling alone establishes closure.

- Collection site references: keep terms.collection.site null in reviewed observations and set the top-level collectionSiteIndex to the evidenced location index. The standard helper resolves the canonical PARTY_LOCATION object. Numeric site values are invalid. Batch180 HTTP400 was GET-reconciled with no profile/default created, corrected and resumed using the saved Party/source/location IDs and original creation key; the Spain temporary append helper now rejects numeric collection.site before writes.

- Domain scan flags need literal-host review: normalized text can cross punctuation (batch183 l-contemporary.com matched unrelated German-dealer prose) or equal a same-lead gallery name. Review canonical hosts/aliases and operator/contact evidence before deciding duplicates. Use set -e or subprocess(check=True) for dependent guard/fetch/write command sequences so a failed guard stops later steps. No root or foreign dealer files/resources were changed during the reconciliation.

- A directory’s website link may lead to a journalist’s article rather than the merchant. Inspect ownership before assigning the host, platform, logo or contacts; keep the dealer name-only when no primary site is resolved.

- Shipping zones accept at most 64 areas. Split larger explicit country lists across equivalent zones, and place unmapped destination assertions after known positive zones. Reconcile a rejected profile creation against the live profile list and default before correcting input; retain all saved Party/source/location IDs.
