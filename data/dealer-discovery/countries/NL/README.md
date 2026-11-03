# Netherlands dealer discovery

Ongoing Netherlands discovery is active until the user stops it. This country run owns only this directory and its dealer identities. Read every country ledger before claims; never change another owner’s files or API records. Use explicit ledger/output paths with shared scripts. Research uses free tools; credentials remain in the external owner-only file.

Current research and aggregate live-readback checkpoint `2026-11-03-nl-495`: **472 Parties, 486 listing sources, 512 locations, 344 original logos, 413 identified platforms and 369 advertised-delivery profiles/defaults**. Every ingestion list is empty. **43 excluded and 252 deferred identities**. Paid-data spend: **$0**. These counts cover all NL-owned ledger components, not the entire platform. Vergulde Kat was added with its KvK/VAT/contact facts, Oosterwolde home gallery and inventory location, WooCommerce, inspected original logo, worldwide shipping and free delivery within the Netherlands; all new resources and empty ingestion were read back. Run 492 added Tribal Design with its named Amsterdam gallery operator, address, WooCommerce and original 133x100 JPEG logo; delivery was left unasserted. African Art Amsterdam was deferred because its pages describe a personal-collection sale and do not verify the legal seller or a Dutch business address. Run 493 added Galerie Lemaire with its owner, Amsterdam contact/location and WordPress catalogue; no logo or shipping facts were asserted. Lodder Asian Art was deferred pending evidence of direct current own-stock sales. Run 494 added Van Pruissen Asian Art with its founders, director/contact, Arnhem appointment-gallery location and current priced own-inventory catalogue; legal registration, technology, logo and delivery metadata remain unverified. Run 495 added Vintage Kroonjuweeltjes with two Amsterdam locations, Shopify, current vintage jewelry and an inspected original logo; shipping details were omitted due to conflicting first-party terms.

Open identity reviews: **Oldin and D’Antan** publish the same VAT number with different contacts and addresses; **Studio Eklektik** publishes a KvK number that conflicts with a same-name registration at the same address. Preserve existing resource sets and empty ingestion while reconciling identity; see [identity-review.json](identity-review.json). Resource readback does not resolve these reviews.

Latest aggregate verification is `INCREMENTAL` across all five NL-owned components (run 495); all 486 listing sources have empty ingestion, with 482 native sources checked or reused and four hosted sources retaining their prior readbacks. Runs 480, 481, 483 and 484 added no API resources; run 482 added Galerie Aanblick. Antiques & More was excluded after its first-party site confirmed auction-house and Catawiki consignment sales; Dutch Antique Company was deferred because its site returned 404 across domain variants, without inferring inactivity. Nana Pearl was deferred because current porcelain offers lack age or provenance establishing antique/collectible stock. Strabagante remains deferred until current stock is available. Rijp Vintage was added with its owners, KvK, city-only Oss warehouse geography, Wix storefront, original high-resolution logo and worldwide quoted delivery; local furniture rates and appointment pickup were preserved without generalizing scope. Replate was added with its KvK/VAT business identity, non-visitor correspondence address, WooCommerce, original logo, current orderable 1962-designed tableware and worldwide cart-calculated tracked shipping. Galerie Aanblick was added under the legal sole proprietorship Frima Financieel Advies, with current orderable art, WooCommerce, original PNG logo and its Haren gallery location; no shipping terms were verified. All three new sources, Parties, locations, logos, platform metadata, applicable shipping profiles/defaults and empty ingestion lists were read back. Search queries and decisions are in [searches.jsonl](searches.jsonl); see [verification.json](verification.json). Timeless Brocante and Broekente Brocante were deferred because current own-stock offers could not be established from fresh first-party item evidence. Roosje Hindeloopen was deferred because its linked webshop is under work and shows no current item offers; reassess the catalogue when live. Edward Wells was deferred after its contact page and terms showed conflicting KvK numbers despite matching contact and VAT details. Kunstenantiekverkoop.nl was deferred after its current first-party collection showed antique offers but did not identify a legal seller, registration or business address; its description says the collection came from an inheritance. Nico van den Assem was deferred pending verifiable current item availability; his first-party business profile and contact details are corroborated by PAN Amsterdam, while the inspected catalogue pages date to October 2025. Hollandia Housewares was deferred because no legal operator or first-party Netherlands business address could be verified and its homepage shows two different WhatsApp numbers. Le Patiné Brocante was added with a verified KvK/VAT identity, registered address, Mijnwebwinkel platform and Netherlands PostNL delivery terms. Its email was omitted because first-party pages disagree on its spelling; no logo was confidently identifiable, and pickup is explicitly unavailable. Charmig was added with a verified KvK/VAT identity, Veenendaal showroom/warehouse, WooCommerce, original PNG logo and empty ingestion. Its item-specific parcel/furniture shipping distinctions remain in the ledger without a source-wide profile. Antiques Forever was added with verified identity, location, WooCommerce and original logo; its DPD terms are retained in the ledger but no shipping profile was created after two rejected profile writes. Camera Obscura Elburg was added with verified KvK, location, WooCommerce, original logo and empty ingestion; PostNL package-format rates remain documented without a source-wide profile. Mammoet Oude Bouwmaterialen BV was added with its Den Haag showroom, WooCommerce, original logo and empty ingestion; local delivery terms remain recorded without an API profile because the territory and price are not specified. l’Argenterie Antique was added with its registered silver business, Gees appointment address, MMBeheer CMS and original logo; its seller-paid shipping terms lack destinations and rates, so no profile was created.

[Ledger](dealers.jsonl) holds evidence, assessed confidence, omissions, resource IDs and mutation checkpoints. [Index](index.tsv) is the generated domain/alias lookup. [Frontier](frontier.jsonl) holds pending decisions, exclusions and foreign queues. [Verification](verification.json) records live checks. Public page/asset caches are outside the repository in `/tmp/aura-dealer-discovery/NL/`.

Native-domain observations remain in `dealers.jsonl`, with their component readback in [verification-root.json](verification-root.json). Merchant-specific hosted catalogues have separate `hosted/<merchant>/dealers.jsonl` and `verification.json` files, including [Depot J&J](hosted/depot-j-j/dealers.jsonl), [Kunstkabinet](hosted/kunstkabinet/dealers.jsonl), [Bries en Bloei](hosted/bries-en-bloei/dealers.jsonl) and [De Verzamelkramer](hosted/verzamelkramer/dealers.jsonl); the shared host is never a claimed marketplace operator or alias. Deduplicate by exact catalogue scope, stable merchant account, operator and contacts across every discovery ledger and the API. The country verification aggregates every owned component, requiring matching runs, current observation digests, verified resource IDs and unique sources. The country index preserves its first eight columns and adds catalogue URL, ledger path and source scope. Verify each explicit component path before aggregation; periodic full checks cover every component. [Depot J&J](hosted/depot-j-j/dealers.jsonl) uses account21765202, including equivalent public profile URL forms; an unavailable old website is not its source.

| Run | Sources added | Validation |
| --- | ---: | --- |
| 2026-11-03-nl-392 | 0 | Art Deco Webwinkel deferred pending proof of current genuine antique/vintage stock; Antiek Stijl Webwinkel excluded because its first-party site says its goods are newly made reproductions |
| 2026-11-03-nl-393 | 0 | Incremental native-ledger API readback; updated exclusion/defer counts after the NL candidate review |
| 2026-11-03-nl-394 | 0 | Aggregate country index and verification refreshed across all five NL-owned components; hosted component files were not changed |
| 2026-11-03-nl-395 | 0 | Nogges Online deferred: current first-party catalogue is general kringloop stock, not verified as an antique-dealer source |
| 2026-11-03-nl-396 | 0 | Incremental native-ledger API readback; updated deferred count after the NL candidate review |
| 2026-11-03-nl-397 | 0 | Aggregate country index and verification refreshed across all five NL-owned components; hosted component files were not changed |
| 2026-11-03-nl-398 | 1 | Antiques Forever added with WooCommerce, verified KvK/contact identity, correspondence location and original logo; DPD shipping terms retained but profile import rejected twice with no profile/default created |
| 2026-11-03-nl-399 | 0 | Incremental native-ledger readback; all 453 sources verified with empty ingestion |
| 2026-11-03-nl-400 | 0 | Aggregate country index and verification refreshed across all five NL-owned components; hosted component files were not changed |
| 2026-11-03-nl-401 | 1 | Camera Obscura Elburg added with verified KVK/contact identity, original 1936 Leica in-stock offer, appointment showroom, WooCommerce and inspected original logo; shipping rate branches retained but not generalized |
| 2026-11-03-nl-402 | 0 | Incremental native-ledger readback; all 454 sources verified with empty ingestion |
| 2026-11-03-nl-403 | 0 | Aggregate country index and verification refreshed across all five NL-owned components; hosted component files were not changed |
| 2026-11-03-nl-404 | 1 | Mammoet Oude Bouwmaterialen BV added after current catalogue, consumer terms, contacts and showroom identity were verified; WooCommerce and original logo recorded; unresolved local delivery left without a profile |
| 2026-11-03-nl-405 | 0 | Incremental native-ledger readback; all 455 sources verified with empty ingestion |
| 2026-11-03-nl-406 | 0 | Aggregate country index and verification refreshed across all five NL-owned components; hosted component files were not changed |
| 2026-11-03-nl-407 | 1 | l’Argenterie Antique added after KvK/contact/location and current priced 18th–19th-century silver offers were verified; MMBeheer CMS and original GIF wordmark recorded; shipping left unasserted |
| 2026-11-03-nl-408 | 0 | Incremental native-ledger readback; all 456 sources verified with empty ingestion |
| 2026-11-03-nl-409 | 0 | Aggregate country index and verification refreshed across all five NL-owned components; hosted component files were not changed |
| 2026-11-01-nl-001 | 3 | Full readback |
| 2026-11-01-nl-002 | 4 | Incremental readback |
| 2026-11-01-nl-003 | 3 | Incremental readback |
| 2026-11-01-nl-004 | 3 | Incremental readback |
| 2026-11-01-nl-005 | 4 | Full readback of all 17 managed dealers |
| 2026-11-01-nl-006 | 2 | Incremental readback of all managed metadata and new resources |
| 2026-11-01-nl-007 | 3 | Incremental readback |
| 2026-11-01-nl-008 | 3 | Incremental readback |
| 2026-11-01-nl-009 | 4 | Incremental readback |
| 2026-11-01-nl-010 | 4 | Full readback of all 33 managed dealers |
| 2026-11-01-nl-011 | 4 | Incremental readback; Koster has no verified delivery profile |
| 2026-11-01-nl-012 | 3 | Incremental readback; Angevaren deferred |
| 2026-11-01-nl-013 | 2 | Incremental readback; two stock-verification deferrals |
| 2026-11-01-nl-014 | 2 | Incremental readback; domain/access deferrals |
| 2026-11-01-nl-015 | 4 | Full readback of all 48 managed dealers |
| 2026-11-01-nl-016 | 3 | Incremental readback |
| 2026-11-01-nl-017 | 4 | Incremental readback |
| 2026-11-01-nl-018 | 2 | Incremental readback |
| 2026-11-01-nl-019 | 2 | Incremental readback |
| 2026-11-01-nl-020 | 3 | Full readback |
| 2026-11-01-nl-021 | 3 | Incremental readback |
| 2026-11-01-nl-022 | 1 | Incremental readback |
| 2026-11-01-nl-023 | 2 | Incremental readback |
| 2026-11-01-nl-024 | 2 | Incremental readback |
| 2026-11-01-nl-025 | 1 | Full readback |
| 2026-11-01-nl-026 | 1 | Incremental readback |
| 2026-11-01-nl-027 | 3 | Incremental readback |
| 2026-11-01-nl-028 | 3 | Incremental readback |
| 2026-11-01-nl-029 | 3 | Incremental readback |
| 2026-11-01-nl-030 | 2 | Full readback |
| 2026-11-01-nl-031 | 3 | Incremental readback |
| 2026-11-02-nl-032 | 3 | Incremental readback |
| 2026-11-02-nl-033 | 2 | Incremental readback |
| 2026-11-02-nl-034 | 2 | Incremental readback |
| 2026-11-02-nl-035 | 4 | Full readback |
| 2026-11-02-nl-036 | 3 | Incremental readback |
| 2026-11-02-nl-037 | 3 | Incremental readback |
| 2026-11-02-nl-038 | 3 | Incremental readback |
| 2026-11-02-nl-039 | 3 | Incremental readback |
| 2026-11-02-nl-040 | 2 | Full readback |
| 2026-11-02-nl-041 | 3 | Incremental readback |
| 2026-11-02-nl-042 | 0 | Research decisions; no new resources |
| 2026-11-02-nl-043 | 1 | Incremental readback |
| 2026-11-02-nl-044 | 2 | Incremental readback |
| 2026-11-02-nl-045 | 2 | Full readback |
| 2026-11-02-nl-046 | 0 | Research decisions; no new resources |
| 2026-11-02-nl-047 | 3 | Incremental readback |
| 2026-11-02-nl-048 | 2 | Incremental readback |
| 2026-11-02-nl-049 | 2 | Incremental readback |
| 2026-11-02-nl-050 | 1 | Full readback |
| 2026-11-02-nl-051 | 1 | Incremental readback |
| 2026-11-02-nl-052 | 2 | Incremental readback |
| 2026-11-02-nl-053 | 4 | Incremental readback |
| 2026-11-02-nl-054 | 2 | Incremental readback |
| 2026-11-02-nl-055 | 3 | Full readback |
| 2026-11-02-nl-056 | 2 | Incremental readback |
| 2026-11-02-nl-057 | 2 | Incremental readback |
| 2026-11-02-nl-058 | 2 | Incremental readback |
| 2026-11-02-nl-059 | 1 | Incremental readback |
| 2026-11-02-nl-060 | 2 | Full readback |
| 2026-11-02-nl-061 | 3 | Incremental readback |
| 2026-11-02-nl-062 | 3 | Incremental readback |
| 2026-11-02-nl-063 | 1 | Incremental readback |
| 2026-11-02-nl-064 | 1 | Incremental readback |
| 2026-11-02-nl-065 | 2 | Full readback |
| 2026-11-02-nl-066 | 4 | Incremental readback |
| 2026-11-02-nl-067 | 2 | Incremental readback |
| 2026-11-02-nl-068 | 2 | Incremental readback |
| 2026-11-02-nl-069 | 3 | Incremental readback |
| 2026-11-02-nl-070 | 1 | Full readback |
| 2026-11-02-nl-071 | 3 | Incremental readback |
| 2026-11-02-nl-072 | 0 | Incremental readback |
| 2026-11-02-nl-073 | 3 | Incremental readback |
| 2026-11-02-nl-074 | 2 | Incremental readback |
| 2026-11-02-nl-075 | 3 | Full readback |
| 2026-11-02-nl-076 | 2 | Incremental readback |
| 2026-11-02-nl-077 | 1 | Incremental readback |
| 2026-11-02-nl-078 | 1 | Incremental readback |
| 2026-11-02-nl-079 | 1 | Incremental readback |
| 2026-11-02-nl-080 | 2 | Full readback |
| 2026-11-02-nl-081 | 2 | Incremental readback |
| 2026-11-02-nl-082 | 1 | Incremental readback |
| 2026-11-02-nl-083 | 3 | Incremental readback |
| 2026-11-02-nl-084 | 0 | Incremental readback |
| 2026-11-02-nl-085 | 1 | Full readback |
| 2026-11-02-nl-086 | 2 | Incremental readback |
| 2026-11-02-nl-087 | 1 | Incremental readback |
| 2026-11-02-nl-088 | 1 | Incremental readback |
| 2026-11-02-nl-089 | 3 | Incremental readback |
| 2026-11-02-nl-090 | 3 | Full readback |
| 2026-11-02-nl-091 | 2 | Incremental readback |
| 2026-11-02-nl-092 | 1 | Incremental readback |
| 2026-11-02-nl-093 | 2 | Incremental readback |
| 2026-11-02-nl-094 | 2 | Incremental readback |
| 2026-11-02-nl-095 | 4 | Full readback |
| 2026-11-02-nl-096 | 4 | Incremental readback |
| 2026-11-02-nl-097 | 1 | Incremental readback |
| 2026-11-02-nl-098 | 1 | Incremental readback |
| 2026-11-02-nl-099 | 1 | Incremental readback |
| 2026-11-02-nl-100 | 1 | Full readback |
| 2026-11-02-nl-101 | 1 | Incremental readback |
| 2026-11-02-nl-102 | 2 | Incremental readback |
| 2026-11-02-nl-103 | 1 | Incremental readback |
| 2026-11-02-nl-104 | 1 | Incremental readback |
| 2026-11-02-nl-105 | 3 | Full readback |
| 2026-11-02-nl-106 | 3 | Incremental readback |
| 2026-11-02-nl-107 | 2 | Incremental readback |
| 2026-11-02-nl-108 | 1 | Incremental readback |
| 2026-11-02-nl-109 | 3 | Incremental readback |
| 2026-11-02-nl-110 | 2 | Full readback |
| 2026-11-02-nl-111 | 2 | Incremental readback |
| 2026-11-02-nl-112 | 2 | Incremental readback |
| 2026-11-02-nl-113 | 2 | Incremental readback |
| 2026-11-02-nl-114 | 1 | Incremental readback |
| 2026-11-02-nl-115 | 1 | Full readback |
| 2026-11-02-nl-116 | 1 | Incremental readback |
| 2026-11-02-nl-117 | 2 | Incremental readback |
| 2026-11-02-nl-118 | 1 | Incremental readback |
| 2026-11-02-nl-119 | 1 | Incremental readback |
| 2026-11-02-nl-120 | 1 | Full readback |
| 2026-11-02-nl-121 | 2 | Incremental readback |
| 2026-11-02-nl-122 | 0 | Incremental readback |
| 2026-11-02-nl-123 | 1 | Incremental readback |
| 2026-11-02-nl-124 | 3 | Incremental readback |
| 2026-11-02-nl-125 | 1 | Full readback |
| 2026-11-02-nl-126 | 1 | Incremental readback |
| 2026-11-02-nl-127 | 3 | Incremental readback |
| 2026-11-02-nl-128 | 3 | Incremental readback |
| 2026-11-02-nl-129 | 2 | Incremental readback |
| 2026-11-02-nl-130 | 1 | Full readback |
| 2026-11-02-nl-131 | 0 | Incremental readback |
| 2026-11-02-nl-132 | 1 | Incremental readback |
| 2026-11-02-nl-133 | 3 | Incremental readback |
| 2026-11-02-nl-134 | 1 | Incremental readback |
| 2026-11-02-nl-135 | 0 | Full resource readback; incremental metadata refresh |
| 2026-11-02-nl-136 | 2 | Incremental readback |
| 2026-11-02-nl-137 | 1 | Incremental readback |
| 2026-11-02-nl-138 | 1 | Incremental readback |
| 2026-11-02-nl-139 | 0 | Incremental readback |
| 2026-11-02-nl-140 | 1 | Full readback |
| 2026-11-02-nl-141 | 1 | Incremental readback |
| 2026-11-02-nl-142 | 2 | Incremental readback |
| 2026-11-02-nl-143 | 2 | Incremental readback |
| 2026-11-02-nl-144 | 0 | Incremental readback |
| 2026-11-02-nl-146 | 1 | Incremental readback |
| 2026-11-02-nl-147 | 1 | Incremental readback |
| 2026-11-02-nl-148 | 1 | Incremental readback |
| 2026-11-02-nl-149 | 1 | Incremental readback |
| 2026-11-02-nl-151 | 1 | Incremental readback |
| 2026-11-02-nl-152 | 1 | Incremental readback |
| 2026-11-02-nl-153 | 1 | Incremental readback |
| 2026-11-02-nl-155 | 1 | Full readback |
| 2026-11-02-nl-156 | 1 | Incremental readback; De Colibri added, destination-unresolved PostNL terms |
| 2026-11-02-nl-157 | 1 | Incremental readback; Van der Veer Antiek & Decoratie added, shipment geography unresolved |
| 2026-11-02-nl-158 | 1 | Incremental readback; Ronald Antiek added, FineVintage deferred pending available-stock evidence |
| 2026-11-02-nl-159 | 1 | Incremental readback; Meels Antiques added, Van de Moosdijk deferred pending inspectable stock |
| 2026-11-02-nl-161 | 2 | Incremental readback; Timeless Interiors and De Twee Linden Antiques added |
| 2026-11-02-nl-162 | 2 | Incremental readback of native and hosted components; Burbri and Het Wagenwiel Antiek added, two current-stock leads deferred |
| 2026-11-02-nl-163 | 2 | Incremental readback of native and hosted components; De Spot and JUKI AMSTERDAM added; marketplace excluded and two leads deferred |
| 2026-11-02-nl-164 | 2 | Incremental readback of native and hosted components; Zeger van Olden and Vintage Venue added; legal-identity conflict deferred |
| 2026-11-02-nl-165 | 2 | Full readback of all native and hosted components; OKER and ÅTMØS added; exact free-rate import for ÅTMØS remains unresolved |
| 2026-11-02-nl-166 | 2 | Full readback of all native and hosted components; Antiek Arcade and distinct Antiek Brocante added; two leads deferred |
| 2026-11-02-nl-167 | 1 | Full readback of all native and hosted components; Toast’s Vintage Finds added; four leads deferred |
| 2026-11-02-nl-168 | 4 | Full readback of all native and hosted components; four dealers added, Smeerling domain aliased, two leads deferred |
| 2026-11-02-nl-169 | 1 | Full readback of all native and hosted components; LOUI added |
| 2026-11-02-nl-170 | 1 | Full readback of all native and hosted components; Old BASICS added |
| 2026-11-02-nl-171 | 1 | Full readback of all native and hosted components; Van Trommelen added, VAT conflict documented |
| 2026-11-02-nl-172 | 1 | Full readback of all native and hosted components; Briks Brocante added after first-party indexed research |
| 2026-11-02-nl-173 | 1 | Full readback of all native and hosted components; TILT vintage design added after first-party review |
| 2026-11-02-nl-174 | 1 | Full readback of all native and hosted components; Vintage.nl by Misceaup added; Wieskes Brocante deferred over contradictory identity/contact details |
| 2026-11-02-nl-175 | 1 | Full readback of all native and hosted components; Vormfocus added with Mijnwebwinkel and NL/BE delivery terms |
| 2026-11-02-nl-176 | 1 | Full readback of all native and hosted components; Renetro added with Shopify and appointment collection |
| 2026-11-02-nl-177 | 3 | Full readback of all native and hosted components; Heikens Habitat, SMUK collectables and Van Roman Antiques added |
| 2026-11-02-nl-178 | 1 | Incremental readback; Next Door Vintage added with EU weight-banded terms, Vivre deferred pending an inspectable online catalogue |
| 2026-11-02-nl-179 | 1 | Incremental readback; Antiquariaat Clemens Hondtong added with WooCommerce and worldwide weight-banded terms; three leads deferred pending current catalogue or auction-scope verification |
| 2026-11-02-nl-180 | 1 | Incremental readback; Scrinium Classical Antiquity added with a direct antiquarian catalogue; MC Rare Books and Rashi deferred pending current activity and consistent first-party identity evidence |
| 2026-11-02-nl-181 | 1 | Incremental readback; Van Eyck Antiques added with verified operator, Maastricht premises, original logo and shipping terms; ingestion remains empty |
| 2026-11-02-nl-182 | 1 | Incremental readback; Antiquariaat Matthys de Jongh added with current first-party list, Zutphen premises and incorporated worldwide book-delivery terms; ingestion remains empty |
| 2026-11-02-nl-183 | 1 | Incremental readback; Het Kompas added with current own-stock catalogue, WooCommerce, original logo and Wolvega premises; legal identity and shipping unasserted |
| 2026-11-02-nl-184 | 1 | Incremental readback; Lutterzijl Antiek added with Simon van Dijk, own priced antique catalogue and Genemuiden premises; furniture delivery observed but territory/rate unresolved |
| 2026-11-02-nl-185 | 1 | Full readback of native and hosted components; Kroonluchter Atelier de Rode Hoeve added with registered trade name, WordPress fingerprint, original logo and Oldebroek showroom |
| 2026-11-02-nl-186 | 1 | Incremental readback; ’t Goudmijntje's online antique catalogue added with Mijnwebwinkel and a correspondence address; former storefront remains closed and shipping/legal-entity data unasserted |
| 2026-11-02-nl-188 | 1 | Incremental readback of native and hosted components; AntiekSite.nl added as De Dubbele Deur Antiek with MIJNWEBWINKEL and thresholded NL/BE delivery; legal form and original logo omitted |
| 2026-11-02-nl-189 | 1 | Incremental readback across native and hosted components; Vintage Lamps Amsterdam added with WooCommerce, Amstelveen seller geography and partial international shipping; Meester & Van der Bruggen deferred because current first-party offers are sold |
| 2026-11-02-nl-190 | 0 | Research checkpoint; In Reverse and Villa Pietha deferred pending an available qualifying item and inspectable offer/shipping details; Studio Eklektik confirmed already managed |
| 2026-11-02-nl-191 | 0 | Research checkpoint; Antiquaire and Antiquariaat Ovidius deferred pending inspectable current stock; Jack Nelis deferred because the only identified catalogue is a Trocadero marketplace storefront |
| 2026-11-02-nl-192 | 1 | Incremental readback across native and hosted components; Zegwaart & Leidelmeijer added with WooCommerce, city-only Heerhugowaard geography, original PNG wordmark and insured/tracked Dutch shipping plus appointment collection; Volanticus and C. Schouten deferred pending inspectable available stock |
| 2026-11-02-nl-193 | 1 | Incremental readback across native and hosted components; Versmissen Antiques and Decoration added under Versmissen Antiek B.V. with WooCommerce, Best showroom, original JPEG wordmark and partial container/LCL/airfreight terms |
| 2026-11-02-nl-194 | 1 | Incremental readback across native and hosted components; Tijdperk added with current vintage furniture catalogue, Tubbergen showroom and empty ingestion; legal form, platform, logo and shipping omitted where not verified; Eduard van Dishoeck deferred because its linked catalogue was inaccessible |
| 2026-11-02-nl-195 | 1 | Incremental readback across native and hosted components; Stock Brocante added with WordPress, original PNG wordmark, Huizen business premises and partial shipping plus appointment collection; Alkenaer and AntiekePrenten.nl deferred pending inspectable current stock |
| 2026-11-02-nl-196 | 1 | Incremental readback across native and hosted components; Leo’s Eyecatching Antiques added with Wix storefront, original high-resolution JPEG logo, city-level Gorssel geography and partial delivery/collection terms |
| 2026-11-02-nl-197 | 1 | Incremental readback across native and hosted components; Vintage Jewellery BV added with Shopify, original 1538×604 PNG logo, three Dutch boutique premises and registered Monnickendam address, insured/tracked worldwide shipping and named-country rates; Collector’s World deferred after its first-party domain began serving a reserved-domain notice |
| 2026-11-02-nl-198 | 1 | Incremental readback across native and hosted components; De Huiszwaluw Vintage Interiors added with WordPress, original JPEG wordmark, city-level Boven-Leeuwen premises and worldwide transport with destination/size/weight-based costs |
| 2026-11-02-nl-199 | 0 | Incremental readback across native and hosted components; Vintage Designs deferred because the current owner named in the first-party story conflicts with stale legal-operator terms |
| 2026-11-02-nl-200 | 1 | Incremental readback across native and hosted components; De Keizerin added with WordPress, original SVG logo, Heeze shop, appointment collection and partial item-specific shipping |
| 2026-11-02-nl-201 | 1 | Full readback across native and hosted components; Vintage Point V.O.F. added with JouwWeb storefront, original 1500×1500 JPEG logo, registered Zaandam address and partial Netherlands/international shipping; pickup location left unspecified |
| 2026-11-02-nl-202 | 2 | Incremental readback across native and hosted components; Koffie en Kansen and Vintage Troep added with JouwWeb storefronts, original logos, verified Hague/Amsterdam premises and advertised shipping; Tess Vintage deferred for missing operator/contact and shipping particulars |
| 2026-11-02-nl-203 | 1 | Incremental readback across native and hosted components; Deja Vintage V.O.F. added with Wix storefront, current Den Haag showroom and insured worldwide shipping; mismatched DIBS33 header asset omitted |
| 2026-11-02-nl-204 | 1 | Incremental readback across native and hosted components; Brevordia Antiek added with WooCommerce, original logo, Bredevoort premises and insured/tracked PostNL; conflicting international rate schedules retained without asserting rates. De Schatkamer and Appie Goossens deferred pending operator/fulfillment and Dutch-base verification |
| 2026-11-02-nl-205 | 0 | Incremental readback across native and hosted components; HomeRestyling deferred pending current legal-operator and advertised-fulfillment verification |
| 2026-11-02-nl-206 | 1 | Incremental readback across native and hosted components; Antiek-Zilver.nl added with Ole Klabou, Drupal metadata, original SVG, Haarlem correspondence address and empty ingestion. Registered-post terms retained with destination unresolved, so no shipping profile was set |
| 2026-11-02-nl-207 | 1 | Full readback across native and hosted components; Selected Objects added as a separate curated catalogue under Ole Klabou’s existing Party; location, logo, platform and shipping omitted where not verified |
| 2026-11-02-nl-209 | 1 | Incremental readback across native and hosted components; Hisstory Antiques added with WooCommerce, legal identifiers, Geertruidenberg locations and original wordmark. Worldwide insured PostNL terms retained; weight boundary prevents a faithful generic shipping profile |
| 2026-11-02-nl-210 | 0 | Incremental readback across native and hosted components; (S)old Antiques retained as deferred because its current first-party shop says the web store is under renewal and its offer page shows past assortment only |
| 2026-11-02-nl-211 | 1 | Incremental readback across native and hosted components; Snackers Antiques & Fine Art V.O.F. added with verified KvK/VAT identity, Maastricht gallery, original SVG logo and worldwide delivery coverage. Rate/carrier/insurance and platform remain unasserted; temporary PAN closure is due for activity recheck 2026-11-12 |
| 2026-11-02-nl-212 | 1 | Incremental readback across native and hosted components; Mazereeuw Antiquair added with WooCommerce, Amsterdam gallery, original SVG and worldwide insured shipping with free seller-order delivery above EUR 75. Horatii Antiques deferred for missing verified operator/contact and current orderable offer evidence |
| 2026-11-02-nl-213 | 0 | Research checkpoint; Umbria Antiques retained as deferred because its Amsterdam gallery has no inspectable current first-party stock catalogue; recheck if item-level offers return |
| 2026-11-02-nl-215 | 0 | Incremental live readback across native and hosted components; A.A. Knoop deferred because its current first-party site has no inspectable item-level offers |
| 2026-11-02-nl-216 | 0 | Incremental live readback across native and hosted components; FineVintage remains deferred after sampled current product pages still showed sold items despite “Nieuw” listings |
| 2026-11-02-nl-217 | 1 | Full live readback across native and hosted components; Golden Hour Vintage added with Shopify, inspected original PNG, appointment-only Drachten premises and domestic free shipping from EUR 175; lower-order rates retained in evidence because exclusive maximum thresholds cannot be represented faithfully |
| 2026-11-02-nl-218 | 1 | Incremental readback across native and hosted components; House Of Tourmaline added with Shopify, original PNG, Heemstede correspondence address and free insured/tracked delivery to the Netherlands and Belgium; broader rates retained without inferring country membership |
| 2026-11-02-nl-219 | 0 | Incremental live readback across native and hosted components; Anita Potters deferred after first-party contact and terms pages published conflicting VAT identifiers; no API resources created |
| 2026-11-02-nl-220 | 0 | Incremental readback across native and hosted components; Amsterdam Vintage Jewels deferred because the inspected first-party vintage category showed contemporary design offers rather than verifiable current vintage/antique stock |
| 2026-11-02-nl-221 | 1 | Full live readback across native and hosted components; House of Tudor added with WooCommerce, original SVG, Maastricht shop and empty ingestion; shipping terms were not published and remain unasserted |
| 2026-11-02-nl-222 | 1 | Incremental readback across native and hosted components; DroogVintage added with WooCommerce, original SVG, Gorinchem garage, partial Netherlands small-parcel terms and empty ingestion; Voor Dag en Dauw deferred pending verified legal-operator identity |
| 2026-11-02-nl-223 | 2 | Full live readback across native and all four hosted components; Sixty Fruits added with WooCommerce and unresolved fixed-transporter geography, plus De Verzamelkramer’s exact Marktplaats seller account with partial NL/BE courier coverage; empty ingestion retained |
| 2026-11-02-nl-224 | 1 | Incremental live readback across native and all four hosted components; Curiosa Cabinet B.V. added with Lightspeed, original PNG, separately recorded Amsterdam shop and registered address, partial worldwide delivery and empty ingestion |
| 2026-11-02-nl-225 | 1 | Incremental live readback across native and all four hosted components; Maps & Prints added under Bartele Gallery with custom commerce metadata, Langweer gallery, original antique catalogue and complete worldwide standard/express shipping tariffs; no catalogue logo verified; empty ingestion retained; Schuurschatten deferred pending authoritative operator identity |
| 2026-11-02-nl-226 | 9 | Incremental live readback across native and all four hosted components; nine Bartele Gallery specialist catalogue domains added under the existing Party and Langweer dispatch location with custom commerce metadata and worldwide standard/express shipping; no original logos verified; empty ingestion retained; Antique Natural History deferred pending inspectable item-level offers |
| 2026-11-02-nl-227 | 0 | Candidate recheck confirmed wagenwielwebshop.nl was already managed under its verified existing Party/source; duplicate screening deferral was corrected, no API mutation; all components read back successfully |
| 2026-11-02-nl-228 | 1 | Incremental live readback across native and all four hosted components; Apollo Art & Antiques added with KvK/VAT identity, WooCommerce, inspected original PNG, correspondence address and worldwide insured dynamic shipping; empty ingestion retained |
| 2026-11-02-nl-229 | 1 | Incremental live readback across native and all four hosted components; Asher Rare Books added under A. Asher & Co. B.V. with custom commerce, Houten business address and Dutch postal shipment scope unresolved; no source logo identified; empty ingestion retained |
| 2026-11-02-nl-230 | 1 | Incremental live readback across native and all four hosted components; Ruud van der Velden Kunst added with WordPress metadata, inspected original PNG, Rotterdam business location and no asserted shipping terms; empty ingestion retained; Onyx Antiek deferred pending direct first-party retry after DNS failure; Antiek & Curiosa deferred pending legal identity and current offer reconciliation |
| 2026-11-03-nl-231 | 1 | Incremental live readback across native and all four hosted components; Bob Scholte Fine Art added with Wix metadata, inspected original PNG, Haarlem appointment gallery, insured free shipping within the Netherlands and appointment collection; foreign destinations unresolved; empty ingestion retained |
| 2026-11-03-nl-232 | 0 | Research checkpoint; Studio Eklektik remains managed with empty ingestion and receives an identity review for conflicting KvK records; Kunsthandel Jan Kooijman deferred because its primary site publishes conflicting postcodes and an incomplete KvK number |
| 2026-11-03-nl-233 | 1 | Full live readback across native and all four hosted components; Alta Design added with WooCommerce, inspected original PNG, Tilburg shop and empty ingestion; item/category-dependent shipping retained in the ledger without a source-wide delivery profile |
| 2026-11-03-nl-234 | 0 | Follow-up confirmed Bestwelhip’s live contact page calls the operator a B.V. while first-party terms for the same KvK still call it a sole proprietorship; kept deferred pending authoritative identity resolution |
| 2026-11-03-nl-235 | 1 | Incremental live readback across native and all four hosted components; Lefft Vintage added under Sparkling Business Innovation with Wix, original PNG, Houten shop and empty ingestion; item-dependent parcel, pickup and delivery terms retained without a source-wide profile |
| 2026-11-03-nl-236 | 1 | Incremental live readback across native and all four hosted components; Vintage Design Store added with WooCommerce, original 1400x1400 PNG, verified KvK/VAT, Breda showroom/pickup and empty ingestion; distance-, weight- and floor-dependent shipping terms retained without a source-wide profile |
| 2026-11-03-nl-237 | 0 | Ouwe Koeie kept deferred after current first-party pages confirmed a shared concept-store/inboedel scope without individually priced, attributable items |
| 2026-11-03-nl-238 | 1 | Full live readback across native and all four hosted components; CuriosaenKunst.nl added with WooCommerce, original PNG, KvK/contact and Valkenburg address; shipping availability is advertised but destination/rate terms remain unpublished, so no delivery profile was asserted |
| 2026-11-03-nl-240 | 1 | Incremental live readback; De Toonkamer added with WooCommerce, its first-party priced antique catalogue, Amersfoort appointment shop, partial NL/BE shipping and empty ingestion; legal registration and original logo omitted where not verified |
| 2026-11-03-nl-241 | 0 | Vlist Brocante and Fifties Store deferred pending inspectable current item offers; Fifties Store also has conflicting first-party street numbers. No API resources created |
| 2026-11-03-nl-242 | 1 | Incremental live readback; Schipperswinkel’s dedicated antique catalogue added under Noordkaper Verhuur with WooCommerce, original PNG, KvK/VAT, Assen correspondence address, NL/BE/DE shipping and empty ingestion |
| 2026-11-03-nl-243 | 0 | Antique Prints Amsterdam deferred: no inspectable item-level current offers or verified legal operator; API preflight found no duplicate (1,805 Parties, 1,827 sources); no API writes |
| 2026-11-03-nl-244 | 1 | Incremental live readback; Cedante Interieur added with WooCommerce, original PNG, KvK/VAT/contact details and Den Haag premises; shipping policy conflicts remain unnormalized; ingestion empty; Prinsheerlijk Antiek was matched to its existing managed source |
| 2026-11-03-nl-245 | 0 | Incremental live readback; Wildschut Antiques’ closed Jordaan storefront and continued online/by-appointment operation recorded; active priced catalogue observed; no address/default or API change inferred |
| 2026-11-03-nl-246 | 0 | ZGAO’s first-party pages remain a JavaScript shell to the available public fetcher; search-indexed product modules are placeholders, so no offer facts were persisted and the existing deferral remains |
| 2026-11-03-nl-247 | 1 | Incremental live readback; Antiekhuis Nick Toma added with priced antique catalogue, CUSTOM_COMMERCE, inspected original PNG, Heerlen shop/contact and empty ingestion; shipping terms unpublished |
| 2026-11-03-nl-248 | 0 | Rechecked Vintage Masters primary legal pages; VAT/address conflict persists. Deferred geWoongoed bij Karin pending current stock/activity and verified operator. No API writes. |
| 2026-11-03-nl-249 | 0 | Deferred Prentenwinkel pending confirmation that disabled catalogue items remain available in its Haarlem shop; original historical stock, contact and shop hours recorded. No API writes. |
| 2026-11-03-nl-250 | 1 | Full live readback across native and all four hosted components; Kuipers Kunst & Antiek added with inspected original SVG and Delden showroom; platform technology and shipping remain unknown; ingestion empty. |
| 2026-11-03-nl-251 | 1 | Incremental native plus full hosted-component readback; Becker Antiques added with inspected original PNG, WordPress technology and Amsterdam gallery; shipping unknown; ingestion empty. |
| 2026-11-03-nl-252 | 0 | Research decisions; no new resources |
| 2026-11-03-nl-253 | 1 | Incremental native plus retained full hosted-component readbacks; Jacob J. Roosjen SRI added with inspected original PNG, WordPress metadata and Breukelen appointment location; sole proprietorship and domestic shipping/pickup terms verified after the initial onboarding; no international coverage or fixed rate asserted; ingestion empty. Run252 deferred Pine & Classics because its public category/new-arrival pages exposed no inspectable current offers. |
| 2026-11-03-nl-254 | 0 | Incremental native and retained hosted readbacks; Bastings Antiquairs deferred because its catalogue warns many displayed items are sold without item-level current availability; no API writes. |
| 2026-11-03-nl-255 | 1 | Incremental native plus retained hosted readbacks; Rocks and Clocks V.O.F. added with Hague shop/collection location, KvK/VAT identity, inspected original JPG logo and custom commerce metadata; shipping terms unpublished, ingestion empty. |
| 2026-11-03-nl-256 | 1 | Incremental readback |
| 2026-11-03-nl-257 | 1 | Incremental readback |
| 2026-11-03-nl-258 | 1 | Incremental native plus retained hosted readbacks; VSKM collections added with Shoppagina fixed-price catalogue, Putten visit/correspondence address, published KvK/VAT identifiers, partial PostNL terms with unresolved destination, no verified original logo, and empty ingestion. |
| 2026-11-03-nl-259 | 1 | Incremental native plus retained hosted readbacks; Amsterdam Curated added with Shopify antique-painting catalogue, published KvK/contact address, partially represented free international shipping, no verified original logo or legal entity form, and empty ingestion. |
| 2026-11-03-nl-260 | 1 | Full live readback across native and all four hosted components; Haffmans Antiek excluded after its first-party About page disclosed frequent eBay auction sales. Its unused source was removed and its API location retired; Party retained by API location-history protection. |
| 2026-11-03-nl-261 | 1 | Incremental live readback; The Fox and some Vintage added with WooCommerce, current circa-1920 own-stock antiques, published KVK/VAT and contact data, separate Haarlem shop/business addresses, and partial NL/BE destination evidence. Item-dependent delivery eligibility/rates and original logo remain unasserted; ingestion empty. |
| 2026-11-03-nl-262 | 1 | Incremental live readback; Wilstra Books added with WooCommerce, inspected original PNG wordmark, in-stock 1713–1817 antiquarian books, KvK/VAT and Nieuwe-Tonge visit address. Shipping costs are published but destinations are not, so coverage stays unresolved; ingestion empty. |
| 2026-11-03-nl-263 | 0 | Excluded Special Vintage after first-party homepage review confirmed weekly Catawiki auction sales; direct retail catalogue does not remove the auction-scope exclusion. No API resources created. |
| 2026-11-03-nl-264 | 0 | Deferred Léon Paul Antiques: all five sampled current Shopify “new products” were marked sold out; recheck for available first-party stock on 2026-12-03. No API resources created. |
| 2026-11-03-nl-265 | 1 | Added Maartens Vintage Meubels with JouwWeb, verified seller/KvK/contact, Haren appointment location, current priced vintage stock and empty ingestion; shipping remains item-dependent and no original standalone logo was identified. Deferred Sjaalman because surfaced first-party offers remain dated 2016; recheck 2026-12-03. Incremental live readback passed. |
| 2026-11-03-nl-266 | 0 | Deferred Meubels met een verhaal because current item availability and seller identity/contact could not be established from its first-party site; recheck 2026-12-03. No API resources created. Incremental live readback passed. |
| 2026-11-03-nl-267 | 0 | Deferred L.J. Mennink and Jos Roest Antiek because item-level current offers could not be inspected; their legal/contact/geography or delivery details also remain incomplete. Recheck 2026-12-03. No API resources created; incremental readback passed. |
| 2026-11-03-nl-268 | 1 | Added Antiquariaat Anna with WooCommerce, inspected original PNG, published contact details and Driebergen-Rijsenburg shop location. Current priced antiquarian stock inspected. No published legal identifier or delivery policy; ingestion empty and no shipping profile asserted. Incremental live readback passed. |
| 2026-11-03-nl-269 | 1 | Added Matilda's Boek with custom ASP commerce, inspected original 183×200 GIF emblem, KVK/VAT and contact, Sas van Gent shop, and partial PostNL coverage for NL/BE plus unresolved outside destinations. General rates remain unknown; audiobook-specific rates were not generalized. Ingestion empty. Incremental live readback passed. |
| 2026-11-03-nl-270 | 1 | Added Vintage Hoarder with Ecwid, inspected original PNG, KVK/VAT and public contacts. Current in-stock vintage design offers verified. Conflicting Rotterdam/Haarlem office/showroom descriptions kept street geography unresolved; item-dependent NL/international delivery retained without a generic profile. Ingestion empty. Incremental live readback passed. |
| 2026-11-03-nl-271 | 1 | Added Løkie Furniture with Shopify, inspected original 439×237 PNG, KVK/contact, Rotterdam showroom and current purchasable 1960s furniture. Shipping destinations/rates preserved in the ledger; carrier and timing claims conflict, so no generic profile was asserted. Ingestion empty. Incremental live readback passed. |
| 2026-11-03-nl-272 | 0 | Deferred Utopiadesign after first-party review found no individually inspectable current offers on its own site; recorded public identity, Afferden location and shipping-at-buyer-risk terms for 2026-12-03 recheck. No API resources created. |
| 2026-11-03-nl-273 | 1 | Added Ztijl Design with WooCommerce, inspected original 1138×972 PNG, VOF/KVK/VAT, owner contacts, Berkel en Rodenrijs showroom and currently available original vintage furniture. Parcel, destination-city and large-item shipping terms retained without a broad profile. Empty ingestion; incremental live readback passed. |
| 2026-11-03-nl-274 | 0 | Excluded Last Century Design because its own site says sales currently occur only through Marktplaats and Catawiki, with ordering/payment unavailable on the site. Deferred Schoon Licht because no individually inspectable, priced current offers were visible; first-party contact, Den Haag shop and broad NL/Europe delivery claim recorded for 2026-12-03 review. No API writes. |
| 2026-11-03-nl-275 | 0 | Deferred Leen Helmink Antique Maps because first-party pages confirm California/Netherlands appointment presence but do not establish the operating country, Dutch legal operator/address or public contacts. Current priced map offers recorded; assign to a country ledger after the operator base is verified. |
| 2026-11-03-nl-276 | 3 | Incremental readback; three vintage audio dealers added, Vintage Kelder deferred for unresolved legal operator, and JensDo Records queued after a temporary HTTP 500 |
| 2026-11-03-nl-277 | 1 | Incremental readback; De Parel added with Shopify, inspected original logo, KVK, Rotterdam premises and partial source-wide shipping profile. Lamplord and KruijkAntiques deferred for identity follow-up |
| 2026-11-03-nl-278 | 1 | Incremental readback |
| 2026-11-03-nl-279 | 1 | Incremental readback |
| 2026-11-03-nl-280 | 1 | Incremental readback |
| 2026-11-03-nl-281 | 0 | Incremental readback |
| 2026-11-03-nl-282 | 2 | Incremental readback |
| 2026-11-03-nl-283 | 1 | Incremental readback |
| 2026-11-03-nl-284 | 1 | Incremental readback |
| 2026-11-03-nl-285 | 2 | Incremental readback |
| 2026-11-03-nl-286 | 1 | Incremental readback |
| 2026-11-03-nl-287 | 0 | Incremental readback |
| 2026-11-03-nl-288 | 1 | Incremental readback |
| 2026-11-03-nl-289 | 1 | Incremental readback |
| 2026-11-03-nl-290 | 1 | Incremental readback |
| 2026-11-03-nl-291 | 0 | Full readback |
| 2026-11-03-nl-292 | 1 | Full readback |
| 2026-11-03-nl-293 | 0 | Incremental readback |
| 2026-11-03-nl-294 | 0 | Incremental readback |
| 2026-11-03-nl-295 | 0 | Incremental readback |
| 2026-11-03-nl-296 | 0 | Incremental readback |
| 2026-11-03-nl-297 | 0 | Incremental readback |
| 2026-11-03-nl-298 | 1 | Incremental readback |
| 2026-11-03-nl-299 | 1 | Incremental readback |
| 2026-11-03-nl-300 | 0 | Full readback |
| 2026-11-03-nl-301 | 0 | Incremental readback |
| 2026-11-03-nl-302 | 1 | Incremental readback |
| 2026-11-03-nl-304 | 1 | Incremental readback |
| 2026-11-03-nl-305 | 1 | Full readback |
| 2026-11-03-nl-306 | 1 | Incremental readback |
| 2026-11-03-nl-307 | 1 | Incremental readback |
| 2026-11-03-nl-308 | 1 | Incremental readback |
| 2026-11-03-nl-309 | 0 | Research decisions; no new resources |
| 2026-11-03-nl-310 | 0 | Full readback |
| 2026-11-03-nl-311 | 1 | Incremental readback |
| 2026-11-03-nl-312 | 1 | Incremental readback |

Run 278 added the direct Het Kunstkabinet Google Sites assortment under the existing verified KvK 33235324 Party, with a read-back Beverwijk shop location default, verified empty ingestion and no unsupported logo or delivery profile. Next: continue deduplicated NL candidates, retry due deferrals and maintain country-only checkpoints while the ongoing goal remains active.

Run 279 added Negen Levens under De Vries & Heiliegers V.O.F. (KvK 87399873), with an inspected original logo, current Haarlem premises and email-arranged shipping left destination- and item-specific. Run 278 added the separate direct Het Kunstkabinet Google Sites assortment under its existing verified Party. Continue deduplicated NL candidates, retry due deferrals and maintain country-only checkpoints while the ongoing goal remains active.

Raechell, Vintage by Lexie, LauLau Treasures, De Oude Pelikaan and Clercq Militaria were onboarded and read back in runs 2026-11-03-nl-298, 2026-11-03-nl-299, 2026-11-03-nl-302, 2026-11-03-nl-304 and 2026-11-03-nl-305. Runs 2026-11-03-nl-300 and 2026-11-03-nl-305 completed full readbacks of all five NL-owned components. De wereld van Kant was added in run 2026-11-03-nl-306 with weight-dependent charges and the seller’s “most sale products” shipping caveat preserved. Lichtkringen was added in run 2026-11-03-nl-307 with the published 21-country package tariffs retained and service eligibility left unresolved because package size and weight cannot be mapped to listing classes. Twentrain was added in run 2026-11-03-nl-308 with the original logo, owner/KvK/VAT/contact facts, used model-train catalogue and NL/EU threshold shipping. ModelspoorWebshop and KallisToys were excluded as auction-house/auction-marketplace leads; Veluwetoys is deferred until it publishes current stock. The Robin and Monkeystyle remain deferred pending current-stock checks; The Robin also needs its return statements reconciled. Run 2026-11-03-nl-309 deferred Antiquariaat Van Hoorn pending domain/operator reconciliation and Klaas heeft Alles pending evidence of genuine vintage stock. Run 2026-11-03-nl-310 completed full readback of all five NL-owned components. Vintage Studio was added in run 2026-11-03-nl-311 with its original logo, current catalog, named operator/contact/KvK/VAT, correspondence address and UPS quote terms; a conflicting embedded map location remains unresolved and was not classified as premises or stock. Focus on Film was added in run 2026-11-03-nl-312 with WooCommerce, an inspected original logo, published KVK/VAT/contact details, correspondence-only postal address and PostNL dispatch terms whose destinations remain unstated. The Analog Camera Store, Analog Club Amsterdam and De Cameraman were deferred pending current-offer or operator/catalog verification. Run 2026-11-03-nl-313 added OLD Beauties with its JouwWeb catalog, original logo, sole-proprietor KVK/VAT/contact facts, appointment business address and partial NL shipping terms; the distinct storage pickup address and item classes remain unresolved. Bruens Juwelier & Antiquair was excluded on official final AML compliance enforcement findings, without a broader fraud or closure claim. Juweelwinkel was also added in this run. Incremental API readback passed for all 429 native sources; all ingestion configurations are empty. Run 2026-11-03-nl-314 deferred Marinus Licht because its first-party catalogue pages are dated 2018 and no current individual offer or current own-stock evidence was inspectable; this is an evidence gap rather than an inactivity finding. Run 2026-11-03-nl-315 completed a full native-ledger readback and added Oudekaarten.com as a separate Shopify map-specialist source under the already verified Prima Prints and Maps Party/location. The current catalog reports 59 in-stock maps; the observed 1858 map has available Shopify variant data and an enabled cart control. Its candidate header image was a map rather than a distinct logo and was omitted. Run 2026-11-03-nl-316 added Vintage Electronics under SDN engineering, with OpenCart, the original wordmark, KvK/contact/location facts, a current 1948 Philips gramophone restoration project and worldwide PostNL/DHL terms; rates remain checkout-dependent. Run 2026-11-03-nl-317 added Odd Stuff with JouwWeb, an inspected original wordmark, KVK/VAT/contact facts, a current 1985 toy collectible and item-scoped Dutch shipping terms. Its expired July dispatch notice was not treated as closure. Run 2026-11-03-nl-318 added Potma Edelmetaal after verifying the BV identity, WooCommerce catalogue, current in-stock watch, Den Haag premises and free worldwide DHL terms; Vintage Horloges was deferred because its current sales page had no products. Run319 deferred Vogelaar Vintage pending legal-operator and shipping evidence and Apollo Art Books pending legal identity and source-wide fulfillment scope. Run320 completed a full readback of all five NL-owned components; all 437 sources read back with empty ingestion. Current aggregate counts are 423 Parties, 437 sources, 460 locations, 310 logos, 369 identified platforms and 350 delivery profiles; 36 exclusions and 200 deferrals are recorded. Run 2026-11-03-nl-322 deferred old-roses.nl because its first-party pages returned HTTP 403, leaving operator, live stock and business facts unverifiable. All prior broad-search candidates in the current batch were deduplicated against country ledgers and the current API snapshot; no duplicate onboarding was performed. Continue fresh Dutch research after deduplicating every NL-owned ledger and current API state.

Enrichment gaps: **129 source logos** remain unverified, **68 platforms** remain unknown, and **87 sources lack a delivery profile/default**. Asset, identity and activity follow-ups are dated in the ledger. Run320 completed a full readback across all five Netherlands-owned components at 2026-11-03T04:01:32.051290Z; run321 completed an incremental readback at 2026-11-03T04:06:39.478034Z; run323 completed an incremental readback at 2026-11-03T04:11:22.240434Z; run324 completed an incremental readback at 2026-11-03T04:17:04.460623Z. Run322 recorded old-roses.nl as deferred after all inspected first-party pages returned HTTP 403; no business facts were persisted. The run322 production snapshot remains owner-only under /tmp, deduplication was checked against all country ledgers and that snapshot, and no API mutation was warranted. No pending API operations remain; uncertain shipping terms and candidate follow-ups are recorded for review. ÅTMØS advertises free delivery in NL. Its API profile records NL delivery availability at unknown rate and by-appointment collection; the exact free rate and item-class international tariffs remain in the ledger for importer reconciliation.

Lessons for continuation:

- Match legal operators, contacts and registration numbers. Klinkhamer Antiek (Stephan Veenstra, KvK54174589) is distinct from excluded Veilinghuis Klinkhamer (KvK55738788). Shared software/name is not identity.
- Wildschut’s closed Jordaan branch does not establish business closure. Its office is distinct from its unpublished warehouse; keep pickup unbound to the office. Kollenburg’s PO box is correspondence only.
- Source delivery must not override item restrictions. Schievink has explicitly nonshippable heavy stock, so its source profile retains collection only. Furniture/fragile exceptions block unassigned universal parcel prices or free thresholds. SennaBenna and Art & More have explicit complete destination limits independent of unknown prices.
- A historical auction reference, valuation service or negotiated offer is not by itself an auction operation. Art-Leap explicitly advertises weekly auctions in its current stock page and is excluded under the user’s scope.
- Follow literal published shop/frame links. Kramer’s old marketing domain embeds its Magento shop; matching legal identity/contacts establishes one source with an alias and three explicit premises. The old domain’s TLS mismatch was not bypassed. Empty Roemers stock did not mean the whole shop was empty: other named sale categories had real enquiry offers.
- Inspect decoded assets and actual availability state. Some JPG URLs contain PNGs; transparent brands need contrasting inspection backgrounds. Native file dimensions can be smaller than HTML dimensions. Hidden generic sold-state templates must be checked against actual product state and offers. Provider seals and photos are not dealer logos.
- Merchant-specific hosted catalogues can qualify an independent dealer while the marketplace remains excluded. Keep each catalogue in its own scoped ledger and aggregate every component into the country report. Compare stable merchant accounts across renamed and legacy profile URLs; never claim ownership of a shared host.
- A primary shop move overrides directory addresses and surviving footer fragments. De Brocante Dames continues online after its Lelystad shop closed; its Dronten p/a address is correspondence, and appointment collection stays unbound. An empty hosted catalogue at neiZ does not establish business closure.
- Shipping policies can disagree on dispatch weekdays or personal delivery while agreeing on parcel packing and handling. Keep the common verified assertions, preserve weight and pickup-only restrictions, and leave unresolved eligibility, prices and geography unknown.

- A site's “newest products” can still be sold: check each item’s detailed availability state. FineVintage’s four homepage-promoted newest items were all sold, so defer rather than onboarding from the homepage alone.
- Omit unsupported exact facts. Search snippets are leads; currency selectors are not delivery coverage; visits are not collection; legal/contact addresses are not stock or dispatch assertions. No coordinates or product-specific authenticity certifications were invented.

- Smeerling’s native public Next.js data contains real product records absent from the text preview. Its dated October holiday is not permanent closure. D’Antan furniture is explicitly collection-only, so its source profile does not promise parcel shipment. Aronson’s delivery terms offer a service but specify no destination countries; its profile preserves unresolved geography rather than inventing worldwide coverage.

- Free public page reading can expose real current offers when ordinary HTTP disconnects; Antique Warehouse’s currently offered bookcase and first-party owner/showroom were inspected this way. De Gaarde’s readable policies did not compensate for blocked stock. De Gele Etalage’s current contact phone overrides an about-page typo; its old legal PDF street was not imported as current premises.

- A generic theme SVG `icon-woocommerce` is not proof of a WooCommerce catalogue: B&M uses WordPress posts. Fragmenta Selecta exposes an actual VirtueMart catalogue despite a vendor-branded CMS generator. Unspecified shipping destinations remain unresolved; a worldwide customer base alone is not destination evidence.

- A disabled checkout can coexist with explicit contact-to-buy and invoice policies. HKvintage marks sold items separately and offers a non-sold1970s Brabantia set; its contact purchase flow was inspected. Small-item-only shipping was not generalized to furniture. Blauwe Roos per-parcel rates use shipment basis; undefined “other Europe” rates remain notes and the global fallback price stays unknown.

- An accessible template terms page with placeholders supplies no actual shipping commitment. Koster has verified identity/stock but no delivery profile. JustLoveAntiques has public trading identity/contacts and current stock; registration/legal form remain unknown because the privacy template is unfilled.

- Daatselaar’s published new-gallery move overrides the old directory street; retain business city and separate correspondence PO box. Its hidden Sold widget is controlled by an absent out-of-stock body class while the actual product is instock. Angevaren’s replacement website currently lacks inspectable current stock; an older shop-closure heading is not a permanent business closure.

- Antiekeprenten’s file named winkellogoDP is a botanical leaf print rather than a logo; visual inspection prevented importing stock art as branding. Its unrestricted order-value free-shipping thresholds are represented with country-specific fallbacks. For physical dealers, an explicit genuine historical retail range and inspectable original retail gallery can establish qualification without an online cart or individual SKU. An expressly past-stock gallery without current retail-range evidence, or a future-only shop notice, still needs follow-up; do not assert continued availability of each pictured piece.

- Renssen’s CSS-backed Artlogic logo is an actual300x200 WebP despite a PNG URL suffix; official `c_limit` did not upscale it. Its shipment policy explicitly includes crating, insurance and tracking. Contemporary original art and historical graphic art qualify under the skill; provider membership marks and artwork photos never substitute for merchant logos.

- An explicit primary auction-cessation notice can distinguish current retail from former auctions: Gelders Veilinghuis says it no longer auctions and directs sellers to Mabo. A. Gerits’s old updates were corroborated by current available merchant-only stock. Henk van der Kamp uses an HTML meta refresh to a real Zen Cart catalogue; dimension-limited postal tariffs were not made universal.

- Antiqbook merchant-only profiles can verify current stock without creating a marketplace Party/source. Fokas’s Paulbooks profile explicitly names the same business and contacts. Kok’s modern bookbags.nl imprint confirms its old NVVA-hosted catalogue identity. Generic provider marks and image collages were not used as logos; weight-limited postage/free thresholds remain scoped observations.

- A primary closure announcement overrides surviving old catalogues: Junk retired in February2026. The separately operated Dieter Schierenberg BV remains active; shared family history does not merge identities or closure decisions.

- Inspect the actual edition: Artemis’s 1915 Noguchi and Clio’s first Dutch Dahl1968 qualify, while Otia’s current undated/reprinted editions remain unverified. Leeuwarder Magazijn has current antique stock, but a current business location still needs first-party confirmation; no old address was reused.

- Image-only contact cards can carry valid published contacts. Bij Lente’s original card agrees with its currently linked privacy document. Its Koekange pickup is unbound to the correspondence street. Product categories can contain complete current offers even when a product route fails to render.

- Conflicting shipping tariffs warrant unknown prices: Art Nouveau Plateel’s worldwide packing/insurance is explicit, but its two rate schedules disagree. A malformed published mobile number was omitted without guessing a repair. Antiek & Chique’s Catawiki selling-channel reference remains an auction-eligibility deferral, not a fabricated closure or confirmed-auction exclusion.

- Rental-venue catalogues and independent tenants are different identities. Vintage2youstore explicitly rents dozens of seller spaces and is excluded; Jalabala and Chic Antiek identify their own stock and operator within shared venues. Chic/Kristalpaleis have identical registration details and reciprocal links, so they share one owner record.

- Resolve contradictory primary legal identifiers before onboarding. VintageMeesters has different registration/VAT numbers within its current pages; Vintage Masters has conflicting old terms and current footer. These are identity deferrals, not fraud or closure findings.

- Check actual availability, not sitemap inclusion. Three City Vintage item pages are explicitly out of stock. A credible shop, recent sitemap and original logo do not replace available historical-stock evidence.

- Source-wide worldwide marketing must respect NL-only items. Depot19 retains worldwide geography with eligibility unknown pending item scope, and only agreed Olst city precision where streets conflict. Brocantezussen explicitly separates online and Heusden stock and offers alternate Middelburg collection, so inventory and pickup-address assumptions remain absent.

- Check the rendered stock state behind theme badges: Shopify Dawn can include a hidden sold-out label on an available item. Require agreeing product availability data, price state and catalogue filters. A checkout maintenance notice that explicitly preserves email orders is not a closure notice.

| 2026-11-03-nl-318 | 1 | Incremental readback across native and hosted components; Potma Edelmetaal added, Vintage Horloges deferred pending current orderable stock evidence |

| 2026-11-03-nl-319 | 0 | Research decisions; Vogelaar Vintage and Apollo Art Books deferred over unresolved operator/fulfillment evidence |

| 2026-11-03-nl-320 | 0 | Full readback across all five native and hosted components |

| 2026-11-03-nl-321 | 1 | Incremental readback across native and hosted components; Antiek & Antiquiteiten added under Het Raaijmakers Antiek & Interieur B.V. |
| 2026-11-03-nl-322 | 0 | Deduplicated current API snapshot; old-roses.nl deferred after HTTP 403 and antiqueringshop.nl recorded as an alias of the existing Shopify source |
| 2026-11-03-nl-323 | 1 | Incremental API readback; De Veteraan added with ePages, current stock and destination-unknown DPD terms |
| 2026-11-03-nl-324 | 1 | Incremental API readback; Zilverwebsite.nl added with Shopify, verified logo and item-scoped shipping terms |

| 2026-11-03-nl-325 | 0 | Full live readback across all five NL-owned components; fresh research leads deduplicated against the Netherlands ledger and current API |

| 2026-11-03-nl-326 | 0 | Watt Design deferred: current sampled catalogue shows new Giso/Thabur reissues; no current original vintage stock verified |

| 2026-11-03-nl-327 | 0 | In’t Veld Geluid, Handelsonderneming Van Berkel and Audiozolder deferred pending itemized first-party current stock or offer evidence |

| 2026-11-03-nl-328 | 0 | Studio29Penbox and Modeltreinmarkt deferred pending current individual stock and pen/model vintage evidence |

| 2026-11-03-nl-329 | 0 | 2ehandssieraden.nl deferred under the existing Vintage Jewellery BV Party pending item-age evidence distinguishing antique/vintage stock from general secondhand jewellery |
| 2026-11-03-nl-330 | 1 | Incremental live readback across all five NL-owned components; Regts - Delft Tiles added with MAGENTO, appointment-only business premises and empty ingestion; scoped shipping terms and logo omission recorded |
| 2026-11-03-nl-331 | 0 | Incremental readback across all five NL-owned components; VintEdge Design deferred over unresolved responsible seller identity, while current 1994 stock and worldwide shipping were recorded |
| 2026-11-03-nl-332 | 1 | Incremental readback across all five NL-owned components; VAN ONS re-reviewed and added after recent itemized vintage stock, first-party KvK/contact/address; empty ingestion; shipping and logo left unasserted |
| 2026-11-03-nl-333 | 1 | Incremental readback across all five NL-owned components; Kevelam added with NOPCOMMERCE, office location and empty ingestion; Dutch Coin Legacy excluded for current own-stock auction sales; Antiekhuys deferred over uncertain item age/authenticity and seller identity |
| 2026-11-03-nl-334 | 1 | Incremental readback across all five NL-owned components; Insulinde added with MYSHOP, inspected original logo and Netherlands shipping profile; lower-order rate remains unknown due conflicting first-party pages; Theo Peters excluded for operating AA Muntenveiling; De Gems deferred pending qualifying first-party stock |
| 2026-11-03-nl-335 | 1 | Full native-ledger readback; Goldberg Juweliers added with LIGHTSPEED, original logo and location; API rejected shipping-profile creation, so terms and rejection are retained in the ledger and no profile/default was asserted |
| 2026-11-03-nl-336 | 1 | Included in the full native-ledger readback; Lyppens Juweliers added with SHOPIFY and inspected original logo; no shipping profile because published shipping terms lack coverage and rates |
| 2026-11-03-nl-337 | 0 | Country index and aggregate checkpoint refreshed from the native full readback and existing hosted-component reports; other component files were not changed |
| 2026-11-03-nl-338 | 0 | Boekennietje deferred: active bookseller but own-site itemized catalogue unavailable; sales route through third-party platforms |
| 2026-11-03-nl-339 | 1 | Toy Paradise added with WOOCOMMERCE, city-level Hoofddorp location and empty ingestion; first-party 2016 shipping terms retained, but API profile creation returned HTTP 400 and fresh readback confirmed no profile |
| 2026-11-03-nl-340 | 0 | Full native-ledger readback: 443 sources verified, all ingestion methods empty |
| 2026-11-03-nl-341 | 0 | Aggregate country index refreshed from native full readback and existing hosted-component reports; other component files were not changed |
| 2026-11-03-nl-342 | 0 | Decades Voyage deferred because current itemized offers and legal seller identity were not sufficiently verifiable; Antiekepop deferred for lack of current own-stock evidence; Het Berenstoepke excluded as repair supplies only |
| 2026-11-03-nl-343 | 0 | Country index and aggregate refreshed from the native full readback and existing hosted-component reports; other component files were not changed |
| 2026-11-03-nl-344 | 1 | Haarlemsche Zilversmederij added with WOOCOMMERCE, KHS brand header/logo and Haarlem workshop location; direct in-stock 1774 silver offer; no shipping scope asserted |
| 2026-11-03-nl-345 | 1 | Balanced Watches added with WOOCOMMERCE, inspected original logo and Buitenpost office; current 1970 vintage watch listing; delivery coverage/rates not published |
| 2026-11-03-nl-346 | 0 | Full native-ledger readback: 445 sources verified with empty ingestion |
| 2026-11-03-nl-347 | 0 | Vintage Times deferred for no current in-stock offer; La Grange Antiques and WatchMeisters deferred while their public sites/catalogues remain unavailable or under redesign |
| 2026-11-03-nl-348 | 0 | Country index and aggregate refreshed from the native full readback and existing hosted-component reports; other component files were not changed |
| 2026-11-03-nl-350 | 1 | Ca-Art added after full ledger/API dedup; verified WooCommerce, legal registration, contact, location and original logo; shipping unasserted |
| 2026-11-03-nl-352 | 0 | PostcardMax deferred: active postcard dealer, but reviewed site routes sales through marketplaces/fairs without an itemized own-stock catalogue |
| 2026-11-03-nl-354 | 1 | MHS Militaria added with Ecwid, Groningen base, original logo, current militaria stock and worldwide insured shipping; unknown rates retained |
| 2026-11-03-nl-357 | 0 | JVL Militaria deferred pending verifiable public operator identity, contact and business location |
| 2026-11-03-nl-358 | 1 | AVK Militaria added with Concept500, proprietor and registration, Nieuwegein base and worldwide insured-export terms; rates and logo omitted |
| 2026-11-03-nl-361 | 0 | Westland Militaria deferred pending first-party legal operator/address evidence and distinction from Westland London |
| 2026-11-03-nl-362 | 0 | Full native-ledger readback: 448 sources verified with empty ingestion |
| 2026-11-03-nl-363 | 0 | Aggregate country index and verification refreshed across all five NL-owned components; hosted component files were not changed |
| 2026-11-03-nl-365 | 1 | David-Coin added with WooCommerce, verified legal registration and contacts, Dordrecht location, and first-party weight-based/international insured shipping terms; no logo asserted |
| 2026-11-03-nl-368 | 0 | PMH Enibas deferred pending verified current qualifying historical stock; active business status retained |
| 2026-11-03-nl-369 | 0 | Brocanteurs deferred pending first-party operator, direct contact, current business location and shipping/collection details |
| 2026-11-03-nl-370 | 0 | Sanderus Antiquariaat queued in the NL frontier for Belgium; no foreign ledger or API record changed |
| 2026-11-03-nl-371 | 0 | Full native-ledger readback: 449 sources verified with empty ingestion |
| 2026-11-03-nl-374 | 1 | Munthandel Omlandia added with Wix, Groningen city-level location, first-party direct contacts and registration, and advertised PostNL/free-tracked terms; coverage remains unresolved and no logo asserted |
| 2026-11-03-nl-375 | 0 | Full native-ledger readback: 450 sources verified with empty ingestion |
| 2026-11-03-nl-376 | 0 | Aggregate country index and verification refreshed across all five NL-owned components; hosted component files were not changed |
| 2026-11-03-nl-378 | 0 | Munt-Online deferred pending primary evidence for legal entity and physical location, and confirmation that its current antique-coin category represents qualifying dealer activity |
| 2026-11-03-nl-379 | 0 | Full native-ledger readback: 450 sources verified with empty ingestion |
| 2026-11-03-nl-380 | 0 | Aggregate country index and verification refreshed across all five NL-owned components; hosted component files were not changed |
| 2026-11-03-nl-382 | 1 | EA-Militaria added after full API/all-ledger dedup; first-party operators, registration/contact details, Netherlands-level geography and worldwide tracked delivery recorded; inspected original WebP logo; platform left unknown |
| 2026-11-03-nl-383 | 0 | Full native-ledger readback: 451 sources verified with empty ingestion |
| 2026-11-03-nl-384 | 0 | Aggregate country index and verification refreshed across all five NL-owned components; hosted component files were not changed |
| 2026-11-03-nl-387 | 1 | Servies Merij added with Mijnwebwinkel, the published VOF identity, registered/correspondence address, inspected original logo and a partial tracked PostNL shipping profile; destination countries and rates remain unknown |
| 2026-11-03-nl-388 | 0 | Full native-ledger readback: 452 sources verified with empty ingestion |
| 2026-11-03-nl-389 | 0 | Aggregate country index and verification refreshed across all five NL-owned components; hosted component files were not changed |

| 2026-11-03-nl-410 | 1 | Artiquair added after live identity checks; two Amsterdam addresses, WooCommerce and inspected original PNG monogram verified; email and conflicting shipping details omitted |
| 2026-11-03-nl-411 | 0 | Incremental native-ledger readback; all 457 sources verified with empty ingestion |
| 2026-11-03-nl-412 | 0 | Aggregate country index and verification refreshed across all five NL-owned components; hosted component files were not changed |

| 2026-11-03-nl-413 | 1 | Galerie Anna van Elteren added with verified KvK, original SVG wordmark and canonical-domain alias; variable shipping remains unprofiled |
| 2026-11-03-nl-414 | 0 | Incremental native-ledger readback; all 458 sources verified with empty ingestion |
| 2026-11-03-nl-415 | 0 | Aggregate country index and verification refreshed across all five NL-owned components; hosted component files were not changed |

| 2026-11-03-nl-417 | 1 | Quellinus Oude kunst en Antiek added with WooCommerce, current priced own-stock catalogue and Gouda premises; logo and shipping omitted where unverifiable |
| 2026-11-03-nl-418 | 0 | Incremental native-ledger readback; all 459 sources verified with empty ingestion |
| 2026-11-03-nl-419 | 0 | Aggregate country index and verification refreshed across all five NL-owned components; hosted component files were not changed |

| 2026-11-03-nl-421 | 1 | De Gouden Engel added after current direct in-stock webshop offers superseded the earlier Catawiki-only exclusion; third-party channels omitted |
| 2026-11-03-nl-422 | 0 | Incremental native-ledger readback; all 460 sources verified with empty ingestion |
| 2026-11-03-nl-423 | 0 | Aggregate country index and verification refreshed across all five NL-owned components; hosted component files were not changed |

| 2026-11-03-nl-425 | 0 | Odette Welvaars recorded as deferred: contact and correspondence address verified; current available own-stock offer not confirmed |
| 2026-11-03-nl-427 | 0 | Full native-ledger API readback; all 460 sources verified with empty ingestion |
| 2026-11-03-nl-428–431 | 0 | Full API readbacks of all four hosted components, each containing one source; empty ingestion verified |
| 2026-11-03-nl-432 | 0 | Full aggregate verification and index refreshed across all five NL-owned components; all 464 sources verified |

| 2026-11-03-nl-433 | 0 | Antiek58 deferred: operators, contacts and shop address verified; current first-party collection reports no items and accessible item page has no availability status |
| 2026-11-03-nl-434 | 0 | Aggregate country index and verification refreshed across all five NL-owned components; all 464 sources verified and ingestion remains empty |

| 2026-11-03-nl-435 | 0 | Heidema Antiek deferred: current WooCommerce antique offer and warehouse/workshop contacts verified; legal seller identity remains unresolved and published terms are incomplete |
| 2026-11-03-nl-436 | 0 | Aggregate country index and verification refreshed across all five NL-owned components; all 464 API sources remain verified with empty ingestion |

| 2026-11-03-nl-437 | 0 | Onterfd Goed deferred: registered nonprofit and current visit address verified; no public individual offers to establish current qualifying antique/vintage stock, and terms retain an older address |
| 2026-11-03-nl-438 | 0 | Aggregate country index and full verification refreshed across all five NL-owned components; all 464 API sources remain verified with empty ingestion |

| 2026-11-03-nl-439 | 0 | Janssen Antieke Schouwen deferred: current WooCommerce catalogue, showroom, contacts and inspected original logo verified; legal entity conflicts with a 2022 footer using the same KvK number |
| 2026-11-03-nl-440 | 0 | Aggregate country index and full verification refreshed across all five NL-owned components; all 464 API sources remain verified with empty ingestion |
| 2026-11-03-nl-441 | 1 | Studio Galli added after current own-stock vintage/antique offers, KvK/VAT, JouwWeb, appointment showroom and original logo verified; address postal-code conflict and transport destinations omitted pending clarification |
| 2026-11-03-nl-442 | 0 | Incremental aggregate country checkpoint refreshed across all five NL-owned components; all 465 sources verified with empty ingestion |
| 2026-11-03-nl-443 | 0 | Jaber Inkoop Verkoop deferred: mixed new-electronics inventory and current antiques are visible, but legal seller identity and sustained dealer focus remain unverified |
| 2026-11-03-nl-444 | 0 | Aggregate country index and verification refreshed across all five NL-owned components; all 465 sources verified with empty ingestion |
| 2026-11-03-nl-445 | 0 | Van Galen Antiek deferred: active Limmen shop and own delivery asserted, but no individually inspectable current stock or public legal registration details found |
| 2026-11-03-nl-446 | 0 | Mantorini excluded: first-party page confirms current auction brokerage and Catawiki auction sales |
| 2026-11-03-nl-447 | 0 | Aggregate country index and verification refreshed across all five NL-owned components; all 465 sources verified with empty ingestion |
| 2026-11-03-nl-448 | 0 | Incremental native-ledger readback refreshed after the candidate review; 461 sources verified with empty ingestion |
| 2026-11-03-nl-449 | 0 | Aggregate country index and verification refreshed across all five NL-owned components; all 465 sources verified with empty ingestion |
| 2026-11-03-nl-450 | 1 | Old Goods added with KvK/VAT identity, Shopify, correspondence address, inspected original logo, worldwide quote-based shipping and empty ingestion; no inventory or dispatch address inferred |
| 2026-11-03-nl-451 | 0 | Incremental aggregate country checkpoint refreshed across all five NL-owned components; all 466 sources verified with empty ingestion |
| 2026-11-03-nl-452 | 0 | Cox Antiek deferred: legal contact, active premises and antique resale verified, but category pages expose no inspectable current item offers |
| 2026-11-03-nl-453 | 1 | Pappalotti added with sole-proprietor/KvK identity, 255-item WooCommerce catalogue, original logo and empty ingestion; no street location or shipping policy found |
| 2026-11-03-nl-454 | 1 | Bodes & Bode added with Shopify vintage catalogue and two active boutiques; payment-logo composite and unverified legal suffix/shipping terms omitted |
| 2026-11-03-nl-455 | 0 | Incremental native-ledger readback; 464 sources verified with empty ingestion |
| 2026-11-03-nl-456 | 0 | Aggregate country index and verification refreshed across all five NL-owned components; all 468 sources verified with empty ingestion |

| 2026-11-03-nl-461 | 1 | Le Patiné Brocante added after current own-stock offers and matching KvK/VAT identity were verified; Mijnwebwinkel and Netherlands PostNL rates recorded, with uncertain email and logo omitted |

| 2026-11-03-nl-462 | 0 | Kunstenantiekverkoop.nl deferred: current antique offers and contact route verified, but inherited collection has no verified legal dealer identity or business address; aggregate index and verification refreshed across all five NL-owned components |

| 2026-11-03-nl-463 | 0 | Nico van den Assem deferred: specialist business and contacts verified, but inspected item pages are dated October 2025 and availability is not explicit; aggregate index and verification refreshed across all five NL-owned components |

| 2026-11-03-nl-464 | 0 | Hollandia Housewares deferred: live antique/vintage catalogue and Shopify confirmed, but legal operator and Netherlands business location are unverified and first-party WhatsApp numbers conflict; aggregate index and verification refreshed across all five NL-owned components |
| 2026-11-03-nl-465 | 0 | De Overkant deferred: Dutch showroom and contact details found, but legal operator is unverified, the linked shop exposes no inspectable offers, and its published visit schedule is for 2025; aggregate index and verification refreshed across all five NL-owned components |
| 2026-11-03-nl-466 | 1 | Verzamelwinkel In Stijl added with first-party operator/KvK/contact evidence, weekly updated priced collector stock, Boskoop shop, inspected 709x58 GIF wordmark and empty ingestion; platform and shipping terms omitted as unverified |
| 2026-11-03-nl-467 | 0 | ’t Rommelparadijs deferred: current Obdam shop and contact details are published, but no qualifying item-level antique/vintage offers or legal registration details were verifiable; aggregate index and verification refreshed |
| 2026-11-03-nl-468 | 0 | Amelia’s Hoeve Antiek deferred: current orderable antique furniture and delivery terms are documented, but two current first-party KvK numbers conflict; aggregate index and verification refreshed |
| 2026-11-03-nl-469 | 0 | Holland Art Shop deferred: current antique/vintage art, Shopify platform and Netherlands presence verified; legal operator and address remain unverified; aggregate index and verification refreshed |
| 2026-11-03-nl-470 | 1 | JU+JU added with verified vintage furniture stock, KvK/contact/premises, WooCommerce, inspected original PNG logo, domestic/international delivery terms and empty ingestion; source, Party, location and delivery defaults read back |
| 2026-11-03-nl-471 | 0 | Design2have deferred: current priced vintage stock, Mijnwebwinkel and KvK number confirmed, but the first-party address is incomplete and legal operator cannot be verified; conflicting directory address not adopted; aggregate index and verification refreshed |
| 2026-11-03-nl-472 | 1 | Kyodai Originals added with identified operator, contact details, custom React/Supabase platform, inspected original logo, Amsterdam appointment-viewing location and insured worldwide shipping; all resources read back and ingestion empty; aggregate verification refreshed |
| 2026-11-03-nl-473 | 0 | Mieke Zilverberg deferred pending free authoritative reconciliation of the current first-party Amsterdam address with KvK 33218073; no API resources created; aggregate checkpoint refreshed |
| 2026-11-03-nl-475 | 1 | The Lighterhouse added with M.A.X. Meijster / KvK 89207092, Delft registered address, current WooCommerce catalogue, inspected original wordmark, worldwide insured shipping included in price, and empty ingestion; all resources read back |
| 2026-11-03-nl-476 | 1 | Kunsthandel Mieke Zilverberg added with verified owner/contact, current Amsterdam gallery address, WooCommerce catalogue, inspected original PNG logo and empty ingestion; conflicting KvK/address evidence omitted; no delivery terms asserted |

| 2026-11-03-nl-477 | 0 | Fresh free-search batch: results were mainly identities already managed or previously deferred; Heritage Antiques & Brocante reconfirmed closed/without current offers and ZGAO still exposes only a JavaScript shell; live API snapshot and all-ledger deduplication complete; no API writes |

| 2026-11-03-nl-478 | 1 | Rijp Vintage added after all-ledger/live API deduplication; owner names and KvK, Wix storefront, current 1950s–1990s furniture, original 9449x9449 PNG logo, Oss warehouse, worldwide quoted delivery and empty ingestion read back |
| 2026-11-03-nl-479 | 1 | Replate added after all-ledger/live API deduplication; KvK/VAT identity, correspondence location, WooCommerce, orderable 1962-designed vintage tableware, original 600x271 PNG logo, worldwide cart-calculated tracked delivery and empty ingestion read back |
| 2026-11-03-nl-480 | 0 | Eclectic Studio deferred because its collection is not yet online and the site lacks legal identity; Joris Antiek deferred after bounded first-party requests returned HTTP 403 and no current item-level offer or operator could be verified |
| 2026-11-03-nl-481 | 0 | Follow-up free search batch deduplicated against NL ledgers/API; Redesign Vintage, Design2have, Retro Studio, Studio1900, Bestwelhip, In de tijd, Alta Design, Vintage Venue and related leads were already present; no writes |
| 2026-11-03-nl-482 | 1 | Galerie Aanblick added after live/all-ledger identity deduplication; KvK 02057603 and legal sole proprietorship, current purchasable art, WooCommerce, original 400x67 PNG logo and Haren gallery read back; no shipping asserted |
| 2026-11-03-nl-483 | 0 | Antiques & More excluded as an auction-brokerage business based on its first-party auction-house/Catawiki service; Dutch Antique Company deferred after first-party site 404s; Strabagante rechecked and remains deferred with all sale items sold out |
| 2026-11-03-nl-484 | 0 | Nana Pearl deferred because first-party current Asian porcelain listings expose no date, period or provenance to verify antique/collectible stock; Esteemed Antiques, Pieter Hoogendijk and D’Antan were already indexed; Van der Heijden continues to return HTTP 500 before its scheduled recheck |

| 2026-11-03-nl-485 | 0 | Peeters Zilverwerk deferred: physical shop is announced closed while website orders remain possible, but current GZU advertisements could not be attributed to Peeters own stock; broad free-search results were already indexed; no API writes |

| 2026-11-03-nl-486 | 0 | Specialty search mostly resolved to existing or deferred NL leads; Barnett Antiques — The Collector deferred because its official venue profile confirms an antique shop but the linked merchant domain did not resolve and legal identity/direct catalogue remain unverified; no API writes |

| 2026-11-03-nl-487 | 1 | Asian African Art added with first-party entrepreneur/KvK/VAT/contact facts, Vessem gallery and postal addresses, WooCommerce, inspected original PNG logo, current orderable ethnographic art and historic coins, and worldwide cart-calculated tracked/insured delivery; empty ingestion read back. Logo fact shape was corrected and the live image PATCH was read back. |
| 2026-11-03-nl-488 | 2 | Frans Faber Tribal Arts and Pablo Teleng added after first-party catalog, operator/contact and geography checks; Frans has an inspected original Squarespace PNG logo and appointment-only Amsterdam gallery; Pablo has KvK/VAT and correspondence address. No unverified delivery terms were asserted; both ingestion lists are empty and all resources were read back. |
| 2026-11-03-nl-489 | 0 | Archipel International Maritime Gallery excluded for third-party commission sales; Poppenverzameling excluded as a private-collection disposal; Relimarkt/CollectionSale deferred pending confirmation of independent dealer/agency status. Evidence and decisions recorded; no API writes. |
| 2026-11-03-nl-490 | 0 | Search results were deduplicated against NL ledgers and the live API. Chong Fine Objects was deferred after its contact and terms pages published conflicting KvK/operator details; M. van den Heuvel Antiek remains deferred pending inspectable current stock. No API writes. |
| 2026-11-03-nl-491 | 1 | Vergulde Kat added after current first-party stock, KvK/VAT/contact, home-gallery geography, WooCommerce and original logo checks; worldwide shipping and free Netherlands delivery recorded with unknown international rate; Party/source/location/default/profile and empty ingestion read back; aggregate refreshed across all five components. |
| 2026-11-03-nl-492 | 1 | Tribal Design added after current first-party priced object listings, named gallery operator/contact, Amsterdam gallery address, WooCommerce and inspected original logo checks; source, Party, location/default and empty ingestion read back. African Art Amsterdam deferred pending independent-dealer status, registered seller and business-address evidence; aggregate refreshed across all five owned components. No paid spend. |
| 2026-11-03-nl-493 | 1 | Galerie Lemaire added after current priced object offers, named owner/contact, Amsterdam gallery and WordPress catalogue were verified; source, Party, location/default and empty ingestion read back. No logo or shipping asserted. Lodder Asian Art deferred pending proof of direct own-stock dealer sales; aggregate refreshed across all five owned components. No paid spend. |
| 2026-11-03-nl-494 | 1 | Van Pruissen Asian Art added after first-party founders/director/contact, Arnhem appointment gallery and current priced own-stock listings were verified; source, Party, location/default and empty ingestion read back. Legal registration, platform, logo and delivery unasserted. Other search results were deduplicated against current country/API records. Aggregate refreshed across all five owned components; no paid spend. |
| 2026-11-03-nl-495 | 1 | Vintage Kroonjuweeltjes added with current vintage jewelry stock, contacts, two Amsterdam locations, Shopify and inspected original PNG; source, Party, locations/default and empty ingestion read back. Legal registration/postcode omitted; conflicting first-party delivery policies left without a shipping profile. Aggregate refreshed across all five owned components; no paid spend. |
