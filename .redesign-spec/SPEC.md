# Crawler review console redesign — implementation specification

Branch: `epic/#2743-crawler-follow-ups` (PR #2753). Files in scope: `src/crawler/src/review/ui/index.html`, `app.js`, `styles.css`, `inspector.js`, plus `docs/crawler/operations.md` where it describes changed screens.

This document stands alone. Read it top to bottom once, then work in the order of section 14.

## 0. Constraints (do not relax)

- Frontend plus one additive API change: section 5a adds four fields to `GET /api/listing_sources` (one needs a column added to the crawler-local baseline migration). No other route, response shape or query changes; everything else is built from what `GET /api/reviews`, `GET /api/reviews/{id}`, `GET /api/reviews/{id}/matrix` and the other existing endpoints already return.
- No new dependency, build step, CDN, icon font or external font. Fonts are the system stack. Icons are the small inline SVGs listed in section 3.9.
- Keep every security property of the current `app.js`: `escapeHtml` / `escapeHtmlAttr` on every shop-controlled or persisted string; URLs become links only through `productPageLink` / `isHttpUrl` / `linkifyText`; inline `onclick` handlers never interpolate an untrusted value (they name things by index or by an id that matched `REVIEW_ID_PATTERN`); the `Origin`/session handling (`api`, `ensureReviewSession`, `requestReviewAuthToken`, `clearReviewAuthToken`) stays as it is; the preview iframe keeps `sandbox="allow-scripts"` and the `/inspect` endpoints. The CSP allows `'unsafe-inline'` for `script-src` and `style-src` on the console, so inline handlers and the one `style="background:…"` on highlight swatches keep working; add no others.
- Keep the keyboard behaviour: every control is a `<button>`, `<a>`, `<select>` or `<input>`; the action panel keeps `Escape` = cancel and `Ctrl/Cmd+Enter` = confirm; `replaceKeepingFocus` keeps focus across polls; the queue entries stay links to `#review=…` fragments; the fragment contract (`#review=<id>&page=<id>&schema=<index>`) is unchanged.
- Keep `app.js` lean: remove every function and CSS rule that the redesign makes dead (section 11 lists them). Do not add a framework, a template engine or a second render path.
- Every capability in `docs/crawler/operations.md` and `docs/crawler/extraction-and-review.md` stays reachable: queue, Sources (health) view, review detail with candidate schemas, matrix per page with fields, failed checks and non-blocking notes, live preview and inspector, rule editing including all five extraction kinds, approve / reject / needs repair with notes, restore previous, revoke rejection, review page add/remove, missing-value page review, removed-page templates, URL-pattern decision, sample labels, raw attributes, schema diff, "highlight all fields", history, raw JSON of a product-schema candidate. The product owner decided (section 12): the **Raw payload** view of URL-pattern reviews is removed; Diff and Highlight all are kept.
- The "unmapped state label" notice is gone from the product. Do not render it. `pageNotices()` reports only `foreign_price_currency` today and stays the one extension point for future non-blocking notes.

## 1. What changes, in one paragraph

A calmer, larger-type console with one neutral surface palette, one accent, and three meaning colours (green = fine, amber = waiting on you or an informational note, red = failure or blocking). The console gets two top-level views switched in the header — **Sources** (the landing view, full width) and **Reviews** (queue sidebar + review detail) — each with a count badge, so "what needs me" is one click from anywhere. The health view becomes the Sources page that answers "what needs me and why" with a summary strip, a grouped table with one worded status line per issue (failing since, paused until, held since, the blocking review linked directly), the missing-value pages and the domains needing an operator inside it; the sidebar becomes the review queue only. The review page gets one primary action (Approve), one title block, one stacked notice area, a two-row preview toolbar and an inspector whose Selector tab is a form (schemas → rule → fallbacks → picked element), whose Data tab is coverage → failed checks → notes → extracted values, with failed checks in red and notes in amber, visibly separate. Everything that said the same thing twice is said once.

## 2. Design tokens

Replace the top of `styles.css` with exactly this block (values are final):

```css
:root {
    --bg: #f4f5f8;
    --surface: #ffffff;
    --surface-2: #f2f4f7;
    --surface-3: #e7eaf0;
    --ink: #1b2130;
    --ink-2: #4b5468;
    --muted: #6f7788;
    --line: #e2e5eb;
    --line-strong: #c6ccd7;
    --accent: #2b5bd7;
    --accent-hover: #244cb4;
    --accent-soft: #e9effd;
    --accent-ink: #ffffff;
    --ok: #1b7a4e;
    --ok-soft: #e4f4ea;
    --warn: #8a5300;
    --warn-soft: #fcf1dc;
    --bad: #b2261f;
    --bad-soft: #fcebea;
    --code-bg: #f6f7fa;
    --focus: #2b5bd7;
    --shadow: 0 1px 2px rgba(20, 26, 40, 0.06);
    --s-1: 4px;  --s-2: 8px;  --s-3: 12px;  --s-4: 16px;
    --s-5: 24px; --s-6: 32px; --s-7: 48px;
    --r-sm: 6px;   /* controls, badges-as-tags, notices */
    --r-md: 10px;  /* cards */
    --font: ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
    --mono: ui-monospace, SFMono-Regular, Menlo, Consolas, "Liberation Mono", monospace;
    --fs-xs: 12px; --fs-sm: 13px; --fs-md: 14px; --fs-lg: 16px; --fs-xl: 20px; --fs-stat: 26px;
    --sidebar-w: 320px;
    --header-h: 56px;
    color-scheme: light;
}

:root[data-theme="dark"] {
    --bg: #0f1216;
    --surface: #171b22;
    --surface-2: #1d222b;
    --surface-3: #262c37;
    --ink: #e8eaef;
    --ink-2: #bfc5d0;
    --muted: #8b93a3;
    --line: #262c37;
    --line-strong: #3a4250;
    --accent: #8aaaff;
    --accent-hover: #a6bfff;
    --accent-soft: #1b2745;
    --accent-ink: #0f1216;
    --ok: #71d2a0;
    --ok-soft: #132a1f;
    --warn: #f1bb60;
    --warn-soft: #2e2412;
    --bad: #ff928a;
    --bad-soft: #341c1b;
    --code-bg: #12161c;
    --focus: #8aaaff;
    --shadow: none;
    color-scheme: dark;
}
```

Rules for using them:

- Colour means something or is neutral. `--ok` only for "fine / approved / covered"; `--warn` for "waits on an operator" (pending, blocked by review, held removals, repair requested, unsaved edits) and for informational notes; `--bad` for failures (failed check, failing scrape, needs operator, rejected, needs a human, refusal messages). The old purple `--info` family is gone.
- Only one filled accent button per view (Approve on a pending review; none elsewhere).
- Text sizes: page title `--fs-xl`, card and section headings `--fs-md` 600, body `--fs-md`, secondary text `--fs-sm`, badges and table headers `--fs-xs`. Minimum body text is 13 px; nothing below 12 px.
- Spacing: cards pad `--s-4` `--s-5`; sections within a tab are separated by `--s-5` and a 1 px `--line`; controls inside a form gap `--s-3`; inline control groups gap `--s-2`.
- Radius: `--r-md` for cards, `--r-sm` for controls, 999 px for status pills and schema chips.

## 3. Base styles and components

Rewrite `styles.css` from scratch with these components; the mockup stylesheet `console-design/mockups/console.css` (same tokens, same class names) is a working reference for the CSS below, minus its `.shop` and `.faux-menu` mockup-only rules.

### 3.1 Base

```css
* { box-sizing: border-box; }
body { margin: 0; background: var(--bg); color: var(--ink); font: var(--fs-md)/1.5 var(--font); -webkit-font-smoothing: antialiased; }
h1, h2, h3, h4, p { margin: 0; }
h1 { font-size: var(--fs-xl); font-weight: 600; letter-spacing: -0.01em; line-height: 1.3; }
h2 { font-size: var(--fs-lg); font-weight: 600; }
h3 { font-size: var(--fs-md); font-weight: 600; }
h4 { font-size: var(--fs-sm); font-weight: 600; color: var(--ink-2); }
a { color: var(--accent); text-decoration: none; }  a:hover { text-decoration: underline; }
code, .mono { font-family: var(--mono); font-size: 0.93em; overflow-wrap: anywhere; }
.muted { color: var(--muted); }  .small { font-size: var(--fs-sm); }  .xs { font-size: var(--fs-xs); }
:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
```

### 3.2 Controls

```css
button, select, input, textarea { font: inherit; color: var(--ink); }
button, select, input { min-height: 34px; padding: 6px 12px; border: 1px solid var(--line-strong); border-radius: var(--r-sm); background: var(--surface); }
input::placeholder { color: var(--muted); }
input.mono, textarea { font-family: var(--mono); font-size: var(--fs-sm); }
button { cursor: pointer; font-weight: 500; white-space: nowrap; }
button:hover { background: var(--surface-2); }
button.primary { border-color: var(--accent); background: var(--accent); color: var(--accent-ink); font-weight: 600; }
button.primary:hover { border-color: var(--accent-hover); background: var(--accent-hover); }
button.primary-outline { border-color: var(--accent); color: var(--accent); font-weight: 600; }   /* Save field, Save pattern, Save labels, Save templates */
button.primary-outline:hover { background: var(--accent-soft); }
button.danger { color: var(--bad); }  button.danger:hover { background: var(--bad-soft); border-color: var(--bad); }
button.quiet { min-height: 28px; padding: 2px 8px; border-color: transparent; background: transparent; color: var(--ink-2); }
button.quiet:hover { background: var(--surface-2); color: var(--ink); }
button.quiet.danger { color: var(--bad); }
button.icon { display: inline-grid; place-items: center; width: 34px; padding: 0; color: var(--muted); }
button.icon.quiet { width: 28px; min-height: 28px; }
button.icon:hover { color: var(--ink); }
button:disabled, button:disabled:hover { border-color: var(--line); background: var(--surface-2); color: var(--muted); cursor: not-allowed; }
button.quiet:disabled { background: transparent; border-color: transparent; }
label.field { display: grid; gap: var(--s-1); color: var(--muted); font-size: var(--fs-sm); font-weight: 500; }
label.field > input, label.field > select { min-width: 0; color: var(--ink); font-size: var(--fs-md); }
.actions { display: flex; flex-wrap: wrap; align-items: center; gap: var(--s-2); }
pre, textarea { width: 100%; margin: 0; padding: var(--s-3); overflow: auto; border: 1px solid var(--line); border-radius: var(--r-sm); background: var(--code-bg); font-family: var(--mono); font-size: var(--fs-sm); }
textarea { min-height: 280px; }
```

Button classes by role: `primary` (Approve only), `primary-outline` (the save of the tab you are in), default (secondary), `danger` (Reject; destructive confirmations), `quiet` (tertiary text actions: Move up, Delete, Discard edits, Remove page), `quiet danger`, `icon` (one-glyph buttons with `aria-label` and `title`).

### 3.3 Badges and dots

```css
.badge { display: inline-flex; align-items: center; gap: 6px; padding: 2px 10px; border-radius: 999px; background: var(--surface-3); color: var(--ink-2); font-size: var(--fs-xs); font-weight: 600; line-height: 18px; white-space: nowrap; }
.badge-warn { background: var(--warn-soft); color: var(--warn); }
.badge-ok   { background: var(--ok-soft);   color: var(--ok); }
.badge-bad  { background: var(--bad-soft);  color: var(--bad); }
.badge-accent { background: var(--accent-soft); color: var(--accent); }
.dot { display: inline-block; flex: none; width: 8px; height: 8px; border-radius: 50%; background: var(--line-strong); }
.dot-ok { background: var(--ok); }  .dot-warn { background: var(--warn); }  .dot-bad { background: var(--bad); }
.time { color: var(--muted); white-space: nowrap; }
```

`badge(text, tone)` keeps its signature; `BADGE_TONES` becomes `new Set(['warn', 'ok', 'bad', 'accent'])`. `STATUS_TONES`: `PENDING_REVIEW: 'warn'`, `APPROVED: 'ok'`, `REJECTED: 'bad'`, `SUPERSEDED: undefined` (neutral), `PRODUCT: 'ok'` is **not** used any more for URL classes (neutral). `statusBadge('PENDING_REVIEW')` shows **Pending** (not "Pending review"): add `PENDING_REVIEW: 'Pending'` to a small `STATUS_LABELS` map consulted before `displayName`. Actor badges: `HUMAN` → `badge-accent` "Human"; `AGENT_AUTHOR` / `AGENT_REVIEWER` / `SYSTEM` → neutral. Repair badge "Waiting for repair" → `warn`; "Needs a human" → `bad`.

`timeBadge(value, label)` stays (relative time, full date in `title`); render it with class `time`.

### 3.4 Cards, notices, stat tiles

```css
.card { min-width: 0; border: 1px solid var(--line); border-radius: var(--r-md); background: var(--surface); box-shadow: var(--shadow); }
.card-body { padding: var(--s-4) var(--s-5); }

.notice { display: grid; grid-template-columns: auto minmax(0, 1fr) auto; align-items: start; gap: var(--s-2) var(--s-3); padding: var(--s-3) var(--s-4); border: 1px solid var(--line); border-left-width: 3px; border-radius: var(--r-sm); background: var(--surface); color: var(--ink-2); font-size: var(--fs-sm); }
.notice .notice-kind { font-weight: 600; white-space: nowrap; }
.notice-bad  { border-left-color: var(--bad); }  .notice-bad  .notice-kind { color: var(--bad); }
.notice-warn { border-left-color: var(--warn); } .notice-warn .notice-kind { color: var(--warn); }
.notice-ok   { border-left-color: var(--ok); }   .notice-ok   .notice-kind { color: var(--ok); }
.notice-neutral { border-left-color: var(--line-strong); }
.notice .actions { grid-column: 3; }
.notice .dismiss { grid-column: 3; }   /* × icon button, quiet */

.stats { display: flex; flex-wrap: wrap; gap: var(--s-3); }
.stat { display: grid; gap: 2px; min-width: 170px; padding: var(--s-3) var(--s-4); border: 1px solid var(--line); border-radius: var(--r-md); background: var(--surface); }
.stat strong { font-size: var(--fs-stat); font-weight: 600; line-height: 1.1; font-variant-numeric: tabular-nums; }
.stat span { display: flex; align-items: center; gap: 8px; color: var(--muted); font-size: var(--fs-sm); }
```

A notice is `<div class="notice notice-<tone>" role="…"><span class="notice-kind">Kind</span><span>Body</span>[<span class="actions">…</span>]</div>`. The kind is one or two words ("Note", "Failed check", "Not evaluated", "Needs a human", "Repair requested", "Saved", "Refused", "Changed elsewhere", "Cannot approve"). The body is ink-2, never coloured. One notice type replaces today's `hint-card`, `finding-hint`, `success-hint`, `error-panel`, `refresh-banner`, `human-required-card`, `repair-request-card` and `llm-risks`.

### 3.5 Tables

```css
table { width: 100%; border-collapse: collapse; }
th, td { padding: var(--s-3) var(--s-4); border-bottom: 1px solid var(--line); text-align: left; vertical-align: top; }
th { color: var(--muted); font-size: var(--fs-xs); font-weight: 600; letter-spacing: 0.04em; text-transform: uppercase; white-space: nowrap; }
tbody tr:last-child td { border-bottom: 0; }
.table-card { overflow: hidden; }            /* a .card holding one table */
.table-card table { table-layout: fixed; }
tr.group-row { background: var(--surface-2); }
tr.group-row td { padding: var(--s-2) var(--s-4); color: var(--muted); font-size: var(--fs-xs); font-weight: 600; letter-spacing: 0.04em; text-transform: uppercase; }
tr.disabled td { color: var(--muted); }
.cell-main { font-weight: 600; }
.cell-sub { margin-top: 2px; color: var(--muted); font-size: var(--fs-sm); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.data-grid-wrap { width: 100%; overflow-x: auto; border: 1px solid var(--line); border-radius: var(--r-sm); }  /* tables inside tabs */
```

Do not name a row class `section`; `.section` is the inspector section (3.7).

### 3.6 App shell

```css
header.app-header { display: grid; grid-template-columns: 1fr auto 1fr; align-items: center; gap: var(--s-4); height: var(--header-h); padding: 0 var(--s-5); border-bottom: 1px solid var(--line); background: var(--surface); }
.app-header h1 { font-size: var(--fs-lg); }
.header-actions { display: flex; align-items: center; justify-self: end; gap: var(--s-2); }
.view-switch { display: inline-flex; gap: 2px; padding: 3px; border: 1px solid var(--line); border-radius: 999px; background: var(--surface-2); }
.view-tab { display: inline-flex; align-items: center; gap: 8px; min-height: 32px; padding: 0 16px; border-radius: 999px; color: var(--ink-2); font-weight: 500; }
.view-tab:hover { color: var(--ink); text-decoration: none; }
.view-tab.active { background: var(--surface); color: var(--ink); font-weight: 600; box-shadow: var(--shadow); }
.view-tab .badge { padding: 0 7px; line-height: 18px; }
main.app.view-sources { grid-template-columns: minmax(0, 1fr); }
main.app.view-sources .sidebar { display: none; }
.detail.sources { max-width: 1320px; margin: 0 auto; }
.empty-detail { display: grid; gap: var(--s-2); justify-items: center; padding: 20vh var(--s-5) 0; color: var(--muted); text-align: center; }
.empty-detail strong { color: var(--ink); font-size: var(--fs-lg); font-weight: 600; }
.system-status { display: inline-flex; align-items: center; gap: 8px; color: var(--muted); font-size: var(--fs-sm); }
.system-status:empty { display: none; }
.system-status.dirty { color: var(--warn); }  .system-status.stale { color: var(--bad); }

main.app { display: grid; grid-template-columns: var(--sidebar-w) minmax(0, 1fr); height: calc(100vh - var(--header-h)); }
main.app.sidebar-collapsed { grid-template-columns: 48px minmax(0, 1fr); }
.sidebar { display: flex; flex-direction: column; min-height: 0; border-right: 1px solid var(--line); background: var(--surface); }
.sidebar-head { display: flex; align-items: center; gap: var(--s-2); padding: var(--s-3); border-bottom: 1px solid var(--line); }
.sidebar-head input { flex: 1; min-width: 0; }
.sidebar-collapsed .sidebar-head input { display: none; }
.queue { flex: 1; min-height: 0; overflow: auto; padding: var(--s-2) var(--s-2) var(--s-4); }
.nav-item { display: grid; gap: 2px; padding: var(--s-2) var(--s-3); border-radius: var(--r-sm); color: inherit; }
.nav-item:hover { background: var(--surface-2); text-decoration: none; }
.nav-item.active { background: var(--accent-soft); }
.nav-row { display: flex; align-items: center; justify-content: space-between; gap: var(--s-2); min-width: 0; }
.nav-title { font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.nav-meta { color: var(--muted); font-size: var(--fs-sm); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.nav-badges { display: flex; gap: var(--s-1); flex: none; }
.group { margin-top: var(--s-1); }
.group-head { display: flex; align-items: center; gap: var(--s-1); width: 100%; padding: var(--s-2) var(--s-2) var(--s-2) var(--s-1); border: 0; border-radius: var(--r-sm); background: transparent; text-align: left; white-space: normal; font-weight: 500; }
.group-head:hover { background: var(--surface-2); }
.chev { display: inline-grid; place-items: center; flex: none; width: 22px; height: 22px; color: var(--muted); }
[aria-expanded="false"] > .chev svg, details:not([open]) > summary .chev svg { transform: rotate(-90deg); }
.group-text { display: grid; flex: 1; min-width: 0; }
.group-list { display: grid; gap: 2px; padding-left: 26px; }
.group-list.collapsed { display: none; }
.group-list .nav-item { padding: 6px var(--s-3); }
.group-list .nav-title { font-weight: 500; font-size: var(--fs-sm); }

.detail-shell { min-width: 0; overflow: auto; }
.detail { max-width: 1480px; padding: var(--s-6) var(--s-6) var(--s-7); }
.page { display: grid; gap: var(--s-5); }
.page-head { display: flex; flex-wrap: wrap; align-items: flex-start; justify-content: space-between; gap: var(--s-3) var(--s-5); }
.page-title { display: flex; flex-wrap: wrap; align-items: center; gap: var(--s-3); }
.page-meta { display: flex; flex-wrap: wrap; gap: var(--s-1) var(--s-3); margin-top: var(--s-1); color: var(--muted); font-size: var(--fs-sm); }
.page-actions { display: flex; align-items: center; gap: var(--s-2); }
```

Keep the collapsed-sidebar behaviour (`toggleSidebar`, `setSidebarCollapsed`, localStorage key), the chevron turning, the `main.sidebar-collapsed` width change and the reduced-motion rule.

### 3.7 Workbench, tabs, sections, chips

```css
.workbench { display: grid; grid-template-columns: minmax(520px, 1.5fr) minmax(400px, 1fr); gap: var(--s-5); align-items: start; }
.preview { position: sticky; top: 0; overflow: hidden; }
.preview-bar { display: grid; gap: var(--s-2); padding: var(--s-3) var(--s-4); border-bottom: 1px solid var(--line); }
.preview-row { display: flex; align-items: center; gap: var(--s-2); min-width: 0; }
.preview-row > label { flex: none; color: var(--muted); font-size: var(--fs-sm); font-weight: 500; }
.preview-row select, .preview-row input { flex: 1 1 auto; min-width: 0; }
iframe { display: block; width: 100%; height: calc(100vh - 260px); min-height: 560px; border: 0; background: #fff; }

.inspector { display: grid; container-type: inline-size; }
.tabs { display: flex; gap: var(--s-1); padding: 0 var(--s-3); border-bottom: 1px solid var(--line); }
.tab { min-height: 44px; margin-bottom: -1px; padding: 0 var(--s-3); border: 0; border-bottom: 2px solid transparent; border-radius: 0; background: transparent; color: var(--muted); font-weight: 500; }
.tab:hover { background: transparent; color: var(--ink); }
.tab.active { border-bottom-color: var(--accent); color: var(--ink); font-weight: 600; }
.tab .count { margin-left: 6px; color: var(--muted); font-weight: 500; }
@container (max-width: 440px) { .tabs { gap: 0; padding: 0 var(--s-1); } .tab { padding: 0 8px; } }
.tab-panel { display: grid; gap: var(--s-5); padding: var(--s-5); min-width: 0; }

.section { display: grid; gap: var(--s-3); }
.section + .section { padding-top: var(--s-5); border-top: 1px solid var(--line); }
.section-head { display: flex; align-items: center; justify-content: space-between; gap: var(--s-2); }
.section-head h3 { display: flex; align-items: center; gap: var(--s-2); }

.chips { display: flex; flex-wrap: wrap; gap: var(--s-2); }
.chip { min-height: 32px; padding: 4px 14px; border-radius: 999px; }
.chip.active { border-color: var(--accent); background: var(--accent-soft); color: var(--accent); font-weight: 600; }
.chip.add { color: var(--muted); border-style: dashed; }
.toolbar { display: flex; flex-wrap: wrap; align-items: center; gap: 2px; }

.selector-row { display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: var(--s-2); }
.rule-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: var(--s-3); }
.rule-grid .wide { grid-column: 1 / -1; }
.fallbacks { display: grid; gap: var(--s-2); }
.fallback { display: grid; grid-template-columns: minmax(0, 1fr) auto auto; gap: var(--s-2); }
.picked { display: grid; gap: var(--s-2); padding: var(--s-3) var(--s-4); border: 1px dashed var(--line-strong); border-radius: var(--r-sm); background: var(--surface-2); font-size: var(--fs-sm); }
.picked-head { color: var(--muted); font-weight: 500; }
.picked code { color: var(--ink); }
.picked .sample { color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
```

### 3.8 Data tab lists

```css
.kv { display: grid; grid-template-columns: 128px minmax(0, 1fr); gap: var(--s-2) var(--s-4); margin: 0; }
.kv dt { color: var(--muted); font-size: var(--fs-sm); font-weight: 500; padding-top: 1px; }
.kv dd { margin: 0; min-width: 0; overflow-wrap: anywhere; }
.kv dt.bad { color: var(--bad); font-weight: 600; }
.kv .blank { color: var(--muted); font-style: italic; }
.check-list { display: grid; gap: var(--s-3); }
.check { display: grid; gap: 6px; }
.check-head { display: flex; align-items: baseline; gap: var(--s-2); }
.check-head.bad strong { color: var(--bad); }  .check-head.warn strong { color: var(--warn); }
.check-pages { display: flex; flex-wrap: wrap; gap: 6px; }
.page-tag { display: inline-flex; align-items: center; gap: 6px; max-width: 100%; padding: 2px 8px; border: 1px solid var(--line); border-radius: var(--r-sm); background: var(--surface-2); color: var(--ink-2); font-size: var(--fs-xs); }
.page-tag code { display: inline-block; max-width: 240px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; vertical-align: bottom; }
.page-tag .role { color: var(--muted); }
.image-list { display: grid; gap: 4px; margin: 0; padding-left: 20px; color: var(--ink-2); }
.image-list li { min-width: 0; overflow-wrap: anywhere; }
.expandable summary { cursor: pointer; color: var(--ink); }  .expandable .ellipsis { color: var(--muted); }
.history summary { display: flex; align-items: center; gap: var(--s-2); padding: var(--s-3) var(--s-4); list-style: none; color: var(--ink-2); font-size: var(--fs-sm); cursor: pointer; }
.history summary::-webkit-details-marker { display: none; }
.history summary strong { color: var(--ink); }
```

### 3.9 Inline SVG icons (the only ones)

All 16–18 px, `stroke="currentColor"`, `fill="none"`, `aria-hidden="true"`, `focusable="false"`; each is a JS constant next to `chevronIcon()`:

- `chevronIcon()` (exists): `M4 6l4 4 4-4`.
- `moonIcon()` / `sunIcon()` for the theme toggle: moon `M21 12.8A9 9 0 1 1 11.2 3 7 7 0 0 0 21 12.8z`; sun: circle r=4 at 12,12 plus eight 2 px rays (`M12 2v2M12 20v2M2 12h2M20 12h2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4`).
- `externalIcon()` "open in new tab": `M14 4h6v6M20 4l-9 9M19 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1h5`.
- `eyeIcon()` "highlight in the preview": `M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z` + circle r=3.
- `closeIcon()` "remove / dismiss": `M6 6l12 12M18 6L6 18`.

## 4. Navigation, header and sidebar

### 4.1 Two views, switched in the header

The console has two top-level views. **Sources** is the landing view: full width, no sidebar, the health overview of section 5. **Reviews** is the queue sidebar plus the review detail (or an empty state). A pill switch in the centre of the header selects them; each pill carries the one number an operator cares about:

```html
<header class="app-header">
  <h1>Crawler review</h1>
  <nav class="view-switch" aria-label="View">
    <a class="view-tab [active]" href="#sources" [aria-current="page"]>Sources [<span class="badge badge-warn">{sources needing attention}</span>]</a>
    <a class="view-tab [active]" href="#reviews" [aria-current="page"]>Reviews [<span class="badge badge-warn">{pending reviews of crawl-enabled sources}</span>]</a>
  </nav>
  <div class="header-actions">
    <span class="system-status" id="reloadState" role="status"></span>
    <button class="quiet" hidden id="signOutButton" onclick="clearReviewAuthToken()" type="button">Sign out</button>
    <button class="icon quiet" id="themeToggle" onclick="toggleTheme()" type="button" aria-label="Switch to dark mode" title="Switch to dark mode"><!-- moon or sun svg --></button>
  </div>
</header>
```

Why this structure: the two views answer two different questions ("what needs me" / "decide this review"), so they get two screens instead of one entry squeezed into the queue; the switch is two links, so deep links, browser back, bookmarks and keyboard tabbing work with no new mechanism; the badges make "what needs me" visible and one click away from every screen, including from inside a review; and the Sources page gets the full width its table needs. Both badges are omitted at 0 and are amber because both count things waiting on an operator. The pills are static markup in `index.html`; `updateViewSwitch()` sets `active` / `aria-current` and the badge text on every queue poll and on every view change.

### 4.2 View state and the URL fragment

The fragment stays the single source of truth. Extend `selectionFromUrl()` to return `view`:

| fragment | view | selected review |
|---|---|---|
| empty, `#`, `#sources` | `sources` | none |
| `#reviews` | `reviews` | none (empty state) |
| `#review=<id>[&page=…&schema=…]` | `reviews` | `<id>` |
| anything else | `sources` | none |

- `applySelectionFromUrl()` first applies the view (`setView(view)`: toggles `main.app.view-sources`, calls `updateViewSwitch()`, and for `sources` calls `clearSelectedReview()` then `showSourceHealth()`), then the review selection as today. Leaving a review for Sources or `#reviews` goes through the same hashchange path, so unsaved edits still ask first (`confirmDiscardingEdits('Leaving the review', …)`), and the confirm/cancel URL bookkeeping is unchanged.
- `selectionUrlHash()` returns `#reviews` when the view is reviews and nothing is selected, `#sources` for sources, otherwise the review fragment as today; `recordSelectionInUrl()` pushes a history entry on a view change exactly as it does on a review change.
- On load without a fragment the console shows Sources. `localStorage` does not remember the view; the URL does.
- The empty Reviews state (`#reviews`, nothing selected) renders in `#detail`: `<div class="empty-detail"><strong>No review selected</strong><span>Pick a review in the queue. {n} review(s) pending; {k} source(s) need you.</span><span class="actions"><a class="view-tab" href="#sources">See what needs you</a></span></div>` (the link gets a 1 px `--line-strong` border in this context). With 0 pending: "Nothing is pending." and the Sources link.
- The Sources page polls as today through `showSourceHealth()` whenever the view is sources; the queue polls in both views (the badges need it) but renders the sidebar only in the Reviews view.

### 4.3 Header status

`updateReloadState()` sets `#reloadState` to one of: `''` (hidden via `:empty`), `Unsaved edits` (class `dirty`), `Changed elsewhere — refresh` (class `stale`), `Queue refresh failed: <reason>` (class `stale`, from `reportQueueFailure`), `Signed out`. The green dot and "Queue auto refresh active" are gone. `applyTheme` swaps `moonIcon()` / `sunIcon()` and the `aria-label` / `title`.

### 4.4 Sidebar = review queue (Reviews view only)

```html
<section class="sidebar" id="reviewSidebar">
  <div class="sidebar-head">
    <input id="reviewSearch" type="search" placeholder="Search reviews" aria-label="Search reviews" oninput="setReviewSearch(this.value)">
    <button class="icon quiet" id="sidebarToggle" aria-controls="reviewSidebarBody" aria-expanded="true" aria-label="Collapse the queue" title="Collapse the queue" onclick="toggleSidebar()" type="button">…chevron pointing left…</button>
  </div>
  <div id="reviewSidebarBody"><div class="queue" id="reviewList"></div></div>
</section>
```

No "Reviews" heading (the header pill says it), no Sources entry, no needs-operator or missing-value groups (both live on the Sources page). `showQueueList()` renders the review groups or the search-empty / queue-unavailable states. Keep `toggleSidebar`, `setSidebarCollapsed` and the localStorage key; a collapsed sidebar shows only the toggle.

Review group head (unchanged structure, new classes): `button.group-head` with `span.chev` + `span.group-text` holding `nav-row` (`nav-title` = source name, right: `badge badge-warn "N pending"` or `nav-meta "Reviewed"`) and `nav-meta` = `reviewCountsByArtifactType` + ` · crawling disabled` when disabled. The list is `div.group-list[.collapsed]`.

Review item (`renderReviewQueueItem`): `a.nav-item[.active]` → `nav-row` (`nav-title` = artifact type label; `nav-badges` = one badge: "Needs a human" (bad) when `human_required`, else "Waiting for repair" (warn) when a pending review has a repair request, else the status badge) and one `nav-meta` = `displayName(reason) · <time created>`. The LLM-evaluation line leaves the queue (it stays on the review page, 6.3).

Empty states (`div.empty`): `No reviews match "<term>".` / `The review queue is unavailable. It is retried every 15 seconds.` / `No review yet.`

## 5. Sources page (replaces "ListingSource health")

### 5.1 Audit of the current page

Questions the page must answer, in priority: (1) which sources need me now and why, (2) what is blocked on me versus broken by the shop versus broken by us, (3) is each source alive (last scrape, next crawl), (4) where do I go next.

| Current element | Verdict | Reason |
|---|---|---|
| Row highlight + "Blocked by review" badge in name cell + "Blocked URLs 47" + "Pending reviews 1 pending" + "URL error kinds PendingSchemaReview 47" | Merge into one status line | The same fact five times. |
| "Scrape failure streak" + "Last error kind" (two columns) | Merge into one status line | One fact: "5 in a row · HTTP 503". |
| "URL error kinds" as raw `code` enum names with counts | Reword, dedupe | Debug-looking. `PendingSchemaReview`, `MassRemovalHeld` and `MissingRequiredValue` duplicate other cells. |
| "Held removals" column | Merge into status | It is an issue, not a measurement. |
| "None" in every empty cell | Replace with `—` | Noise; the eye should land on filled cells only. |
| Subtitle "A highlighted ListingSource is blocked: URLs wait…" | Drop | The status line says it where it applies. |
| Sidebar "Pages missing a required value" group with its own sentence | Move into the source's row | Not a review; belongs to the source. |
| Sidebar "Domains waiting for an operator" group | Move into the source's status | The most severe source problem was not on the health page. |
| Column "Last successful scrape" | Keep (as "Last scrape") | Liveness. |
| Missing: `product_urls`, domain names, `next_crawl_at`, `missing_value_urls`, `needs_operator` domains, `crawl_enabled` grouping | Add | All already in the API response. |
| `llm_calls_count`, `domain_count`, `last_successful_crawl`, `last_crawl_outcome` other than `NEEDS_OPERATOR`, `crawl_failure_count` on healthy domains | Leave out | Not decisions an operator makes here (`last_crawl_outcome` other values feed the "Next crawl" cell only). |

### 5.2 Layout

Rendered into `#detail` with class `detail sources` (centred, max 1320 px) while the view is sources:

```
h1 "Sources"      meta: "{n} sources · {k} need you" | "{n} sources · nothing needs you"
[stat tiles — only non-zero ones]
[card.table-card: grouped table]
```

Stat tiles (`.stats > .stat`), in this order, each omitted when its number is 0:

| number | label | dot |
|---|---|---|
| Σ `blocked_urls` over crawl-enabled sources | "URLs blocked by a review" | warn |
| count of crawl-enabled sources with a `NEEDS_OPERATOR` domain | "Domains need an operator" (singular "Domain needs an operator") | bad |
| count of crawl-enabled sources with `max scrape_failure_streak > 0` | "Sources failing to scrape" / "Source failing to scrape" | bad |
| Σ `held_removal_urls` | "Removals held" | warn |
| Σ `missing_value_urls` over crawl-enabled sources | "Pages missing a value" | neutral |

When every tile would be 0, render instead one `notice notice-ok`: kind "All clear", body "No source needs you. Every crawl-enabled source is scraping."

### 5.3 Grouping and order

`needsAttention(source)` = `crawl_enabled && (needsOperatorDomains(source).length || blocked_urls > 0 || maxStreak(source) > 0 || held_removal_urls > 0)`. Three groups, rendered as `tr.group-row` headers (omit an empty group):

1. **Needs attention** — `needsAttention`, ordered: needs-operator first, then blocked, then longest streak, then held, then name (keep `compareSourceHealth`, add the needs-operator term first).
2. **Healthy** — crawl-enabled, not in 1, by name.
3. **Crawling disabled** — `!crawl_enabled`, by name, rows get `class="disabled"`.

### 5.4 Columns

`table-layout: fixed`; widths: Source 26 %, Status (auto), Reviews 110 px, Recent errors 170 px, Last scrape 120 px, Next crawl 110 px.

| Column | Content |
|---|---|
| Source | `div.cell-main` name (`listingSourceName`), `div.cell-sub` = domains joined by ", " · "{product_urls} URLs" (singular "1 URL"). |
| Status | `div.issues` with one `div.issue` per applicable line (5.5); healthy rows show `dot-ok` "Scraping". |
| Reviews | `pending_reviews` ? link `"{n} pending"` to `#review=<blocking_review_id>` when the source has one, else to the first pending review of this source in `latestQueue` (today's lookup), else plain text : `—`. |
| Recent errors | `url_error_kinds` minus the kinds shown in Status (`PendingSchemaReview`, `MassRemovalHeld`, `MissingRequiredValue`), worded via `errorKindLabel`, as "{count} {label}" joined with " · "; `—` when none. |
| Last scrape | `timeBadge(last_successful_scrape, 'Last successful scrape')` or muted `Never`. |
| Next crawl | earliest `next_crawl_at` across domains: "in 2h" (future) / "due" (past) / `—`. Add `untilText(value)` next to `elapsedTime`, same units, full date in `title`. |

`errorKindLabel(kind)`: `HttpError` → "HTTP error", `RawCaptureFailed` → "capture failed", `Timeout` → "timeout", `BotProtection` → "bot protection", `SourceListingIdConflict` → "identity conflict", `HTTP_503` etc. → "HTTP 503" (replace `_` with a space for `HTTP_*`), otherwise `displayName(kind).toLowerCase()`.

### 5.5 Status lines (the heart of the page)

Each line: `<div class="issue"><span class="dot dot-{tone}"></span><div>{headline}<div class="muted">{detail}</div></div></div>`. Render every line that applies, in this order:

| Condition | Tone | Headline | Detail |
|---|---|---|---|
| a domain has `last_crawl_outcome === 'NEEDS_OPERATOR'` (one line per such domain) | bad | "Domain needs an operator" | `{domain} · {last_crawl_error_kind or 'unknown failure'} · {crawl_failure_count} failed crawls` + " — fix the crawl root, re-register the domain or disable crawling" |
| `crawl_enabled && blocked_urls > 0` | warn | `<a href="#review={blocking_review_id}">Blocked by a review</a>` (plain text while `blocking_review_id` is null) | `blocked_urls === product_urls ? "All {n} URLs wait on it" : "{blocked} of {total} URLs wait on it"` + ` · since {timeBadge(blocked_since, 'Blocked since')}` |
| `maxStreak > 0` (the domain with the longest streak) | bad | "Failing to scrape" | `"{streak} in a row · {errorKindLabel(last_scrape_error_kind)}"` + ` · since {timeBadge(scrape_failing_since, 'Failing since')}` when set + ` · retries {untilText(scrape_paused_until)}` when set (`"in 40m"`, or `"now"` when past; omitted when null, which means the domain is not paused) + ` · {domain}` only when the source has more than one domain |
| `held_removal_urls > 0` | warn | "{n} removals held" | "Kept by the mass-removal guard" + ` · since {timeBadge(held_since, 'Held since')}` |
| `missing_value_urls > 0` | neutral | `<button class="toggle-link" aria-expanded onclick="toggleMissingValuePages('{id}')">{n} pages missing a required value</button>` | — |
| none of the above and `crawl_enabled` | ok | "Scraping" | — |
| `!crawl_enabled` | neutral | "Crawling disabled" | — |

`toggle-link`: `padding: 0; border: 0; background: none; color: var(--accent); font-weight: 500; min-height: 0;` hover underline. The listing-source id in the handler is safe because it is a TypeID the backend produced; keep using `escapeHtmlAttr` as today.

Expanded missing-value pages render as a second row right after the source's row:

```html
<tr class="subrow"><td></td><td colspan="5"><div class="subpages">
  <div class="subpage">{productPageLink(url) shown as its path, class "url mono", full URL in title} <span class="muted">{missing label}</span> <button onclick="reviewMissingValuePage('{id}', {index})">Add to review</button></div>
  …
</div></td></tr>
```

`.subrow td { padding-top: 0; }  .subpages { display: grid; gap: var(--s-2); padding: var(--s-2) 0 var(--s-2) 18px; }  .subpage { display: flex; align-items: center; gap: var(--s-3); font-size: var(--fs-sm); }  .subpage .url { flex: 0 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }  .subpage button { min-height: 28px; padding: 2px 10px; }`. The missing label is `displayName(page.missing)` + " is empty" (e.g. "Title is empty"); when the API gives none, omit it. Load failure renders one `subpage` with the error text, muted. Keep `expandedMissingValueSources`, `toggleMissingValuePages`, `reviewMissingValuePage` and `confirmDiscardingEdits` semantics; a page is still named by index, never by URL, in the handler.

### 5a. Console API additions to `GET /api/listing_sources`

Four additive fields; nothing existing changes. Every time is RFC 3339 UTC like the existing ones (`#[serde(with = "time::serde::rfc3339::option")]`); the id serialises as a TypeID string like `listing_source_id`.

| Field | Where | JSON type | Meaning | `null` when |
|---|---|---|---|---|
| `blocking_review_id` | source | string (review TypeID) or null | The pending product-schema review the source's blocked URLs wait on: the oldest one, the review whose `created` is `blocked_since`. | `blocked_urls` is 0. |
| `held_since` | source | string (time) or null | The earliest time a currently held URL was held. The mass-removal guard records a held URL as scraped without a capture (`last_scraped = NOW()`), so this is `MIN(last_scraped)` of the product URLs whose `last_error_kind` is `MassRemovalHeld`. | `held_removal_urls` is 0. |
| `scrape_failing_since` | each entry of `domains` | string (time) or null | When the domain's current same-kind scrape failure streak began (circuit open or recorded evidence). | `scrape_failure_streak` is 0. |
| `scrape_paused_until` | each entry of `domains` | string (time) or null | The end of the scraper circuit's cooldown, after which one probe URL is fetched: the stored `listing_source_domains.next_scrape_at`. | The circuit is closed, or the streak is recorded evidence without a cooldown. |

Why not a source-level "next scrape": a healthy source's next scrape is not stored anywhere; the scheduler's eligibility CTE computes it from `last_scraped` and configured intervals, and duplicating that in a reader would drift. The stored cooldown end is what the operator of a failing domain needs ("retries in 40m").

Computation, in `src/crawler/src/review/readers/listing_source_overview.rs`:

- `blocking_review_id`: add a CTE `blocking` to the main query: the union of (a) `cr.review_id, cr.created` for pending reviews that an `ACTIVE` / `DORMANT_SOLD` product URL of the source waits on (`su.schema_review_waited_on = cr.review_id AND cr.status = 'PENDING_REVIEW' AND su.last_error_kind = 'PendingSchemaReview'`) and (b) the source's oldest pending `PRODUCT_SCHEMA` review while the source has no non-empty schema list (the existing `fs` subquery), then `SELECT DISTINCT ON (listing_source_id) listing_source_id, review_id … ORDER BY listing_source_id, created, review_id`. Join it into `overview`; map with `crawler_review_id_from_uuid`. It is `Some` exactly when `blocked_urls > 0` (both derive from the same rows).
- `held_since`: in the existing `u` subquery add `MIN(last_scraped) FILTER (WHERE url_class = 'product' AND last_error_kind = $3) AS held_since` (`$3` is already `MASS_REMOVAL_HELD_ERROR_KIND`).
- `scrape_paused_until`: `list_domain_states` selects `next_scrape_at` from `listing_source_domains` and maps it to `DomainCrawlOverview.scrape_paused_until`.
- `scrape_failing_since`: **not stored today.** `listing_source_domains` keeps `scrape_failure_streak`, `last_scrape_error_kind`, `last_scrape_status_code` and `next_scrape_at`; none dates the streak. Deriving it from `listing_source_urls.updated` of URLs that carry the same error kind is only approximate (a URL that failed with the same kind in an earlier episode and was not retried since pulls the time back), so it is stored instead. Minimal addition: add `scrape_failing_since TIMESTAMPTZ` to the `listing_source_domains` table in the crawler-local baseline migration `src/crawler/migrations/20260101000000_initial_schema.sql` (do not add a new migration file: the crawler is not in production, so epic #2743 puts every crawler-local schema change into the baseline, and a dev database is recreated with the `db-reset` script under `src/crawler/scripts/`). In `src/crawler/src/scraper/candidate_service.rs`, the domain `UPDATE` of `mark_fetch_failure_and_open_domain_circuit` adds `scrape_failing_since = CASE WHEN scrape_failure_streak = 0 OR last_scrape_error_kind IS DISTINCT FROM $3 THEN NOW() ELSE scrape_failing_since END` (the streak restarts when the kind changes, so the date does too), and `close_domain_circuit` sets `scrape_failing_since = NULL`. The reader selects it with the other domain columns. The `DomainHealthSnapshot` fences stay as they are; the new column is not part of a fence.

Model (`src/crawler/src/review/model/listing_source_overview.rs`): `pub blocking_review_id: Option<CrawlerReviewId>` and `pub held_since: Option<OffsetDateTime>` on `ListingSourceOverview`; `pub scrape_failing_since: Option<OffsetDateTime>` and `pub scrape_paused_until: Option<OffsetDateTime>` on `DomainCrawlOverview`, each with a doc comment stating the meaning and null rule above.

Tests to add:

- Unit, trailing `mod tests` of the model file: the four fields serialise as RFC 3339 strings / a TypeID string, and as `null` when absent.
- Integration, `src/crawler/tests/crawler_review_repository.rs` next to the existing overview tests: (1) `should_name_the_oldest_blocking_review_of_a_blocked_listing_source`: two pending product-schema reviews, URLs waiting on both, expect the older one's id and `blocked_since` equal to its `created`; a source without a schema list and a pending first review expects that review; an unblocked source expects `null`. (2) Extend `should_list_held_removals_without_moving_the_last_successful_scrape` to assert `held_since == 2026-10-31T08:00:00Z` (the earliest held `last_scraped`) and `null` for a source without held URLs. (3) `should_report_the_scrape_cooldown_end_and_failing_since_per_domain`: a domain with streak 5, `next_scrape_at` and `scrape_failing_since` set expects both back; a domain with streak 0 expects both `null`.
- Scraper integration, where `mark_fetch_failure_and_open_domain_circuit` is covered: opening from streak 0 sets `scrape_failing_since` to about now; a second failure of the same kind keeps it; a failure of another kind resets it; `close_domain_circuit` clears it.

Docs: in `docs/crawler/operations.md`, the console-API paragraph that begins "`GET /api/listing_sources` is the operator's overview" gains, after "`held_removal_urls`": "and `held_since`, the earliest time a currently held URL was held"; after "`blocked_since`": "and `blocking_review_id`, the pending review `blocked_since` dates (`null` while nothing is blocked)"; and in the per-domain list after "`scrape_failure_streak` and `last_scrape_error_kind`": "with `scrape_failing_since`, when that streak began, and `scrape_paused_until`, the circuit's cooldown end (`null` without a cooldown)". In `docs/crawler/scheduling-and-retries.md` (scraper domain circuits), extend "The domain stores a consecutive same-kind `scrape_failure_streak`, stable `last_scrape_error_kind`/status diagnostics, and `next_scrape_at`" with "and `scrape_failing_since`, when the streak began". The crawler-local schema contract is the baseline migration; no other document lists columns.

### 5.6 States

- Loading: `div.empty` "Loading sources…".
- Overview failed (`latestListingSources === null`): `notice notice-bad` kind "Sources unavailable", body "The source overview could not be read; it is retried with the next queue refresh."
- No source: `div.empty` "No source is registered."
- The page re-renders on every poll through `showSourceHealth()` exactly as today (HTML comparison, `replaceKeepingFocus`).

### 5.7 Copy

Heading "Sources". The term "ListingSource" does not appear in the UI; `docs/crawler/operations.md` keeps the term and says the console calls the view "Sources".

## 6. Review page: common frame

### 6.1 Structure

```html
<div class="page">
  <div class="page-head">
    <div>
      <div class="page-title"><h1>{source name}</h1>{status badge}{repair/human badge}{artifact badge}[{"Source disabled" neutral badge}]</div>
      <div class="page-meta"><span>{displayName(reason)}</span><span>·</span><span>opened {time created}</span><span>·</span><span>edited {time updated}</span></div>
    </div>
    <div class="page-actions">{approval panel, 6.2}</div>
  </div>
  <div id="reviewRefreshBanner" hidden></div>   <!-- notice notice-warn, 6.4 -->
  <div id="actionPanel"></div>                   <!-- 6.5 -->
  <div id="editMessages"></div>                  <!-- 6.4 -->
  {candidate error notice}{human required notice}{repair request notice}{matrix notice}{LLM evaluation card}
  {history}
  {workbench | URL-pattern body}
</div>
```

Placeholders that are empty take no room (`.page > div:empty, .page > div[hidden] { display: none; }`).

### 6.2 Actions (`renderApprovalPanel`)

- Pending: `<button onclick="needsRepair()">Needs repair</button> <button class="danger" onclick="rejectReview()">Reject</button> <button class="primary" onclick="approveReview()">Approve</button>` — this order, Approve last and the only filled button.
- Approved, product schema: `candidate_is_live` ? `badge badge-ok` "Live schemas — editable" : neutral badge "Read-only — not the live schemas" (title text as today); plus `<button onclick="restoreApproval()">Restore previous</button>` when `approvalRestorable`.
- Approved, URL pattern: only Restore previous when restorable.
- Rejected: `<button onclick="revokeRejection()">Revoke rejection</button>` when `rejectionRevocable`; neutral badge "Rejection revoked" when revoked.
- Superseded: nothing.

### 6.3 Notices (one stack, one tone each)

| Source of truth | Tone | Kind | Body |
|---|---|---|---|
| `detail.candidate_error` | bad | Cannot approve | "The candidate fails validation: {error}. Repair it before approving." |
| `review.human_required` | bad | Needs a human | "The review agents gave {n} needs-repair verdict(s) since a human last touched this review and stopped. Edit it, change its pages or labels, or give needs-repair with instructions to hand it back; or approve or reject it." |
| `review.repair_request` | warn | Waiting for repair (pending) / Repair requested (closed) | `{note}` + right-aligned `time requested` |
| matrix missing (`selectedMatrixMissing`) | neutral | Not evaluated | "The candidate has not been evaluated on the live review pages. Evaluating fetches every review page from the shop at its crawl delay." + action **Evaluate on live pages** |
| matrix failure | bad | Evaluation failed | `{failure}` + action **Evaluate again** |
| unevaluated pages | warn | Not evaluated | "{n} page(s) joined the review after the last evaluation ({labels}). Evaluate again before approving." + action **Evaluate on live pages** |
| LLM evaluation (`auto_schema_evaluation`) | card, not a notice | — | see below |

LLM evaluation card (`renderAutoSchemaEvaluation`): `card > card-body` with `section-head` (h3 "LLM evaluation", right: outcome badge — Auto-approved `ok` / Rejected by the model `bad` / Needs human review `warn`), body `small` = summary, then one `small muted` line "Model: {decision} · confidence {confidence} · deterministic gate {passed|failed|not recorded}", and risks as a `notice notice-warn` list inside the card (kind "Risks", body = risks joined by line breaks) when any.

### 6.4 Edit messages and refresh banner

- Refusal: `notice notice-bad` kind "Refused", body = `apiErrorMessage`, `role="alert"`, dismiss `button.icon.quiet` with `closeIcon()` (`aria-label="Dismiss"`).
- Success: `notice notice-ok` kind "Saved", body = success message, `role="status"`, dismiss. Success messages shorten to "Field saved", "Rule deleted", "Candidate saved", "Page added", "Page removed", "Sample labels saved", "Approved", "Rejected", "Repair requested", "Previous configuration restored", "Rejection revoked".
- Refresh banner: `notice notice-warn` kind "Changed elsewhere", body = `selectedReviewRefreshReason`, action **Refresh review**.

### 6.5 Action panel (confirmation with notes)

Keep `openActionPanel` / `renderActionPanel` / keyboard handling. Markup:

```html
<div class="card action-panel [action-panel-danger]" role="dialog" aria-label="…" onkeydown="actionPanelKeydown(event)">
  <div class="card-body">
    <h3>{title}</h3>
    [<p class="small" style="white-space: pre-line">{message}</p>]
    [<label class="field">{notesLabel}<textarea id="actionPanelNotes" rows="3" …></textarea></label>]
    <div class="actions"><button id="actionPanelConfirm" class="{danger|primary}">{confirmLabel}</button><button class="quiet" onclick="cancelActionPanel()">Cancel</button></div>
  </div>
</div>
```

`.action-panel { border-left: 3px solid var(--accent); } .action-panel-danger { border-left-color: var(--bad); } .action-panel .card-body { display: grid; gap: var(--s-3); } .action-panel textarea { min-height: 72px; font-family: inherit; font-size: var(--fs-md); background: var(--surface); }`. Titles become: "Approve?", "Reject?", "Needs repair", "Restore previous configuration?", "Revoke rejection?", "Delete Schema {n}?", "Remove review page?", "Discard unsaved edits?", "Unsaved edits". The review name is already in the page title; drop it from the panel title. Notes labels: "Notes (optional)", "What needs repair?", "Why restore? (optional)", "Why revoke? (optional)".

### 6.6 History

```html
<details class="card history" [open] ontoggle="reviewEventsToggled(this.open)">
  <summary><span class="chev">{chevron}</span><strong>History</strong><span>{n} events · latest: {label lowercased} {time} by {actor word}</span></summary>
  <ol class="events">…</ol>
</details>
```

Actor word: "a human", "the author agent", "the reviewer agent", "the system". Event rows (`ol.events { display: grid; gap: var(--s-2); margin: 0; padding: 0 var(--s-5) var(--s-4) 46px; list-style: none; font-size: var(--fs-sm); } .event { display: flex; flex-wrap: wrap; align-items: center; gap: var(--s-1) var(--s-2); }`): actor badge, `<strong>` label, muted summary, italic notes, muted "v{candidate_version}", time. Unchanged data; only classes and the "version" word shortened to "v".

## 7. Product-schema workbench

### 7.1 Preview panel (left)

```html
<div class="card preview">
  <div class="preview-bar">
    <div class="preview-row">
      <label for="reviewPageSelect">Page</label>
      <select id="reviewPageSelect" aria-label="Review page" onchange="…">{options: "{role label} · {path}" with full URL in title}</select>
      <button class="quiet danger" [disabled] onclick="removeReviewPage()" title="Removes a page a reviewer added or a page waiting on the review; seed pages stay">Remove page</button>
    </div>
    <div class="preview-row">
      <input id="livePreviewUrl" class="mono" value="{previewUrl}" placeholder="https://shop.example/product" aria-label="Preview URL" onkeydown="if(event.key==='Enter') loadPreviewUrl()">
      <button class="icon" onclick="openPreviewUrl()" title="Open in a new tab" aria-label="Open in a new tab">{externalIcon}</button>
      [<button class="quiet" onclick="resetPreviewUrl()">Back to review page</button>]   <!-- only while previewUrlOverride is set -->
      <button [disabled] onclick="addReviewPage()" title="Fetches this URL of the source and adds it to the review pages">Add to review</button>
    </div>
  </div>
  <iframe id="snapshotFrame" …>
</div>
```

- Role labels: `Primary`, `Seed`, `Added` (REVIEWER_ADDED), `Waiting` (WAITING_PAGE), else `displayName(role)`; add `PAGE_ROLE_LABELS` and use it in `reviewPageLabel`, the page tags and the extracted-data head.
- "Open URL" becomes: pressing Enter in the input, or the `openPreviewUrl()` icon which first loads the typed URL into the preview (today's `loadPreviewUrl`) and then opens it in a new tab via a plain `<a>` created with `rel="noopener noreferrer"` only when `isHttpUrl(url)`. Keep `loadPreviewUrl` for Enter.
- Drop the separate link row ("…/products/… Reviewer added page · live HTML"); the input holds the URL, the select shows the role.
- Keep `writeDetail` / `graftAroundFrame` frame-keeping untouched.

### 7.2 Inspector tabs

`Selector · Data · Diff · Removed pages[ · n] · JSON`. The removed-page count renders in `span.count`. Tab buttons unchanged in behaviour (`setWorkbenchPanel`).

### 7.3 Selector tab

```
section "Schemas"                 (section-head right: small muted "applied in this order")
  chips: [Schema 1] [Schema 2 (active)] [+ Add schema]
  toolbar: "Schema 2:" [Move up] [Move down] [Delete]        [Highlight all]   (quiet buttons; Delete quiet danger)
section "Rule"
  label.field Field      <select id="selectedFieldSelect">  option text "{fieldLabel}" + " · failed check" | " · note" | " · empty"
  label.field Selector   .selector-row: <input id="selectedSelector" class="mono"> <button class="icon" title="Highlight in the preview">{eyeIcon}</button>
  [notice notice-bad  kind "Failed check" body "{finding descriptions joined by '; '} on this page."]
  [notice notice-warn kind "Note"         body "{notice texts joined by '; '} on this page. This blocks no approval."]
  .rule-grid: label Matches <select first|all>   label Read <select text|attribute|image_url|attribute_or_text|json_ld_offer>
              [label Attribute name <input>]  (attribute, attribute_or_text)      [label Offer property <select availability|price>] (json_ld_offer)
  h4 "Fallback selectors"  .fallbacks rows: <input class="mono"> [eye icon] [close icon]   then .actions [+ Add fallback] (quiet)
  .actions: [Save field (primary-outline)] [Delete rule (quiet danger)]  …spacer…  [Discard edits (quiet, only while dirty)]
  .picked (only while pickedSelector): "Picked in the preview" / <code>{selector}</code> / <span class="sample">{sample}</span> / .actions [Use as selector] [Add as fallback]
section "Raw attributes"  (only when the registry has keys the schema does not configure)
  .selector-row: <select id="newRawAttributeKey"> [Add]
```

Behaviour notes:

- Option labels of the Read select: Text / Attribute / Image URL / Attribute or text / JSON-LD offer; `title` on each option keeps the one-line explanation already in the code ("The attribute when it is not blank, the element's text otherwise", "availability or price from the offers in the matched ld+json script", "The best full-size image URL candidate of each matched element"). Defaults on change stay (`setCurrentRuleType`).
- Field select suffix: " · failed check" when `fieldFindings(matrixPage, field).length`, else " · note" when a page notice names the field, else " · empty" when `optionalFieldMisses(matrixPage)` names it.
- The selector input keeps `oninput="setCurrentSelector(this.value); postHighlight(this.value)"`; the eye icon calls `postHighlight(currentSelector())`. Fallback inputs keep `setAdditionalSelector(i, v); postHighlight(v)`, their eye calls `postHighlight(additionalSelectorAt(i))`, their close icon `removeAdditionalSelector(i)`.
- "Use as selector" = `applyPickedSelector()`; "Add as fallback" = `addPickedAdditionalSelector()`. The per-row "Use clicked" buttons and "Add clicked selector" are removed (same capability through the picked box).
- "Highlight all" = `postHighlightSchema()`. The legend section is removed: the inspector already labels each highlighted element with its field name.
- Read-only states (closed review, approved-not-live): render one `notice notice-neutral` kind "Read-only" at the top of the tab with today's sentences, and disable the inputs (`disabled` attribute) as today.
- Re-evaluate leaves this tab; it lives in the Data tab and in the matrix notices.

### 7.4 Data tab

```
section "Coverage"        right: badge  ok "6 of 6 pages" | warn "4 of 6 pages" | bad "0 of 6 pages"; add " · 1 not evaluated" to the badge text when notEvaluated
  small muted: "{coverage sentence}. {selected schema sentence}. {unused schemas sentence}"
  .actions: [Evaluate on live pages] <small muted "evaluated {time of matrix live_evaluation.fetched_at or 'never'}">   (omit the time when the matrix carries no live_evaluation)
section (dot-bad) "Failed checks"   right: badge-bad "{checks} check(s) · {pages} page(s)"     — only when the selected schema has findings on any page
  small muted: "A failed check keeps a generated schema out of auto-approval; a human approves it knowingly."
  .check-list: one .check per distinct finding description (findingDescription), head .check-head.bad <strong>{description}</strong> <small muted "{schema label} · {n of total} pages">, then .check-pages with one .page-tag per page: <span class="role">{role label}</span><code title="{full url}">{path}</code>
section (dot-warn) "Notes"          right: badge-warn "{notes} note(s) · {pages} page(s)"     — only when any page of the selected schema has a notice or an optional-field miss
  small muted: "Information next to its field; notes block no approval."
  .check-list: one .check per distinct notice text (check-head.warn) with page tags; then one small muted line per optional-miss kind:
     "{Field} left empty on {n} pages (matched only empty elements): {role list}. The page still applies."  /  "{Field} matched nothing on {n} pages: …"
     and the skipped-element lines as today's text, shortened: "{Field} skipped {k} of {m} matched elements without an image URL on {n} pages (loose selector)" / "… — an empty gallery"
section "Extracted from this page"  right: badge ok "Applied" | bad "Failed · {n} check(s)" | bad "Did not apply"
  small muted: "{schema label} · {role label} · <code>{path}</code>"
  [small bad: page.error when !apply_ok]
  dl.kv: one dt/dd per field (as rawDataCell today): dt gets class bad when the field has a finding; dd = value, or <span class="blank">blank</span>; after the value a badge per finding (badge-bad, short label) and per notice (badge-warn, short label), each with the full description in title; images as ol.image-list (file name only — "…/{file}" — full resolved URL in title and href, "+n fallbacks" as xs muted), 5 shown then details "n more images"; long text via expandableValue (unchanged); shared values / contradicting identities lines as today, as small muted.
```

Short labels (`CHECK_LABELS`): BLANK "Blank", TOO_LONG "Too long", ALL_ZERO_DIGITS "Zero price", MARKUP_TOKENS "Markup", DUPLICATE_VALUES "Duplicate", INVALID_IDENTIFIER "Invalid ID", WORD_LIST "Word list", REPEATED_ACROSS_PAGES "Repeated across pages", NO_IMAGE_ON_ANY_PAGE "No image on any page", DIFFERS_FROM_OTHER_SCHEMAS "Differs from other schema", DIFFERS_FROM_CAPTURED_IDENTITY "Differs from captured identity"; unknown → `displayName(check)`. Notice label: FOREIGN_PRICE_CURRENCY "Foreign currency". Raw codes such as `BLANK` or `FOREIGN_PRICE_CURRENCY` never appear as badge text.

Grouping helper: `groupFindingsByDescription(candidate)` → `Map<description, pages[]>` over `candidate.pages`; `groupNoticesByText(candidate)` likewise from `pageNotices(page)`. The old per-page bullet lists (`plausibility-pages`) are removed.

Coverage sentences (reuse `coverageText`): "Every live page is covered by at least one schema." / "{covered} of {total} pages are covered{; n not evaluated}." / selected schema: "Schema {k} applies to all {n}." / "Schema {k} applies to {a} of {n}; it fails on {role list}." / unused: "Schema {j} applies to none — add a page of its template or delete it."

### 7.5 Diff tab

One `.rule-grid` row: label "Schema" select + label "Compare with" select, then the diff rows. Row status: `same` → muted text "same", no badge; `changed` → `badge badge-accent` "Changed"; `missing current` → neutral badge "Only in compare"; `missing compare` → neutral badge "Only in selected". Values stay `expandableValue` in `pre`-like `code`. Empty: `div.empty` "Add a second schema to compare." Remove `same: 'approved'` from `STATUS_TONES` (green meant nothing here).

### 7.6 Removed pages tab

```
section "Removed-page templates"   right: small muted "{n} templates"
  small muted (editable): "A template declares a page removed: a selector bound to the notice, with text it contains or a regex. The list starts from the stored templates; approval replaces them, so removing one here retires it."
  (approved): "Stored when the review was approved; edit on a pending review."  (closed): "Closed without approval; the stored templates are unchanged."
  one .card-body-less block per template, separated by rules (as today), fields: Selector (.selector-row with eye icon: postHighlight(selector, 'Removed notice', '#dc2626')) · Match (Text|Regex) · Notice text|Regex · actions [Use picked element (quiet)] [Remove (quiet danger)]
  .actions: [+ Add template] [Save templates (primary-outline)] [Discard edits (quiet, while dirty)]
  .picked box as in 7.3 when a selector is picked
section "Matched review pages"
  table (data-grid-wrap): Page (path link) · Role · Outcome (badge ok "Removed" | badge-warn "Still a product") · Templates ("Template 2: {notices}"; when product_title: notice notice-warn inline kind "Check breadth" body "takes the title “{title}” a candidate schema extracts for its notice: too broad unless this page is a removal notice")
  states: "Declare a template to see the pages it matches." / "Save the templates to evaluate them on the review pages." / "No review page matches the templates." as small muted
```

### 7.7 JSON tab

`section "Candidate JSON"` — small muted "Use the Selector tab for normal edits; this is the raw candidate." — textarea `#candidatePayload` — `.actions` [Save JSON (primary-outline)] [Discard edits (quiet)]; then `section "Validation summary"` with the `pre`. Rename "Save full JSON" → "Save JSON".

## 8. URL-pattern review body

Rendered in one `card > card-body` as sections:

```
section "Decision"        right: decision badge: badge-ok "Pattern" + <code>, neutral "No pattern", badge-warn "Undecided"
  small muted: "Current pattern: <code>…</code>" | "Current pattern: none"
  (pending) label.field Product URL regex <input id="urlPatternInput" class="mono" placeholder="e.g. /product/">
  (pending) .actions [Save pattern (primary-outline)] [Save as no pattern] ; small muted preview text (urlPatternPreviewText, unchanged wording but "Server classification…" → "Showing the saved decision.")
  (pending, undecided) notice notice-warn kind "Undecided" body "Enter a product URL regex or save no pattern before approving."
section "Sample URLs"     right: .stats-inline = four counts as "{n} sample · {a} product now · {b} product with candidate · {c} changed" (small muted); drop the stat tiles
  .url-filter-bar: chips All/Product/Not product/Changed with counts (class chip, active) + search input "Filter paths"
  table: Path · Now · Candidate · Class · Label (unchanged logic; the ✓/– icons keep their titles; changed rows get background var(--accent-soft))
section "Sample labels"
  small muted: "{p} product, {q} not product ({status}). An agent approval needs enough labels of each and a decision that takes every product URL and no not-product URL."
  table: URL · Label · Reason · Decision takes it · (remove)   — inconsistent cell shows badge-bad "No — inconsistent" / "Yes — inconsistent"
  (pending) adder row + [Save labels (primary-outline)]
```

The **Raw payload** details (candidate JSON textarea, "Save full JSON", validation `pre`) is removed from URL-pattern reviews by product-owner decision: the decision editor and the labels editor cover every edit the API accepts for this artifact (`decision`, `current_pattern`, `sample_labels`). `renderNonSchemaReview` renders only the sections above; `rawPayloadOpenReviewId`, `rawPayloadToggled` and the `.raw-payload` CSS are deleted. `saveCandidate()` stays for the product-schema JSON tab only.

Labels in selects render "Product" / "Not product" (`displayName`); the persisted values stay `PRODUCT` / `NOT_PRODUCT`.

## 9. Keyboard, focus, accessibility

- Every icon button has `aria-label` and `title`. Every tone also carries words ("Failed check", "Note", "Blocked by a review"), never colour alone.
- Tabs: `button.tab[aria-selected]` plus `role="tablist"` on `.tabs`; arrow keys are not required.
- Queue links, the Sources link, group heads and the history `summary` are focusable in document order; polls keep focus through `replaceKeepingFocus`.
- The action panel focuses its textarea or confirm button on open (as today). Esc cancels, Ctrl/Cmd+Enter confirms.
- Reduced motion: no transitions other than the chevron rotation, which is disabled under `prefers-reduced-motion`.
- Contrast: all token pairs above were checked ≥ 4.5:1 for text on their soft backgrounds in both themes; do not lighten `--muted`.

## 10. Responsive

Keep today's breakpoints with the new widths: below 1320 px (sidebar open) and below 1060 px (collapsed) the workbench stacks (`grid-template-columns: minmax(0, 1fr)`, preview `position: static`, iframe `height: 62vh; min-height: 460px`); below 900 px the sidebar stacks above the detail with `.queue { max-height: 40vh }` and `.detail { padding: var(--s-4) }`; below 720 px `.rule-grid, .selector-row, .fallback` go single column and the header wraps. The Sources table scrolls horizontally inside its card below 1100 px (`.table-card { overflow-x: auto }`).

## 11. Remove or merge (with the reason)

Sidebar / header
- "Queue auto refresh active" status + green dot — constant text says nothing; the slot is now empty unless something is wrong.
- "Dark mode"/"Light mode" text button — an icon button with a label does the same in 34 px.
- Sidebar group "Domains waiting for an operator" (`renderDomainsNeedingOperator`) — moved into the source's status line on the Sources page.
- Sidebar group "Pages missing a required value" (`renderMissingValueSources`, `renderMissingValuePages` in the sidebar) — moved under the source's row on the Sources page; same handlers.
- Queue item's second meta line "LLM evaluation: …" — the review page shows the evaluation; the queue shows status.
- The "ListingSource health" queue entry and its meta line — replaced by the header view switch.
- The sidebar's uppercase "Reviews" heading — the header pill names the view; the search input takes its place.

Sources page
- Columns "Blocked URLs", "Scrape failure streak", "Last error kind", "Held removals" — merged into Status lines.
- "Blocked by review" badge in the name cell and the amber row highlight — the Status line and the group say it.
- Raw `url_error_kinds` codes — reworded; the three kinds shown elsewhere are deduplicated.
- The subtitle sentence — dropped.
- `None` placeholders — `—`.

Review page
- `review-topbar` meta "Updated 1m ago" alone — now "opened … · edited …" in one meta line.
- Separate cards for human-required, repair request, candidate error, matrix notice, refresh banner, edit refusal/notice — one notice component.
- "Product View" heading, "Reviewer added page · live HTML" caption, the duplicated URL link row, the "Use review URL" button — folded into the two-row toolbar (7.1).
- "Highlight legend" section + `renderSchemaLegend` — the inspector labels highlighted elements; `legend-swatch` CSS goes too.
- "Highlight field" and "Use clicked element" buttons — eye icon on the input and the picked box.
- Per-fallback "Use clicked" and "Highlight" text buttons, "Add clicked selector" — icons and the picked box.
- "Re-evaluate" and "Discard edits" row at the bottom of the Selector tab — Evaluate lives in the Data tab / notices; Discard shows only while dirty next to Save.
- Section heading "Field" above a label "Field" — section is "Rule".
- Uppercase check and notice codes as badge text (`BLANK`, `FOREIGN_PRICE_CURRENCY`) — short words; code in `title` only is unnecessary too, so drop it.
- Per-page bullet lists under "Plausibility" repeating "Source listing ID is blank" six times — grouped by finding.
- The sentence "Schema 1, Schema 2 failed plausibility checks, which keeps a generated schema out of auto-approval." — one fixed sentence under "Failed checks".
- Diff status badges `same` (green) — green meant nothing; "same" is muted text.
- URL-pattern stat tiles — four numbers become one line; the chips carry the counts already.
- "Raw payload" details on URL-pattern reviews — removed (product-owner decision); product-schema reviews keep the JSON tab.
- Review name repeated in action-panel titles — the page title shows it.
- "version N" in history rows — "vN".
- CSS: `.pill`, `.badge-changed`, `.badge-missing`, `.badge-info`, `--info`, `--info-soft`, `.hint-card`, `.finding-hint`, `.success-hint`, `.error-panel`, `.refresh-banner`, `.llm-risks`, `.plausibility-*`, `.url-pattern-stat*`, `.schema-legend`, `.legend-*`, `.extracted-page*`, `.raw-attribute-card`, `.additional-selector-*`, `.rule-options-card`, `.source-health-*`, `.listing-source-*`, `.queue-*`, `.item`, `.missing-value-page`, `.disclosure-icon`, `.expandable-text > summary::before` (use the chevron SVG or the browser marker), `.tab-panel > h3` — gone with their markup.
- JS made dead: `renderSchemaLegend`, `renderDomainsNeedingOperator` (logic folds into the status line), `urlPatternStat`, `renderUrlPatternStats`, `scrapeFailureStreakText` (inline), `renderBlockedUrlsCell`, `renderUrlErrorKindsCell`, `renderHeldRemovalsCell` (replaced by `renderSourceStatus`, `renderRecentErrors`), `usePickedForAdditionalSelector`, `addPickedAdditionalSelector` stays (used by the picked box), `renderNoticeBadges` → `noticeBadge`, `renderFindingBadges` → `findingBadge`, the `localStorage.removeItem('crawlerReviewCollapsedListingSourceGroups'/'…ShopGroups')` migration lines (old keys from before this branch; remove).

## 12. Product-owner decisions (applied in this spec)

- Raw payload on URL-pattern reviews: **removed** (section 8).
- "Highlight all fields": **kept**, as the quiet "Highlight all" button at the right end of the schema toolbar (7.3); it highlights every rule of the selected schema in the preview, each element labelled by the inspector.
- Diff tab: **kept**, as the third inspector tab (7.5), with its two selects and muted "same" rows.
- API fields: **added** as section 5a (`blocking_review_id`, `held_since`, `scrape_failing_since`, `scrape_paused_until`); "next scrape" of a healthy source is not stored and is not added.

Open for later (not in this spec): a `title_fallback_used` flag on missing-value pages.

## 13. Documentation

In `docs/crawler/operations.md`, the review-service paragraph that describes the health view and the queue (the sentence group starting "While no review is selected, the console shows the list as the ListingSource health view…" and "…the console's queue offers to add one to a review") is updated to: the console opens on its **Sources** view, switched in the header with the **Reviews** view (the queue and the selected review; the fragment `#reviews` opens it without a selection) — one row per ListingSource with a worded status (a domain needing an operator, URLs blocked by a pending review with how long, the longest scrape failure streak with its error kind, held removals, pages missing a required value), its pending reviews, remaining URL error kinds, last successful scrape and next crawl, grouped into sources needing attention, healthy and crawling-disabled; the Sources view lists the missing-value pages under their source and offers to add one to a review; domains recorded `NEEDS_OPERATOR` appear there (replace "the console lists it above the review queue" in the manual-authoring section). The `#review=…` fragment paragraph gains: "`#sources` and `#reviews` open the two views without a review; without a fragment the console opens on Sources." In `docs/crawler/extraction-and-review.md` nothing changes except "the console's **Removed page** tab" → "**Removed pages** tab". Do not add screenshots or CSS details to the docs.

## 14. Order of work

1. `styles.css`: replace wholesale with sections 2–3 and 10. Keep file header comment style. Load the page; everything will look broken but readable.
2. API (5a): baseline-migration column, scraper writes, reader, model, tests, docs — a separate commit, mergeable on its own; the frontend tolerates the fields being absent (an undefined `held_since` renders as null).
3. `index.html`: header with the view switch, sidebar markup (4.1, 4.4), theme toggle icon.
4. `app.js` navigation (4.2): `selectionFromUrl().view`, `setView`, `updateViewSwitch`, `selectionUrlHash`, the empty Reviews state; then `updateReloadState`, `applyTheme`, `showQueueList`, `renderReviewGroups`, `renderReviewQueueItem`, badge tone tables (3.3).
5. Sources page (5): `renderSourceHealth`, `renderSourceHealthRow` → `renderSourceRow`, `renderSourceStatus`, `renderRecentErrors`, `errorKindLabel`, `untilText`, stat tiles, groups, the missing-value sub-row; delete the two sidebar groups and `renderSourceHealthEntry`.
6. Review frame (6): `renderDetail`, `renderApprovalPanel`, notices, `renderEditMessages`, `updateRefreshBanner`, action panel, history, LLM card.
7. Preview toolbar (7.1), tabs (7.2).
8. Selector tab (7.3): schemas, rule, fallbacks, picked box, raw attributes; remove legend.
9. Data tab (7.4): coverage, grouped checks and notes, extracted values with short labels.
10. Diff, Removed pages, JSON (7.5–7.7).
11. URL-pattern body (8) without Raw payload.
12. Dead-code pass over `app.js` and `styles.css` (11); search for every removed class name and function.
13. Docs (13).
14. Check both themes at 1440×900 and 1024×768 against the mockup screenshots; check keyboard paths: Tab from the header pills into the queue, open a review, tab to Approve, open the action panel, Esc, Ctrl+Enter; browser back from a review returns to the view it came from; `#sources`, `#reviews` and `#review=<id>` each load directly.

## 15. Acceptance checklist

- [ ] Only Approve is a filled accent button; on closed reviews no filled button exists.
- [ ] Red appears only for: failed checks, "Failing to scrape", "Domain needs an operator", Rejected, Needs a human, Refused, Cannot approve, Reject/Delete buttons. Amber only for: Pending, Blocked by a review, removals held, Waiting for repair, Notes/Foreign currency, Unsaved edits, Not evaluated. Green only for: Approved, Scraping, coverage complete, Saved, All clear.
- [ ] A foreign-currency note never shares a container, colour or badge style with a failed check.
- [ ] No uppercase enum code is visible anywhere in the UI (search the rendered DOM for `_[A-Z]` inside badges).
- [ ] Every long URL/ID is truncated with the full value in `title` and as the link target.
- [ ] The same fact is not shown twice on the Sources page; `None` does not appear.
- [ ] `grep -n "unmapped" app.js` finds nothing.
- [ ] `app.js` has no function that is not called; `styles.css` has no selector that no markup uses.
- [ ] Fragment navigation, unsaved-edit confirmation, polling, focus keeping, session prompt and all escaping behave as before; `#sources` / `#reviews` / `#review=<id>` round-trip through back and forward.
- [ ] The Sources page shows "since" on failing and held lines and "retries in …" on a paused domain, and "Blocked by a review" links straight to the blocking review.
- [ ] URL-pattern reviews show no Raw payload; product-schema reviews keep the JSON tab.
