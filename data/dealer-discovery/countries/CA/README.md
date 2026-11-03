# Canada dealer discovery

Canada ISO 3166-1 alpha-2 code: **CA**. Ongoing discovery is active. This directory owns newly qualified Canadian-home operators; earlier ownership in any country ledger, including Germany’s foreign records, takes precedence.

[Ledger](dealers.jsonl) records reviewed evidence and mutation checkpoints; [frontier](frontier.jsonl) holds leads and owner queues; [index](index.tsv) is generated; [verification](verification.json) records live country readback; [searches](searches.jsonl) records free discovery queries. Raw pages, API snapshots and original asset previews are under `/tmp/aura-dealer-discovery/CA/`, outside the repository. Credentials use the existing external owner-only file. Paid-data spend: **$0**.

Current selection scope: **direct art and antiques dealers only**. General camera, film, musical-instrument and other unrelated retailers are excluded even when they offer an old collectible. Two batch057 instrument sources remain historically managed with existing IDs and empty ingestion, explicitly flagged outside this narrower scope; the documented API has no source retirement operation. These two retained resources are included in historical resource counts, not newly qualified current-scope dealers.

Hosted merchants have separate `hosted/<merchant>/dealers.jsonl` files with an explicit merchant scope. A shared host does not belong to that merchant. Check stable merchant IDs, catalogue URLs, names and contacts across all ledgers and the live API before writes. The CA-owned external `onboard_hosted.py` adapter retains the shared write journal while checking AbeBooks merchant IDs across regional hosts and changed seller slugs; platform-root matches require reconciliation. [François Côté](hosted/francois-cote/README.md) uses the exact Bibliopolis `/cote/` path boundary through the CA-only external `onboard_cote.py` adapter.

[Native verification](verification-root.json) and each hosted verification report must pass with explicit ledger/output paths for the same run before the external CA `aggregate.py` rebuilds the country [verification](verification.json) and [index](index.tsv). Country totals include all components and count resource IDs once. `index-root.tsv` and each hosted index retain component results. Continue full checks every fifth batch; do not feed the aggregate report to the native incremental verifier.

Initial checkpoint: no prior Canada files existed. All country and nested hosted-dealer ledgers were read and folded before research (3,791 decisions). Current live API identities are checked again immediately before onboarding. Ingestion always remains empty. Import only current first-party facts assessed strictly above 95% confidence; omit uncertain operator forms, assets, geography and shipping scope.


Run `2026-11-03-ca-001`: Added The Antique Warehouse, J.H. Tee Antiques Ltd., Ernest Johnson and Archer’s Antiques. Original logos were visually inspected for three; Ernest Johnson’s membership mark was rejected. Correspondence and showroom scope remain distinct. Three rejected shipping inputs were reconciled as absent and corrected without recreating dealer/source/location IDs; all resources and defaults now read back successfully. Archer’s actual showroom reopening is due for review on 2026-11-10. No exclusions or deferrals this batch; no overall blocker.


Run `2026-11-03-ca-002`: Added Scott Landon Antiques and Interiors, Uno Langmann Limited, Milord antiquités and William Henry Antiques. Three original logos verified; Milord platform and three dealers’ native shipping remain unknown. Scott Landon shipping geography stays unresolved North America, with separately arranged collection and no guessed collection site. William Henry retains city-only business geography. First Arts excluded for current auction operation. No overall blocker.


Run `2026-11-03-ca-003`: Added Gallery Elder, Cherry Hill Antiques and Smiths Creek Antiques after native current stock review, including direct visual review of Smiths Creek’s catalogue photo. Gallery Elder item-restricted shipping retains geographic evidence with null eligibility and no generalized prices/features. Cherry Hill’s UScooperative lead is queued for USowner; Canadian business remainsCA-owned. Aubrey’s deferred after three bounded antique-page oversize responses; modern reproduction stock was not used to qualify. All11managed sources verified, ingestion empty; no overall blocker.


Run `2026-11-03-ca-004`: Added Finer Things Antiques & Curios and A.H.MacMillan & Family with native priced antique stock, original logos and scoped NovaScotia premises. Finer Things worldwide quote evidence and MacMillan’s unresolved arranged-delivery scope retained. Generic ShopifyCDN libraryasset did not establish platform. Carol Telfer deferred for missing native currenthomecountry, Lana Taylor for specificcurrentstock gap. All13managed sources verified; no overall blocker.


Run `2026-11-03-ca-005`: Added Grand Central Inc., Petit Musee Ltd., Les Antiquités Bolduc and Style Labo Antiquités after native stock, contact and original-logo inspection. Ordered CA/US versus rest-of-world Petit Musee terms preserve unknown item eligibility and costs; Bolduc size tariffs remain observations without invented item classes. Three explicit collection defaults reference verified business locations. Full country readback passed for all17sources; every ingestion list is empty. No exclusions, deferrals or overall blocker in this batch; paid spend $0.


Run `2026-11-03-ca-006`: Added Ibon Antiques and Brassy Beehive with native dated stock, original logos and Edmonton business geography. Ibon variant JSON confirmed available antique stock despite template text; its Canadian small-item shipping and explicit collection remain scoped. Brassy’s Alberta-resident and item-size restrictions are preserved without a blanket shipping prohibition. Where on Earth excluded as a vendor mall; LA Vintage deferred for an uninspectable native item catalogue. All 19 managed sources passed country verification, with empty ingestion and $0 spend. No overall blocker.


Run `2026-11-03-ca-007`: Added Antiquité S.G. and proprietor Karine Belzile / Chic Shack Antique & Vintage with two original logos and three scoped business locations. The family estate auction was expressly operated by Miller & Miller, resolving Chic Shack’s auction-role question. Unknown shipping and collection policies remain absent. Maison Bergeron deferred for a support-only site, Peter Baker for HTTPS identity failures; ordinary public HTTP is being checked as a separate route. All 21 managed sources verified, ingestion empty. No overall blocker; paid spend $0.


Run `2026-11-03-ca-008`: Added Baraset House Fine Art, Bytown Vintage and Elisabeth Legge Antique Prints. Baraset’s current Kakiemon dish and Bytown’s live variant data resolve sold-example/template ambiguity. Legge’s original native masthead and dated priced print offers were inspected through its advertised HTTP catalogue, with destination-unknown shipping tariffs retained faithfully. Visbids excluded for native auction operation; Barry Ezrin’s unresolved domain deferred with auction-affiliation flag. Peter Baker remains deferred for current individual-stock evidence after HTTP recovery. All 24 managed sources verified; ingestion empty, spend $0, no overall blocker.


Run `2026-11-03-ca-009`: Added Glen Manor Galleries and Rumi Antiques. Glen Manor’s current native redirect alias, original brand illustration and shop catalogue photo were inspected; inconsistent template contact details were omitted. Rumi retains explicit Canada-only business geography with empty site roles, avoiding unverified showroom or workshop claims. Land & Ross excluded for operating a multi-dealer centre. Turco Persian deferred because a complete fresh native antique-item read could not be established. All 26 managed sources passed country verification; ingestion empty, spend $0, no overall blocker.


Run `2026-11-03-ca-010`: Added Adanac Antiques, Fedora Antiques, The Ole Blacksmith Shop and Jays Mercantile; excluded Old House Revival as a vendor mall. Full verification confirms 30 Parties and listing sources, 32 locations, 23 logos, 25 platforms and 19 shipping profiles, with all 30 ingestion configurations empty. Preserved conditional shipping and unresolved site roles; paid-data spend remains USD 0.


Run `2026-11-03-ca-011`: Added 84 1/2, Rose Cottage Antiques and Gifts, and Moore’s Art Antiques. Deferred Spes Bona for unavailable current stock, Fresh Vintage for its catalogue transition and Bored Whale for missing individual offers. Verified 33 Parties/sources and empty ingestion; shared-operator Not Half Bad art catalogue is queued for reuse. Packing assistance was not promoted into unsupported shipping coverage.


Run `2026-11-03-ca-012`: Added Vie de Garçons and Attic Books plus the distinct Not Half Bad catalogue under the existing 84 1/2 Party and warehouse. Verified 35 unique Parties, 36 sources, 37 locations and empty ingestion; deferred Bloomfield Treasures for old stock currency and About Books for missing individual offers. Preserved restricted CA/US delivery and unresolved postal destinations without guessing rates.


Run `2026-11-03-ca-013`: Added Juniper Books Inc, Acadia Bookstore and Atticus Books; verified 38 operators and39sources with empty ingestion. Atticus current own seller stock was verified through the catalogue linked from its native site and reciprocal merchant profile; its mailing address remains correspondence only. Deferred Alexandre and Krysik for access/current-detail reconciliation. Queued a US YukonOklahoma lead without any foreign mutation.


Run `2026-11-03-ca-014`: Added Rustic North Ltd in WhitehorseYukon and verified39operators/40sources with empty ingestion. Excluded EastCoastCoins for its current operated auction. Deferred NewfoundlandAntiques for unresolved auction activity and stale legal template, CarrefourAtlantic for missing current available stock, and HarbourLight because its catalogue is explicitly demonstration content. No foreign records changed.


Run `2026-11-03-ca-015`: Added Antiquité Michel Prince Inc and Daniel Bilodeau’s Antiquités Comme dans ltemps. Full live verification passed for41uniqueoperators,42sources,43locations and empty ingestion. Excluded Chester for its explicit multi-seller paid hosting/sale service. Omitted former Lambton shop address, kept the new warehouse town only, and preserved the available bookcase’s explicit off-site status.


Run `2026-11-03-ca-016`: Added Sampsons and The Unexpected Party; verified43operators,44sources,45locations and empty ingestion. Excluded AntiquesOnQueen’s multi-dealer shop and deferred FaithGrant for missing inspectable stock and Goldfinch for explicitly uncertain lookbook availability. Sampsons conflicting postcodes are omitted; Campbellford collection remains subject to confirmation at an undisclosed site.


Run `2026-11-03-ca-017`: Added Rabbit Hole Antiques and Art and Antique Addict; verified45operators,46sources,47locations and empty ingestion. Deferred RareAntiques for its live coming-soon transition. AntiqueAddict’s missing original logo is omitted after404; its detailed native priced stock offer and current Saltair premises are retained. RabbitHole city base has no inferred premises roles.


Run `2026-11-03-ca-018`: Added Bernardi’s Antiques and Vintage Home Boutique Ltd; verified47operators,48sources,49locations and empty ingestion. Kept VHB’s showroom city only and preserved third-party delivery responsibilities, item-based fees and uncertain foreign coverage. Deferred HistoricHouse on repeated502 and SundayCreative for an absent current shop catalogue. Full country verification is next scheduled for batch020.


Run `2026-11-03-ca-019`: Added ABC Antiques Books and Collectibles and Robert MacNeil’s Antiques, and verified49operators/50sources with empty ingestion. ABC’s four owned outlets are distinct from its host malls; conflicting street numbers and off-site stock prevent an exact flagship number or inventory default. Confirmed Ubercart commerce for MacNeil instead of underlying Drupal. Excluded ReFound’s group shop; deferred Oak and Fern until individual historic stock is inspectable.


Run `2026-11-03-ca-020`: Added Leaf and Stone Books and Thompson Rare Books. Full live country readback passed for51operators,52sources,56locations,37logos,43platforms and36shipping profiles/defaults; all52ingestion lists remain empty. Deferred JacobQuinlan until fresh available stock can be confirmed. Current country coverage remains incomplete; Saskatchewan/Alberta research continues. Paid-data spend remainsUSD0.


Run `2026-11-03-ca-021`: Added Iron Crow, Maylene Rose Thriftology and sole proprietor LeahSpafford’s SpaffordBooks. Verified54operators,55sources,60locations and empty ingestion. IronCrow sells/ships only withinCanada and its formerNantonstore is permanently closed; neither foreign shipping nor a second active location was invented. Spafford’s shop and POBox remain distinct. Excluded auction-operatingHeinVentures and deferred TheIndefiniteArticle for missing individual current stock.


Run `2026-11-03-ca-022`: Added Reclaim Vintage, Brocantique and iON Vintage & Antique after live identity checks. Preserved unknown delivery reach, consignment distinctions and iON US ordering by confirmation. Deferred Krave vendor-space model and Divine example gallery. Incremental readback passed; all 58 sources have empty ingestion.


Run `2026-11-03-ca-023`: Added Au Fil du Temps, Bonheur d’occasion and O Vieux Bouquins with own inspectable historic stock. Kept two Au Fil shops distinct, O Vieux POBox correspondence-only and Bonheur offsite inventory unknown. Deferred Meg-Antique native fetch and Vieux Pin reproduction catalogue. Incremental verification passed:61 empty-ingestion sources; paid spend zero.


Run `2026-11-03-ca-024`: Added D & E Lake Ltd. Shopify storefront and David Mason Books. Kept Lake’s three premises and distinct legacy catalogue/policy separate; no inventory default. Original logos inspected at actual dimensions. Both worldwide policies preserve their insurance/rate differences. Incremental readback passed:62 operators/63 empty-ingestion sources. New Ontario/BC leads queued; no blocker.


Run `2026-11-03-ca-025`: Added Corby’s, McClosky’s and The Argosy; added D & E Lake’s separate legacy catalogue under its existing Party/three locations. Corby’s live variants confirm stock despite unconditional theme sold-out badges; pickup-only chair/scale restrictions retained. McClosky worldwide free tracking and Argosy freight/route conditions preserved. Full country readback passed:65 operators/67 sources/74 locations; every ingestion list empty. No new exclusions/deferrals or blocker; zero spend.


Run `2026-11-03-ca-026`: Added Back Lane Books, Black’s Fine Books & Manuscripts, Contact Editions and Coast Consignment after fresh live API identity checks. Coast’s legal operator R.A.B. Industries Inc. and dealer-managed consignment model verified; buyer-arranged transport preserved. Black’s Toronto/Winnipeg galleries remain city-only without inventory default. Contact Editions qualified via own exact merchant1757 offer, not undetailed MobyDick page. Incremental readback passed:69 operators/71 sources; no blocker or spend.


Run `2026-11-03-ca-027`: Added Patrick McGahern Books and Prairie Collective. Prairie’s originalSenecaMace tin is10CAD/InStock; shop andPOBox keptseparate. McGahern actual1819book qualifies independently from1968facsimile. Omitted challenged email/corporatesuffix rather than importing searchsnippets. Deferred Obscurity’s historicstock; Pine&Patina remains country-unassigned infrontier. Existing OldHouseRevival exclusion reaffirmed, not duplicated. Incremental live verification passed:71 operators/73 empty-ingestion sources; no blocker.


Run `2026-11-03-ca-028`: Added DDStudiosLoft/DomDeco with its originalwarehouse and separatecorner atVintageDistrict; explicitonlineinventory default points onlytooriginalwarehouse. Omitted unadvertisedshipping/failedpickupwidget. Deferred MrMansfield/BEX sharedshowroom/operator/vendorrelationship and Capital’s fireclosedpremises/mobilecurrentstock. Excluded OneMan’sTreasure25plusvendorstore. ExistingLAdeferredunchanged. Incremental readback passed:72 operators/74 sources, all emptyingestion; zero spend/no blocker.


Run `2026-11-03-ca-029`: Added Demain Peut-Être, Brocante La Reconquête and Brocante La Baraque. Demain’s conflicting tariff/free-threshold claims remain unimported; Canada-only coverage, no PO boxes and item restrictions are retained. Reconquête has no public shopfront and offers appointment collection only; its private pickup site stays unknown. Baraque’s exact Wix subdomain and current 1961 book qualify. Excluded L’Encan du Géant and its Canadian Acrylic Display division under the same auction operator. Incremental verification passed:75 operators and77 sources, every ingestion list empty; paid spend0. Research continues.


Run `2026-11-03-ca-030`: Added Folly & Fine Company and Rêves-Antiques. Folly’s canonical domain and Squarespace technical origin are one source; exact product data confirms one circa1910 Poe set in stock. Rêves qualifies through its intact late1800s buffet offer and matching photograph; missing images on other entries are documented. House of Silver and Noschoses remain unassigned frontier deferrals without imported facts. Full country readback passed:77 operators,79 sources,89 locations; every ingestion list empty and paid spend0. No blocker; research continues.


Run `2026-11-03-ca-031`: Added Jane Vining Antiques, Retrospekt Halifax and Golden Tonic Jewellery after current stock and live API identity checks. Jane’s circa1925 bracelet explicitly discloses faux sapphires; Golden Tonic’s antique dog brooch qualifies separately from recent estate jewellery. Retrospekt’s conflicting Dartmouth/Hammonds Plains premises are omitted; province-only base and unspecified collection site retained. Original SVG logo verified. Deferred Forever Jewellery & Relics pending individually qualified historic stock. Incremental verification passed:80 operators/82 empty-ingestion sources; no blocker or spend.


Run `2026-11-03-ca-032`: Reviewed Bison Books and Timepiece Canada without API mutations. Bison is deferred while its sole owner’s stated auction-house management role is unresolved. Timepiece’s new Coleman repair site has no inspectable current dealer stock; its former root redirects to the new canonical domain. Audit, index and incremental live country verification passed. Totals remain80 operators/82 sources, with37 deferred and13 excluded; every ingestion list empty, paid spend0. No global blocker; continuing new leads.


Run `2026-11-03-ca-033`: Added Antiquités Brocante Charlevoix after current circa-1910 drum-table stock, native shop/contact and fresh live API checks. Excluded 250 Liquidation for operating auctions; deferred Kinder Surprises pending individual current historic stock and Eclectables pending flea-market operator eligibility. Unreachable La Fosse, NJV and Livyers leads remain unassigned in the frontier. Audit, index and incremental live verification passed: 81 operators, 83 sources, 93 locations; all ingestion lists empty. Paid spend 0; no global blocker.


Run `2026-11-03-ca-034`: Added MFLIBRA Antique Books and Irolita Books after original historic-book photos, available native variants/offers and fresh live identity checks. MFLIBRA free-shipping threshold boundaries conflict, so rates stay unknown; insured/tracked packing is explicit. Irolita retains Canada free shipping, heavy-book US exceptions and current EU shipping suspension despite stale worldwide wording and a country selector. No exclusions or deferrals added. Audit/index/incremental verification passed: 83 operators, 85 sources, all ingestion lists empty; spend 0 and no blocker.


Run `2026-11-03-ca-035`: Added Filo Finds, Wilsonville Vintage & Antiques and Viliya’s Vintage after native current historic-stock checks and fresh live API identity checks. Excluded Roadshow’s 400 as an explicit multi-dealer mall. Full live readback passed for all 86 operators, 88 sources, 98 locations, 60 logos, 75 platforms and 68 shipping profiles/defaults; all 88 ingestion lists remain empty. Foreign Antique Sea Charts remains owned by NL and only an owner follow-up is queued here. Spend 0; no blocker, continuing new Canadian leads.


Run `2026-11-03-ca-036`: Added Cynthia Findlay and Maps & More after fresh native stock/operator checks and live related-name/API checks. Cynthia’s historical market-management role ended in 2009; its current related site points to this own-stock catalogue. Maps & More original maps qualify separately from reproductions; selected-map shipping is not included and requires a quote, rather than being prohibited. Deferred Majesty Maps and Cartolina pending original historic stock. Audit/index/incremental verification passed: 88 operators, 90 empty-ingestion sources; spend 0, no blocker.


Run `2026-11-03-ca-037`: Added My Ruby Tuesday, Patron of Dreams Vintage and FTJCo after current qualified antique stock, original assets and live API identity checks. Excluded The Georgian Artisan Shop for conducting timed HiBid auctions. Ruby Tuesday US costs remain unknown under conflicting tariff/free-shipping copy; Patron retains heavy-item pickup-only exceptions, and FTJCo optional inbound insurance is not treated as universal outbound cover. Audit/index/incremental verification passed: 91 operators, 93 sources, 103 locations; every ingestion list empty, spend 0 and no blocker.


Run `2026-11-03-ca-038`: No additions. Deferred Judith Graves/Sassy Lassie pending current inspectable stock and vendor-model clarity, and IJL pending a specific historic object beyond a generic estate pendant. Excluded Jen’s Auction Centre for current weekly auctions. Logan’s old domain is parked; its identity remains unassigned in the frontier, without inferring closure or antique auction activity from registrar advertising. Audit/index/incremental verification passed: 91 operators, 93 sources, all ingestion lists empty; spend 0 and no blocker.


Run `2026-11-03-ca-039`: Added George Manz Coins and Citadel Coins after current fixed-price historic stock, original-photo inspection and fresh live identity checks. Excluded Alliance Coin, London Coin Centre and Calgary Coin Gallery for their auction operations. Citadel’s merchant shipping exclusions for China/Russia retained without generalizing personalized eBay rates. George’s PO box remains correspondence only. Audit/index/incremental verification passed: 93 operators, 95 sources, 105 locations, every ingestion list empty. No deferrals added, spend 0 and no blocker.


Run `2026-11-03-ca-040`: Added Arpin Philately with actual1966 cover-stock inspection, verified native originalSVG and separate headquarters/correspondence locations. Deferred J&M pending its advertised auction role and actual special-order availability. Saskatoon Stamp and Monnaies de Versailles remain unassigned frontier follow-ups after unusable native stock observations. Full country readback passed: 94 operators, 96 sources, 107 locations, 67 logos, 80 identified platforms and 76 shipping profiles/defaults; all ingestion lists empty. Spend 0; no global blocker, continuing.


Run `2026-11-03-ca-041`: Added John W. Doull, Bookseller and Sellers & Newel after current historic-stock and original-image inspection. Trading operators retained without uncertain personal ownership or corporate forms. Deferred Wayfarers for unavailable individual stock evidence; queued High Park Books under its verified Kitchener hosted identity. Audit/index/incremental readback passed: 96 operators, 98 sources, 109 locations and all ingestion lists empty. No exclusions added, spend 0, no global blocker; continuing.


Run `2026-11-03-ca-042`: Added Schooner Books Ltd. and Minotavros Books with specific nineteenth-century stock, original-image inspection and fresh identity checks. Minotavros worldwide tracking and exact appointment-pickup shop preserved. Deferred Robert Wright and Old Favorites pending inspectable current offers; temporary unavailable inventory is not closure. Audit/index/incremental verification passed: 98 operators, 100 sources, 111 locations, all ingestion lists empty. Spend 0, no global blocker; continuing.


Run `2026-11-03-ca-043`: Added Combat-Relics Inc. after native historic-stock/photo review and current official Canadian corporate identity checks. Registered office remains distinct from unspecified Ottawa/show pickup. Excluded Marway for its auction operation; deferred Duncan Robert pending current activity after a reported closing collection sale. Broad Arrow remains unassigned pending home-country evidence. Audit/index/incremental verification passed: 99 operators, 101 sources, 112 locations, all ingestion lists empty. Spend 0, no global blocker; continuing.


Run `2026-11-03-ca-044`: Added The Canadian Soldier Militaria and Grimshaw Military Antiques after current individual historic-stock and original-photo checks. Both US shipping suspensions retained; correspondence addresses remain distinct from unverified stock or pickup sites. Excluded Military Antiques Toronto for its own auction arm. Audit/index/incremental verification passed: 101 operators, 103 sources, 114 locations, all ingestion lists empty. Spend 0, no global blocker; continuing into the next batch and full country readback.


Run `2026-11-03-ca-045`: Added Barry’s Jewellers Inc. and Paul Nudelman Joailliers Inc. with individually inspectable historic art/vintage jewellery, original branding and verified Canadian legal operators. Queued the Vienna boutique for Austrian entity/owner review and Bullion Canada pending successful native observations. Full country audit/index/readback passed: 103 operators, 105 sources, 116 locations, 74 logos, 89 platforms and 84 shipping profiles; every ingestion list empty. No exclusions or country-ledger deferrals added in this batch, spend 0, no global blocker; continuing.


Run `2026-11-03-ca-046`: Added Feheley Fine Arts and Zone Art Inuit inc. with current1959/1967 original-print offers, inspected native images and verified Canadian business identities. Mixed-stock export restrictions, conflicting free-shipping thresholds and art-only insurance remain explicit omissions rather than universal guarantees. Deferred Selim’s pending current stock beyond its past-sale gallery. Audit/index/incremental verification passed: 105 operators, 107 sources, 118 locations, all ingestion lists empty. Spend 0, no global blocker; continuing.


Run `2026-11-03-ca-047`: Added North End Gallery in Whitehorse and ANNAKK Home near Hague after current original-art/vintage-furniture checks. North End’s EU shipping suspension/card exceptions and ANNAKK’s appointment collection remain distinct. Queued Relic Revival in Yukon, Oklahoma for the United States; existing CA-owned leads were skipped. Audit/index/incremental verification passed: 107 operators, 109 sources, 120 locations and all ingestion lists empty. No new exclusions or deferrals, spend 0, no global blocker; continuing.


Run `2026-11-03-ca-048`: Added High Park Books under its exact hosted AbeBooks merchant5286574, with verified Kitchener home, current individual historic-book offers and fresh name/contact/merchant checks. Generic covers/provider branding were omitted. Native and hosted components passed explicit audits/readbacks; aggregate country index and verification now cover108 operators,110 sources,121 locations, with every ingestion list empty. No new exclusions or deferrals; spend0, no global blocker; continuing.


Run `2026-11-03-ca-049`: Added Aquila Books, Benjamin Books and Brown and Dickson; Alphabet Bookshop deferred for inspectable current stock. Native and hosted incremental verification passed. Continuing ABAC member leads.


Run `2026-11-03-ca-050`: Added de Freitas and Edmonton Book Store; Cardinal Books deferred for current stock and E C Rare Books queued pending first-party country and stock. Full native and hosted readbacks passed; all ingestion remains empty. Continuing independent Canadian shops.


Run `2026-11-03-ca-051`: Added The Odd Book and William Matthews / The Haunted Bookshop. Hollywood Canteen deferred for unresolved auction role and individual current stock; Imagination on Paper queued pending native identity. Native and hosted incremental verification passed; paid-data spend zero.


Run `2026-11-03-ca-052`: Added Websters Fine Books and Maps, Jason Rovito and exact hosted Reeve and Clarke / Budleigh Holdings catalogue. Vetus Carta deferred for inspectable current stock; separate Harpers US lead queued without API mutation. All three component audits and readbacks passed, ingestion empty; continuing.


Run `2026-11-03-ca-053`: Added Burton Lysecki Books, Post Horizon Booksellers and Scene of the Crime Books. Monkeys Paw deferred for individual current stock. Readbacks and all component audits passed: 121 operators and 123 sources, all ingestion empty. Current store closures and shipping-origin contradictions preserved without relocating businesses; continuing.


Run `2026-11-03-ca-054`: No additions this review; Balfour Books, The Scribe and The Word deferred for individually verified historical stock. Francois Cote remains a Montreal hosted-path lead with current August 2026 retail catalogue pending exact scope and role review, without claiming the shared host. All three component audits and incremental readbacks passed; 121 operators,123 sources, all ingestion empty. Continuing Nunavut art and antique-clock research.


Run `2026-11-03-ca-055`: Added The Clock Factory and Carvings Nunavut Inc. with current 1924 clock/original artwork, inspected original logos, verified Canadian business homes and separate Iqaluit gallery inventory evidence. Doctor Clock deferred for individual stock; unverified vintagepens.ca kept unassigned. Full native and both hosted audits/readbacks passed: 123 operators,125 sources,137 locations; all ingestion empty. Continuing scoped Francois Cote catalogue and fresh leads.


Run `2026-11-03-ca-056`: Added Francois Cote, libraire under exact Bibliopolis /cote/ merchant scope with current 1967 vintage-book retail offer and correspondence-only Montreal geography. Shared host, other tenants and external auction promotions remain distinct. Existing Contact Editions alternate domain queued for its CA owner after HTTP520. All four component audits/readbacks passed:124 operators,126 sources, all ingestion empty; continuing vintage-instrument dealers.


Run `2026-11-03-ca-057`: Added Folkway Music Inc. and Capsule Music with current 1929/1976 original instrument offers, inspected original SVG/GIF branding and verified Canadian shops. Twelfth Fret excluded for its own published online-auction offering. All four component audits/readbacks passed:126 operators,128 sources, all ingestion empty. Rates/material and merchandise scope retained faithfully; continuing original vintage-camera stock.


Run `2026-11-03-ca-058`: Added FineArtandAntiques.ca with current original-art catalogue, inspected original masthead/photo and city-only Thornbury base. Excluded four general camera retailers under the narrower direct art/antiques scope and Martine Bore for its auction operation; deferred Koolen and Allan Pitchko for fresh individual stock evidence. Two previously onboarded instrument sources are explicitly flagged outside current scope while existing IDs remain retained because the documented API has no retirement operation. All four component audits/indexes/incremental readbacks passed:127 historical operators,129 sources, every ingestion list empty. Spend0; no global blocker, continuing direct art galleries.


Run `2026-11-03-ca-059`: Added Alan Klinkhoff Gallery and Loch Gallery with current1936/1984 original art, inspected native stock photos and separately verified gallery premises. Klinkhoff original SVG/Artlogic verified; Loch Drupal content backend identified through current native page-data, avoiding obsolete search artwork routes. No inventory defaults inferred from individual viewing locations. Excluded Mayberry for its own proprietary auction venture. All four component audits/indexes/incremental readbacks passed:129 historical operators,131 empty-ingestion sources; spend0, no global blocker. Continuing Canadian Indigenous-art galleries; full readback scheduled060.


Run `2026-11-03-ca-060`: Added Inuit Gallery of Vancouver Ltd., Douglas Reynolds Gallery, Coastal Peoples Gallery and Galerie Images Boreales after current own-art/photo/brand/contact checks and fresh live API identity checks. Preserved 2026estateedition versus historic manufacture, actual variantavailability, mixedmaterial exportlimits, optionalinsurance and unknownshipping. Full live country readback passed for all four component ledgers:133 historical operators,135 sources,150 locations; every ingestion list empty. No new exclusions/deferrals; spend0, no global blocker. Continuing direct antique shops in British Columbia and Quebec.


Run `2026-11-03-ca-061`: Added Hansell and Halkett Vintage Home Decor Ltd. through its actual antique-table offering and Charmaines Past and Present through its individually priced original-art gallery. Excluded Everything Old for its own auction catalogue and Fostered Finds under the narrower direct art/antiques scope; deferred Deschambault for current stock beyond native2021archives. Preserved furniture delivery/pickup restrictions and unlabelled painting currency. All four component audits/indexes/incremental readbacks passed:135 historical operators,137 sources,152 locations; every ingestion list empty. Spend0, no global blocker; continuing New Brunswick art galleries.


Run `2026-11-03-ca-062`: Added Gallery78, Gallery on Queen and Trinity Galleries through current original-art offers, inspected original logos/photos, current native contacts and fresh live API checks. Omitted uncertain postcode and physical artwork dimensions, separated sold artist-profile examples from active commerce offers, and retained unknown shipping coverage. Handworks remains country-unassigned in the frontier after native403responses. All four component audits/indexes/incremental readbacks passed:138 historical operators,140 sources,155 locations; every ingestion list empty. Spend0, no global blocker; continuing Woodlands online gallery and new antique-shop leads.


Run `2026-11-03-ca-063`: Added Woodlands Gallery after its public product API confirmed Barry the Beaver in stock, and The Door Store Limited through a current architectural-antique offer. Woodlands closed premises and sold-out Moody painting are not treated as current inventory/location evidence. Deferred Antique Market pending retail activity after its own permanent-door closure notice. Distinct English Antiques by Design queued for GB; Canadian namesake remains country-unassigned after timeouts. All four component audits/indexes/incremental readbacks passed:140 historical operators,142 sources,157 locations; every ingestion list empty. Spend0, no global blocker; continuing Artefacts and Legacy Vintage antique specialists.


Run `2026-11-03-ca-064`: Added Artefacts Salvage and Design and Legacy Expediters Inc. / Legacy Vintage at Home through current inspectable architectural-antique stock, original logos and fresh live API checks. Artefacts inquiry stock, new reproductions and postal correspondence remain distinct; Legacy current Ontario relocation and exact legal operator override old directory details. Shipping destination and pickup unknowns preserved. All four component audits/indexes/incremental readbacks passed:142 historical operators,144 sources,160 locations; every ingestion list empty. No new exclusions/deferrals; spend0, no global blocker. Continuing Manitoba antique leads and Calgary original-art galleries; full readback scheduled065.


Run `2026-11-03-ca-065`: Added The Collectors Gallery of Art, Maus Park Antiques Ltd. and Wallace Galleries through current priced originals or period-antique inventory, inspected original stock photos and fresh live API checks. Sold historical examples, unknown currencies, updated premises and unresolved delivery geography remain explicit. Excluded Masters Gallery for its partnered own auction and Brians Corner for unqualified general secondhand stock; deferred Delaquis and Time Travelers Attic for current specific stock. Full live readback passed for all four component ledgers:145 historical operators,147 sources,163 locations; every ingestion list empty. Spend0, no global blocker; continuing Atlantic antique specialists.


Run `2026-11-03-ca-066`: Added Argyle Fine Art and Down To Earth Art Gallery through current original-art stock, inspected native logos/photos and fresh live API checks. Preserved Argyle explicit current USA shipping exclusion; Down To Earth all-listed-stock gallery statement supports advertised inventory, while item-dependent shipping rates and conflicting handling times remain deferred. Excluded Gallery18 for jointly operated auctions; deferred Tole Treasures and Yaciuks for current individual stock. Distinct Australian Angus queued for AU without foreign mutation. All four component audits/indexes/incremental readbacks passed:147 historical operators,149 sources,165 locations; every ingestion list empty. Spend0, no global blocker; continuing online and seasonal Atlantic original-art galleries.

Current checkpoint `2026-11-03-ca-067`: **150 Parties, 152 sources, 168 locations, 107 original logos, 130 identified platforms and 122 delivery profiles/defaults**. Excluded: **34**; deferred: **66**. Ledger audit, generated index and incremental live country verification passed. Every ingestion list is empty; paid spend **$0**. Country-wide coverage is incomplete. Research continues from the frontier. Last full readback: `2026-11-03T19:36:03.460803Z`.

Run `2026-11-03-ca-067`: Added At the Sign of the Whale Gallery, Twelve Acre Hill Gallery and Tony Diodati Art Gallery through current original-art offers, inspected images and fresh live API checks. Kept former storefront closure, winter physical closure, offsite gallery holdings, original-versus-giclee variants and untracked stock counts distinct. Published contact geography does not imply inventory or pickup. All four component audits/indexes/incremental readbacks passed:150 historical operators,152 sources,168 locations; every ingestion list empty. No new exclusions/deferrals; spend0, no global blocker. Continuing Halifax original-art galleries and checking sister-gallery operator reuse.
