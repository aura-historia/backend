# Germany dealer discovery

Owned scope: Germany (`DE`), including earlier incidental foreign dealers retained
by this ledger. Follow the [shared workflow and country ownership rules](../../README.md).

Discovery scope: specialist art and antique dealers whose core business directly
fits this catalogue. General camera, music, fashion and secondhand retailers are
outside the current discovery scope, even if they occasionally offer an old item.
Qualify current dealer-owned art or antiques from first-party evidence.

## Current index

[index.tsv](index.tsv) is the compact domain lookup; [dealers.jsonl](dealers.jsonl) is
the evidence and checkpoint history. [frontier.jsonl](frontier.jsonl) tracks queued and in-review
search leads and processed decisions, not qualified dealers. None of these files contains API credentials.
The last strict all-pass [verification](verification.json) is retained. The latest
[current readback and discrepancies](verification-current.json) records live counts,
individual resource-check dates and any divergence from discovery observations.
The frontier also journals directory refreshes as `REFERENCE_CRAWL`, including visited
member-profile URLs and the next review date. Check these before repeating a directory
crawl. Association lists can include auctions, provider links and foreign businesses;
each dealer still requires its own qualification.
[searches.jsonl](searches.jsonl) records recent queries and duplicate-filter results.
Check it before repeating regional searches; search results remain leads, not import evidence.

Current production checkpoint `2026-11-04-de-253`: **714 Parties and 718 listing sources**,
**825 locations**, **460 original logo URLs**, **450 advertised-delivery profiles/defaults**,
and **626 identified platforms**. Discovery created every ingestion list empty.
The latest live read finds **431 empty** and **287 nonempty** lists; older records now
contain `WEB_CRAWL`. Standard country verification therefore fails its all-empty
assertion, while identity, presentation and dated location/profile/default readbacks
match. Discovery has not changed those live ingestion settings. A Party
represents the business/operator; the API has no additional organization-create resource.
This workflow has not submitted product listings. Paid search, Places, lead verification
and treg spend: **$0**.

The ledger currently records **136 excluded** and **388 deferred** domains. Use the compact
index for names, IDs, platforms and decision codes, and the ledger for evidence, omissions
and revisit dates. Research deferrals are not fraud findings. Primary closure notices
exclude inactive businesses; a closed historic branch or showroom does not by itself
exclude a dealer that demonstrably continues trading online or at fairs.

Work is **ongoing at the user's request**. Checkpoint batches and continue discovery;
do not end the task merely because a batch completed. Stop only on a user instruction
or an actual blocking condition, preserving enough evidence and IDs to resume.

## Practical lessons and remaining enrichment

- Check the live legal notice and contracting terms after a business takeover: indexed
  search results and founder biographies can still name the former proprietor. A temporary renovation or maintenance notice establishes
  neither permanent closure nor inspectable current stock.
- Follow exact public redirects and meta-refresh targets. A failed legacy deep link can
  coexist with a working replacement shop root; verify its primary legal identity before
  treating it as an alias. Check every country owner after reaching a new canonical host.
- Retained WordPress media folders can be served by an active static Astro frontend.
  Runtime assets and native commerce behavior establish platform technology.
- Native product availability and enabled purchase controls can resolve hidden universal
  “sold out” template text. Inspect the actual offer, while keeping sold archives and
  model examples separate. Bidder clauses in a retail shop’s terms require an operator-wide
  auction review; fixed-price checkout alone does not settle that scope.
- When a dealer labels product images as generated examples, inspect its separately
  published original stock photographs. Public app bundles can also expose the actual
  legal-page content: distinguish the owner from the contact representative, and a
  postal address from a shop or registered office.
- Similar dealer names can identify separate businesses: compare legal operators, contacts
  and premises before merging domains. Antikhalle Dresden Thomas Lauke and AntikHalle
  Dresden Schömann remain separate. Conversely, an exact embedded commerce catalogue ID
  can establish one hosted-shop alias; retain the existing managed IDs when enriching it.
- A source's generic shipping tariff cannot override explicit pickup-only items. Rainer
  Völz advertises a Germany tariff while inspected large furniture/clocks forbid
  shipment. Without real per-item class assignments, retain confirmed collection in
  the API and preserve the more detailed shipping observations in the ledger.
- Similar city domains can be one operator: the Stefan Steder/Kunsthandel-RS sites
  share the same Dortmund address and both phone numbers. Confirm names/contacts and
  cross-links, then record aliases rather than new Parties or supposed city branches.
  The TSV appends confirmed aliases as a searchable final column.
- Alternate web presentations can use different technology. Record the platform of the
  actual listing-source URL; an operator's Wix marketing alias does not establish Wix
  on its separate PHP catalogue. Preserve verified additional contacts without replacing
  the canonical site's current contact merely because another address is published.

- Check actual product pages for owned auctions even on a normal retail site. WANDEL
  expressly solicited bids with an opening amount and closing date for one current
  object. Television appearances or buying at someone else's auction alone are not
  that evidence.
- Single-dealer antiquariat.de/GIAQ storefronts can be framed on merchant subdomains
  or embedded in an independent business website. Verify the exact merchant identity
  and catalogue scope; general marketplace origins are never dealer sources. Record
  ANTIQUARIAT_DE commerce separately from WordPress/Contao marketing in the evidence.
  Follow literal publicly issued anonymous frame URLs; stripping a session path can
  cause HTTP 403 without an account requirement. Keep transient session IDs outside
  durable evidence. Merchant-authored listing descriptions may corroborate own stock,
  but one listing's fees and timelines do not establish source-wide fulfillment.
- In antiquariat.de merchant catalogues, `openwk` / `i=WK...` links add items to
  a basket; do not follow them as product-detail links. Read the actual stable merchant
  category/search pages and original image URLs. Anonymous session IDs stay outside
  durable evidence.
  HESCOM `/warenkorb/add/SKU/` links also change baskets through GET; use the
  distinct `/detail/SKU/` pages for read-only product research.

- Shopnetzwerk year sorting can expose erroneous old-looking years on modern books.
  Inspect the actual edition and binding. Its public advanced search POST redirects to
  reproducible GET parameters `erscheinungsjahr_von` / `erscheinungsjahr_bis` on
  `/suche/ergebnisse`; `/suche` alone only renders the form. Merely
  placing nested form field names in a GET request does not apply the filter. A search
  submission is read-only research; do not submit account, purchase or contact forms.
- Shopnetzwerk's ordinary search uses the public GET parameter `stichwort`; combining
  a binding term with verified year parameters can narrow a large catalogue. Check the
  actual edition text: modern ISBN reprints and default years still appear. Whole-basket
  article-count tariffs cannot use one listing's quantity as the seller-order count;
  retain those brackets in the ledger when the API cannot express their scope.
- VDA's current public member form is `/de/mitglieder-form.php` on antiquare.de.
  Its read-only search posts to `mitglieder-trefferliste.php`; `frmLand=Deutschland`
  and `frmTreffer=1000` returned the German list. The old antiquare.com search link
  redirects to news. The public 2023/24 handbook supplies additional website leads,
  but wrapped/incomplete printed URLs need manual repair, never guessed DNS requests.
  Membership and historical addresses do not establish current trading or exclude
  auction houses; verify each dealer's own pages before onboarding.
- Specific current seller restrictions embedded in multiple item descriptions can
  override generic foreign-delivery footers. Record their evidence and scope. A buyer
  using an independent forwarding address does not establish dealer shipping to that
  buyer's destination. Actual catalogue authentication requirements warrant deferral
  when own stock cannot be inspected; do not register or bypass access controls.

- An image-only legacy homepage can look empty to a text extractor: inspect the actual
  image before deciding closure or identity. AntikArche is a business card without enough
  current stock/legal evidence. Frame hints require real frame markup; they are never fetched
  automatically. The public fetcher exposes validated frame URLs for manual follow-up.
- Follow explicitly linked replacement shops before treating a technical failure as dealer
  inactivity. Intellectus Artis moved commerce to a Wix .com shop; matching operator/contacts
  establish the old .de alias. Record distinct shop, showroom and legal premises separately.
- Embedded shop modules may reveal the commerce engine before JavaScript renders products.
  Kult & Krempel has ECWID store modules inside a Mono website; record ECWID. Generic
  JSON-LD image metadata can still point to a literal LOGO placeholder; inspect assets.
- A current-stock page can load an explicitly published anonymous JSON catalogue while
  featured products elsewhere are sold archive entries. Read the page's exact public
  catalogue URL without executing JavaScript, then confirm an available item's own detail.
  Archive examples and a page's automatic current-date label are not availability evidence.
- Page authors and legal operators are different roles. Kunst + Tradition names designer
  Marcus Buchwald in page metadata and proprietor John Goldsworthy in its legal notice.
  A legal-notice generator credit mentioning GmbH does not establish that the dealer
  is a GmbH; require the operator's own explicit legal designation.
- Honor browser-compatible character encodings: legacy ISO-8859-1 HTML labels map to
  Windows-1252, including euro signs and punctuation. Do not silently repair decoded facts.
  Unicode byte-order marks take precedence over HTTP/meta charset declarations. The
  fetcher now handles UTF-8/UTF-16 BOMs; a Word-exported page at dreiraaben.de otherwise
  appeared as NUL-filled markup. Refetch the bytes with correct decoding instead of
  stripping NULs or guessing damaged names and prices.
  An unknown HTTP encoding label may be discarded in favor of a valid HTML declaration:
  Rotes Antiquariat sends `iso-8559-15` but declares `utf8` in the actual page. The helper
  keeps valid transport/BOM precedence and rejects pages whose declarations are all unknown.
- Shopnetzwerk hosts individual dealers on custom domains and merchant subdomains. Verify
  the merchant-specific imprint and exact shop URL; the general Buchfreund marketplace
  is not the listing source. Use SHOPNETZWERK for commerce even when marketing uses Joomla
  or WordPress. Preserve merchant subdomains in duplicate checks and the frontier.
  A zero-result year filter can reflect missing structured dates: Gallus embeds years
  in publisher descriptions and still has inspectable historical stock and recent arrivals.
  Conversely, years extracted from titles, weights or illustration counts can be wrong;
  inspect the actual edition, binding and condition before qualifying old stock.
- Shared family branding can cover separate legal sellers. COLONEUM names Alwin and David
  Homeier separately; await verified stock allocation rather than merging legal operators.
  A name-only substring match, such as ANTIK & KUNST, also requires contact/operator checks.
- Check linked shop domains for business-wide auction activity; a retail-looking main
  site can omit services advertised by the same legal operator elsewhere.
- Conditional boilerplate ("if shipping/collection is offered") establishes no actual
  availability. "Modernes Antiquariat" may mean new remainder books, not rare old books.
  An ePages shipping page can contain only unedited administrator instructions; these
  establish no shipping countries or prices. Use actual merchant terms and offers.
  Omit doubtful postal codes while retaining supported street/country data; do not guess repairs.
  Appointment visits do not override an explicit no-collection policy. Record dated
  holidays separately from permanent closure, and omit dispatch promises affected by them.
- Inspect anchor labels before following legacy item URLs. On hmj-antik.de,
  `artikelnr` links are **add-to-cart actions**, including when sent as GET; use category
  pages and explicitly linked image assets for read-only stock review. The public fetcher
  retains no cookies. HTTP 200 can also carry a fatal shop error; an accessible marketing
  site alone does not make that broken catalogue inspectable.
- Architectural-antique suppliers often mix original reclaimed doors, fittings, floors
  and stonework with new reproductions or newly made furniture. Qualify an actual
  original historical object; age of the raw material or period-style wording alone
  does not date a newly manufactured item. Transport for demolition/restoration is
  not automatically buyer delivery.
- Drop fragments and deduplicate follow-up URLs before requesting them. Links such as
  `index.php?contact#agbtxt` and `index.php?contact#privacytxt` share one response;
  fetch it once and inspect the cached sections. Preserve distinct query parameters.
  A malformed base or individual link must not discard the rest of a readable page;
  the public helper falls back to the fetched URL and skips invalid links.
  Check a large catalogue's public Content-Length before downloading when available;
  prefer a smaller current own-stock page or catalogue within the research bound.
- HESCOM result rows can publish product navigation in an HTML-escaped
  `document.location` attribute rather than an anchor. Read the literal `/detail/SKU/`
  target without executing JavaScript; the first ordinary links can be unrelated
  sidebar recommendations. Check the actual product title before using its photo.
- Some themes put legal/contact text outside `<main>`. If cached `mainText` contains
  only a heading, inspect full cached `text` and HTML before concluding data is absent.
  Malformed navigation comments can also hide real legal content from a standard
  HTML parser. Stöger's original HTML contains complete imprint/terms; inspecting its
  literal content fragment recovers the evidence without executing scripts or guessing
  facts. Keep the original response and describe this extraction in the ledger.
- A GIF's first frame can show an incomplete brand, and its canvas size can exaggerate
  actual wordmark resolution. Inspect animation frames and visible padding before
  accepting it; retain an accepted original URL rather than inventing a static replacement.
  Check actual dimensions and bytes of advertised retina assets: antikum's `2x` URL
  serves the same 234×76 JPEG as its standard logo. A `srcset` label is not resolution evidence.
- Fetch legal/contact and delivery pages plus a few real catalogue pages. Reuse these
  responses; checking the whole stock is unnecessary for qualification. Search snippets
  are leads. Distinguish daily “closed now” labels from permanent closure notices.
- Require concrete commerce fingerprints: generic theme compatibility CSS and cookie
  templates or disabled gallery options mentioning WooCommerce do not establish a shop.
  Active plugin assets, explicit generators and storefront runtime provide stronger evidence.
  Shared routes identify only a software family: Norderstedter Antikmarkt publishes
  `CMS ConLite 2.0` despite using CONTENIDO-style `front_content.php` routes. Preserve
  the actual named generator instead of inferring another product from its URL shape.
  A custom catalogue can use WooCommerce behind a separate `/shop/` path. Kotte's
  public REST index identifies active `wc/store` and `wc/v3` namespaces even though
  its catalogue has no WordPress assets. Inspect public metadata only; cart actions
  and protected customer/order endpoints are unnecessary for platform identification.
- HTML generators, Shopify runtime/CDN assets and WooCommerce assets establish platform
  identity more cheaply than a paid technology lookup. COCO is explicitly named in
  Schulz's privacy policy. Heymann's custom-looking pages do not establish its underlying
  platform with sufficient confidence, so that field remains empty.
- Respect HTML `base` when resolving links and assets; locale/shop prefixes otherwise
  duplicate. Long menu/currency sections can consume text previews: inspect full cached
  main content before deciding legal identity or shipping information is absent.
  Gallery image captions and alternative text can contain actual prices absent from
  extracted body text; check the original object photo and dealer context as well.
  A blank body can contain a literal public JavaScript redirect: follow the exact published
  URL without executing scripts and verify the destination. Issued nested routes can also
  repeat generic information; check actual returned content before treating one as an imprint.
  Honor declared HTML charsets before importing names/addresses; do not repair replacement
  characters by guessing. Remove discretionary soft hyphens for keyword matching while
  retaining original evidence. Inspect an embedded catalogue's actual iframe body when needed.
- Inspect header placement and alternative text, not just filenames containing “logo”.
  Generic organization metadata can incorrectly identify a painting as a logo. White
  transparent marks need a contrasting preview; never replace them with stock images.
  A large, correctly encoded image can still be blank; inspect decoded content.
  A native SVG can embed a high-resolution PNG: verify self-contained references and
  raster dimensions, inspect it, and retain the original URL/digest without Base64 content.
- Shopnetzwerk's linked `bild-lg` endpoint can return the same generic open-book
  placeholder for unrelated stock IDs. Inspect the content before calling it an
  object photograph. Embedded header logos may be tiny JPEG thumbnails; record
  their dimensions and digest when rejected, without persisting Base64 payloads.
- Shopify HTML can contain hidden sold-out badges and generic pickup-error text
  alongside an available product. Check actual variant availability and the purchase
  button's disabled state; hidden fallback text alone establishes neither sold-out
  stock nor a collection offer. Do not submit the form.
- A charitable shop selling its own donated stock can qualify. Do not confuse it with
  a marketplace, donation collection service or unrelated shops of its parent operator.
  Joomla hidden-mail attributes can supply a published email: decode the address parts
  and displayed text, check agreement, and retain the primary page as evidence.
- ProcessWire's public EmailObfuscation module can encode published contacts with a
  page-specific Base64 alphabet. Decode only the displayed business-contact payload,
  check repeated addresses, and retain the legal-page URL. Its module path together
  with native ProCache assets can establish the CMS; generic custom routes cannot.
  A logo reference inside an HTML comment is not an active dealer brand asset.
- Breit blocks ordinary HTTP. An anonymous rendered-page fetch exposed its explicit
  Jimdo Dolphin generator, dealer-hosted image URL and obfuscated contact email. The
  original logo is only 180×180; retain it unscaled. Larger source-hosted originals
  were available for other dealers. Heller had no inspected brand asset; Schulz's
  289×18 wordmark was omitted for insufficient quality. No ICOs were used.
- Imprint addresses can be correspondence-only. Breit explicitly prohibits pickup;
  KULTOASE publishes a PO box, and its representative's street address was not imported.
  Pezold's warehouse allows pickup only by appointment and has no ordinary visitor access.
  No coordinates were guessed or bought. No legal address was treated as stock evidence.
  If two current imprints disagree on a street but confirm the same operator and
  city/postal code, retain the agreed geographic precision and the discrepancy in the
  ledger; do not invent two active premises. Shared websites can also represent legally
  independent organizations: Berliner Büchertisch's cooperative sells the shop stock,
  while its association remains a distinct entity.
- Tübingen's stock spans two sites; select only the business association, without a
  single inventory default. Hof's official move to Melle overrides the older Osnabrück
  directory entry. Preserve both separate business sites when explicitly described.
- Shipping often needs item-level inputs. Tübingen's furniture-specific prices and
  Heller's painting-specific worldwide terms were not generalized across all stock.
  Pezold's profile records confirmed appointment pickup; shipping awaits item classes
  and weights. Missing terms do not mean shipping is prohibited.
  A buyer collection deadline is not an order readiness or transit interval. Geographic
  coverage may be complete when countries are exhaustively listed, even if prices are unknown.
- KULTOASE's currency/country selector differs from its explicit shipping country list;
  use the policy. Free-shipping services retain their order thresholds and fallback
  rates. arthome24's worldwide island exclusion is retained as an unresolved area,
  keeping assessment conservative instead of promising delivery to islands.

- Inspect native offer controls and image banners together. Sold markers may be
  raster images absent from extracted text, while hidden sold-out template labels
  can coexist with an active, available offer. Provider marks are not dealer logos.

Next discovery work: qualify the queued German leads and continue region-by-region
queries for independent antique/vintage dealers. Deferred identity, catalogue and closure
checks are separate from new discovery; no paid provider is currently necessary.

- Large destination tables must respect the API limit of 64 areas per shipping zone. Split verified destinations into disjoint zones with identical reviewed terms; retain ambiguous legacy labels separately. After a rejected profile write, confirm the live profile collection and default before correcting input and reusing existing resource IDs.

- Batch 190: a legacy German domain can redirect to a legally distinct French gallery. Queue the verified current home country and redirect evidence for its owner; historical directories and family names do not prove a transferred business, current German premises or shared operator.

- Collection arrangements use tagged objects such as `{"kind":"CONTACT_TO_ARRANGE"}`; a bare string is invalid. Reconcile rejected profile creation with the source’s live profile array and default before resuming with retained IDs. Explicit Germany-only delivery supports complete country coverage; unknown pickup streets need no invented site reference.
- Shipping class codes require real per-item crawler assignments. Keep item-specific furniture, parcel and digital-goods restrictions in observations when assignments do not exist; use unknown service eligibility where source-wide availability cannot be supported. Put a free-shipping threshold on its service and retain an unknown-price fallback. These observations do not authorize ingestion.
