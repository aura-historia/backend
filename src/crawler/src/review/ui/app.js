// `sources` (the landing view) or `reviews`; the URL fragment decides, see `selectionFromUrl`.
let currentView = 'sources';
let selectedId = null;
let selectedDetail = null;
let selectedMatrix = null;
let selectedMatrixFailure = '';
let selectedMatrixMissing = false;
let selectedSchemaIndex = 0;
let compareSchemaIndex = 1;
let selectedField = 'title';
let selectedPageId = null;
let activeWorkbenchPanel = 'selector';
let pickedSelector = '';
let pickedSample = '';
let editRefusal = null;
// The confirmation of the selected review's last successful request, shown until the next one.
let editNotice = null;
// The in-page panel a review mutation waits in for the reviewer's confirmation and notes.
let pendingAction = null;
// How many matrix refreshes this console waits for, per review: a refresh moves the review's
// `updated`, which is not a change made elsewhere.
const matrixRefreshesInFlight = new Map();
let rawAttributeKeys = null;
let dirty = false;
let selectedReviewUpdatedAt = null;
let selectedReviewStatus = null;
let selectedReviewNeedsRefresh = false;
let selectedReviewRefreshReason = '';
let sidebarCollapsed = localStorage.getItem('crawlerReviewSidebarCollapsed') === 'true';
let theme = document.documentElement.dataset.theme || localStorage.getItem('crawlerReviewTheme') || 'light';
let reviewSearchTerm = '';
// A status the header shows until it is replaced: `Signed out`, or why the queue refresh failed.
let transientStatus = null;
// Shop groups start collapsed; the reviewer's expanded ones are remembered per browser.
const EXPANDED_GROUPS_KEY = 'crawlerReviewExpandedListingSourceGroups';
let expandedListingSourceGroups = new Set(readStoredList(EXPANDED_GROUPS_KEY));
// The review whose group the queue last opened for it, so a reviewer can collapse it again.
let reviewWhoseGroupWasOpened = null;
let reviewAuthToken = sessionStorage.getItem('crawlerReviewAuthToken') || '';
let reviewSessionEstablished = false;
let previewUrlOverride = '';
// The page and schema a URL fragment asks for, applied when its review has loaded.
let requestedSelection = null;
// The sample labels being edited on the selected URL-pattern review, or null while the stored
// labels are shown unedited.
let sampleLabelDraft = null;

const selectorFields = [
    'source_listing_id', 'title', 'description', 'price', 'price_estimate_min',
    'price_estimate_max', 'state', 'images'
];
const optionalFields = new Set([
    'description', 'price', 'price_estimate_min', 'price_estimate_max'
]);
const schemaHighlightColors = {
    source_listing_id: '#2563eb',
    title: '#7c3aed',
    description: '#0891b2',
    price: '#16a34a',
    price_estimate_min: '#65a30d',
    price_estimate_max: '#0d9488',
    state: '#f97316',
    images: '#db2777'
};
const rawAttributeHighlightColor = '#64748b';
// A raw attribute rule is edited as the field `raw_attributes.<key>`, as `/schema-field` names it.
const RAW_ATTRIBUTE_FIELD_PREFIX = 'raw_attributes.';
// Pages a reviewer may remove from a review: the ones they added and the ones waiting on it.
const REMOVABLE_PAGE_ROLES = new Set(['REVIEWER_ADDED', 'WAITING_PAGE']);
const PAGE_ROLE_LABELS = {PRIMARY: 'Primary', SEED: 'Seed', REVIEWER_ADDED: 'Added', WAITING_PAGE: 'Waiting'};

applyTheme(theme);

// Reviewers count schemas from 1; the URL fragment, the API and the matrix keep the 0-based
// schema index, so links and agents stay valid.
function schemaLabel(index) {
    return `Schema ${Number(index) + 1}`;
}

function applyTheme(nextTheme) {
    theme = nextTheme === 'dark' ? 'dark' : 'light';
    document.documentElement.dataset.theme = theme;
    localStorage.setItem('crawlerReviewTheme', theme);
    const toggle = document.getElementById('themeToggle');
    if (toggle) {
        const label = theme === 'dark' ? 'Switch to light mode' : 'Switch to dark mode';
        toggle.innerHTML = theme === 'dark' ? sunIcon() : moonIcon();
        toggle.setAttribute('aria-label', label);
        toggle.title = label;
    }
}

function toggleTheme() {
    applyTheme(theme === 'dark' ? 'light' : 'dark');
}

function setReviewAuthToken(token) {
    reviewAuthToken = token || '';
    reviewSessionEstablished = false;
    if (reviewAuthToken) {
        if (transientStatus?.text === 'Signed out') transientStatus = null;
        sessionStorage.setItem('crawlerReviewAuthToken', reviewAuthToken);
    } else {
        sessionStorage.removeItem('crawlerReviewAuthToken');
    }
    updateSignOutButton();
}

// Signing out matters only where the console asked for a review token: it ends the browser
// session and forgets the token. The tokenless local console has nothing to sign out of.
function updateSignOutButton() {
    const button = document.getElementById('signOutButton');
    if (button) button.hidden = !reviewAuthToken;
}

async function clearReviewAuthToken() {
    if (reviewAuthToken) {
        await fetch('/api/session/logout', {
            method: 'POST',
            headers: {authorization: `Bearer ${reviewAuthToken}`},
            credentials: 'same-origin'
        }).catch(() => {});
    }
    setReviewAuthToken('');
    transientStatus = {text: 'Signed out', className: 'system-status'};
    updateReloadState();
}

// A request the browser could not send, or whose answer never arrived, carries no HTTP status;
// `apiErrorMessage` reports it as a network failure.
async function fetchOrNetworkError(path, options) {
    try {
        return await fetch(path, options);
    } catch (cause) {
        const error = new Error(String((cause && cause.message) || cause));
        error.network = true;
        throw error;
    }
}

async function requestReviewAuthToken() {
    const token = window.prompt('Enter crawler review token');
    if (token === null || !token.trim()) return false;
    const response = await fetchOrNetworkError('/api/session', {
        method: 'POST',
        headers: {authorization: `Bearer ${token.trim()}`},
        credentials: 'same-origin'
    });
    if (!response.ok) return false;
    setReviewAuthToken(token.trim());
    reviewSessionEstablished = true;
    return true;
}

async function ensureReviewSession() {
    if (!reviewAuthToken || reviewSessionEstablished) return;
    const response = await fetchOrNetworkError('/api/session', {
        method: 'POST',
        headers: {authorization: `Bearer ${reviewAuthToken}`},
        credentials: 'same-origin'
    });
    if (!response.ok) return false;
    reviewSessionEstablished = true;
    return true;
}

async function api(path, options = {}, allowPrompt = true) {
    await ensureReviewSession();
    const headers = new Headers(options.headers || {});
    headers.set('content-type', 'application/json');
    if (reviewAuthToken) headers.set('authorization', `Bearer ${reviewAuthToken}`);
    const res = await fetchOrNetworkError(path, {...options, headers, credentials: 'same-origin'});
    if (res.status === 401 && allowPrompt && await requestReviewAuthToken()) {
        return api(path, options, false);
    }
    if (!res.ok) {
        const error = new Error(await res.text());
        error.status = res.status;
        error.statusText = res.statusText;
        throw error;
    }
    return res.json();
}

// The machine-readable `reason` of a refused request, such as `NO_MATRIX`, or null.
function refusalReason(error) {
    try {
        return JSON.parse(error.message).reason || null;
    } catch (_) {
        return null;
    }
}

// A 409 that carries `candidate_version` means the review changed under the request: an edit
// based on an older candidate version, or a matrix refresh during which the candidate changed
// or an evaluated page was removed. The request may be repeated once the review is reloaded.
function changedCandidateVersion(error) {
    if (error?.status !== 409) return null;
    try {
        const version = JSON.parse(error.message).candidate_version;
        return Number.isInteger(version) ? version : null;
    } catch (_) {
        return null;
    }
}

// Numbers queue loads, so a poll that answers after a later load, such as the one a verdict
// starts, cannot show the older queue or report a stale status change of the selected review.
let queueLoadSequence = 0;

async function loadReviews() {
    const sequence = ++queueLoadSequence;
    const overviewLoaded = api('/api/listing_sources').then(
        listingSources => listingSources,
        () => null
    ).then(listingSources => {
        if (sequence !== queueLoadSequence) return;
        latestListingSources = listingSources;
        showQueueList();
        showIdleDetail();
    });
    let reviews;
    try {
        reviews = await api('/api/reviews');
    } catch (error) {
        if (sequence === queueLoadSequence) {
            queueLoadFailed = true;
            showQueueList();
            showIdleDetail();
        }
        throw error;
    }
    if (sequence === queueLoadSequence) {
        queueLoadFailed = false;
        if (transientStatus?.className === 'system-status stale') transientStatus = null;
        latestQueue = reviews;
        showQueueList();
        showIdleDetail();
        updateSelectedReviewFreshness(reviews);
    }
    await overviewLoaded;
    return reviews;
}

// The sidebar is the review queue and shows only in the Reviews view; the header's counts follow
// every poll in both views.
function showQueueList() {
    updateViewSwitch();
    if (currentView !== 'reviews') return;
    showQueue(queueLoadFailed && !latestQueue.length
        ? '<div class="empty">The review queue is unavailable. It is retried every 15 seconds.</div>'
        : renderReviewGroups(latestQueue));
}

// The detail pane shows the Sources page, or the empty Reviews state while no review is selected.
function showIdleDetail() {
    if (selectedId) return;
    if (currentView === 'sources') {
        showSourceHealth();
    } else {
        showReviewsEmpty();
    }
}

// `undefined` until the first poll answers, `null` when the overview could not be read.
let latestListingSources;
let latestQueue = [];
let queueLoadFailed = false;
let shownDetailHtml = '';

// A ListingSource is blocked while URLs of a crawl-enabled one wait on a pending review: a
// pending review nothing waits on, or one of a disabled ListingSource, holds back nothing.
function blockedByReview(source) {
    return Boolean(source.crawl_enabled) && Number(source.blocked_urls) > 0;
}

// The scraper circuit's failure streak of a ListingSource; 0 while it scrapes.
function scrapeFailureStreak(source) {
    return Number(source?.scrape_failure_streak) || 0;
}

// A ListingSource recorded `NEEDS_OPERATOR` is crawled no more until its presentation URL changes
// or its crawling is disabled.
function crawlNeedsOperator(source) {
    return source.last_crawl_outcome === 'NEEDS_OPERATOR';
}

function listingSourceName(source) {
    return String(source.listing_source_name || source.listing_source_id);
}

// What needs an operator: a crawl-enabled ListingSource whose crawl waits for one, URLs blocked
// on a review, a scrape failure streak or held removals.
function needsAttention(source) {
    return Boolean(source.crawl_enabled) && (crawlNeedsOperator(source)
        || Number(source.blocked_urls) > 0
        || scrapeFailureStreak(source) > 0
        || Number(source.held_removal_urls) > 0);
}

// Needs-operator ListingSources first; then the blocked ones, the longest failure streaks, the
// held removals and finally by name.
function compareSourceHealth(a, b) {
    return Number(crawlNeedsOperator(b)) - Number(crawlNeedsOperator(a))
        || Number(blockedByReview(b)) - Number(blockedByReview(a))
        || scrapeFailureStreak(b) - scrapeFailureStreak(a)
        || Number(Number(b.held_removal_urls) > 0) - Number(Number(a.held_removal_urls) > 0)
        || listingSourceName(a).localeCompare(listingSourceName(b));
}

function pendingReviewCount() {
    return latestQueue.filter(review =>
        review.status === 'PENDING_REVIEW' && review.listing_source_crawl_enabled !== false).length;
}

function sourcesNeedingAttention() {
    return Array.isArray(latestListingSources) ? latestListingSources.filter(needsAttention).length : 0;
}

// The header's two views, with the one number an operator wants from each: the sources that need
// them and the reviews waiting for a verdict. Both are amber and omitted at 0.
function updateViewSwitch() {
    const tabs = [
        ['viewTabSources', 'sources', 'Sources', sourcesNeedingAttention()],
        ['viewTabReviews', 'reviews', 'Reviews', pendingReviewCount()]
    ];
    for (const [id, view, label, count] of tabs) {
        const tab = document.getElementById(id);
        if (!tab) continue;
        const active = currentView === view;
        tab.classList.toggle('active', active);
        if (active) tab.setAttribute('aria-current', 'page');
        else tab.removeAttribute('aria-current');
        const html = `${label}${count ? ` ${badge(String(count), 'warn')}` : ''}`;
        if (tab.innerHTML !== html) tab.innerHTML = html;
    }
}

// The view is a function of the URL fragment. The Reviews view shows the queue sidebar; the
// Sources view takes the full width.
function setView(view) {
    currentView = view === 'reviews' ? 'reviews' : 'sources';
    document.getElementById('appShell')?.classList.toggle('view-sources', currentView === 'sources');
    document.getElementById('detail')?.classList.toggle('sources', currentView === 'sources' && !selectedId);
    updateViewSwitch();
    showQueueList();
}

// A render that changes nothing leaves the view alone, and one that does, such as a poll that
// only moves an elapsed time, keeps the reviewer's keyboard focus on the link they were on.
function showIdleHtml(html) {
    const detail = document.getElementById('detail');
    detail.classList.toggle('sources', currentView === 'sources');
    if (html === shownDetailHtml && detail.childElementCount) return;
    replaceKeepingFocus(detail, html);
    shownDetailHtml = html;
}

function showSourceHealth() {
    if (selectedId) return;
    showIdleHtml(renderSourceHealth());
}

function showReviewsEmpty() {
    if (selectedId) return;
    const pending = pendingReviewCount();
    const needing = sourcesNeedingAttention();
    const detail = latestQueue.length || !queueLoadFailed
        ? `Pick a review in the queue. ${pending ? `${pending} review${pending === 1 ? '' : 's'} pending` : 'Nothing is pending'}${needing ? `; ${needing} source${needing === 1 ? '' : 's'} need${needing === 1 ? 's' : ''} you` : ''}.`
        : 'The review queue is unavailable. It is retried every 15 seconds.';
    showIdleHtml(`<div class="empty-detail">
      <strong>No review selected</strong>
      <span>${escapeHtml(detail)}</span>
      <span class="actions"><a class="view-tab" href="#sources">See what needs you</a></span>
    </div>`);
}

function renderSourceHealth() {
    if (latestListingSources === undefined) return '<div class="empty">Loading sources…</div>';
    if (latestListingSources === null) {
        return `<div class="page"><h1>Sources</h1><div class="notice notice-bad" role="alert"><span class="notice-kind">Sources unavailable</span><span>The source overview could not be read; it is retried with the next queue refresh.</span></div></div>`;
    }
    const sources = latestListingSources.slice().sort(compareSourceHealth);
    const needing = sources.filter(needsAttention);
    const crawling = sources.filter(source => source.crawling_since).length;
    const scraping = sources.filter(source => source.scraping_since).length;
    const meta = [`${sources.length} source${sources.length === 1 ? '' : 's'}`,
        needing.length ? `${needing.length} need${needing.length === 1 ? 's' : ''} you` : 'nothing needs you',
        crawling ? `${crawling} crawling now` : '', scraping ? `${scraping} scraping now` : ''].filter(Boolean).join(' · ');
    const groups = [
        ['Needs attention', needing, false],
        ['Healthy', sources.filter(source => source.crawl_enabled && !needsAttention(source)), false],
        ['Crawling disabled', sources.filter(source => !source.crawl_enabled), true]
    ];
    return `<div class="page">
      <div class="page-head"><div>
        <h1>Sources</h1>
        <div class="page-meta"><span>${escapeHtml(meta)}</span></div>
      </div></div>
      ${sources.length ? `${renderSourceStats(sources)}
      <div class="card table-card">
        <table>
          <thead><tr>
            <th scope="col">Source</th>
            <th scope="col">Status</th>
            <th scope="col">Reviews</th>
            <th scope="col">Recent errors</th>
            <th scope="col">Last scrape</th>
            <th scope="col">Next crawl</th>
          </tr></thead>
          <tbody>${groups.filter(([, members]) => members.length).map(([title, members, disabled]) =>
              `<tr class="group-row"><td colspan="6">${escapeHtml(title)}</td></tr>${members.map(source => renderSourceRow(source, disabled)).join('')}`).join('')}</tbody>
        </table>
      </div>` : '<div class="empty">No source is registered.</div>'}
    </div>`;
}

// The numbers behind "what needs me", each shown only while it is not 0.
function renderSourceStats(sources) {
    const enabled = sources.filter(source => source.crawl_enabled);
    const sum = (list, field) => list.reduce((total, source) => total + (Number(source[field]) || 0), 0);
    const operator = enabled.filter(crawlNeedsOperator).length;
    const failing = enabled.filter(source => scrapeFailureStreak(source) > 0).length;
    const tiles = [
        [sum(enabled, 'blocked_urls'), 'URLs blocked by a review', 'URLs blocked by a review', 'warn'],
        [operator, 'Crawl needs an operator', 'Crawls need an operator', 'bad'],
        [failing, 'Source failing to scrape', 'Sources failing to scrape', 'bad'],
        [sum(sources, 'held_removal_urls'), 'Removal held', 'Removals held', 'warn'],
        [sum(enabled, 'missing_value_urls'), 'Page missing a value', 'Pages missing a value', '']
    ].filter(([count]) => count > 0);
    if (!tiles.length) {
        return '<div class="notice notice-ok"><span class="notice-kind">All clear</span><span>No source needs you. Every crawl-enabled source is scraping.</span></div>';
    }
    return `<div class="stats">${tiles.map(([count, one, many, tone]) =>
        `<div class="stat"><strong>${escapeHtml(String(count))}</strong><span><span class="dot${tone ? ` dot-${tone}` : ''}"></span>${escapeHtml(count === 1 ? one : many)}</span></div>`).join('')}</div>`;
}

function renderSourceRow(source, disabled) {
    const id = source.listing_source_id;
    const root = source.crawl_root_url || 'No crawl root';
    const urls = Number(source.product_urls) || 0;
    const pages = expandedMissingValueSources.get(id);
    return `<tr${disabled ? ' class="disabled"' : ''}>
      <td><div class="cell-main" title="${escapeHtmlAttr(listingSourceName(source))}">${escapeHtml(listingSourceName(source))}</div>
        <div class="cell-sub" title="${escapeHtmlAttr(root)}">${escapeHtml([root, `${urls} URL${urls === 1 ? '' : 's'}`].join(' · '))}</div></td>
      <td>${renderSourceStatus(source)}</td>
      <td>${renderPendingReviewsCell(source)}</td>
      <td>${renderRecentErrors(source)}</td>
      <td>${source.last_successful_scrape ? timeBadge(source.last_successful_scrape, 'Last successful scrape') : '<span class="muted">Never</span>'}</td>
      <td>${renderNextCrawl(source)}</td>
    </tr>${pages ? `<tr class="subrow"><td></td><td colspan="5">${renderMissingValuePages(id, pages)}</td></tr>` : ''}${openMassRemovalConfirmations.has(id) ? `<tr class="subrow"><td></td><td colspan="5">${renderMassRemovalConfirmation(id)}</td></tr>` : ''}`;
}

function issueLine(tone, headline, detail) {
    return `<div class="issue"><span class="dot${tone ? ` dot-${tone}` : ''}"></span><div>${headline}${detail ? `<div class="muted">${detail}</div>` : ''}</div></div>`;
}

// The worded status of a source, one line per issue, in the order of severity.
function renderSourceStatus(source) {
    const lines = [];
    const id = escapeHtmlAttr(source.listing_source_id);
    if (source.crawl_enabled) {
        if (crawlNeedsOperator(source)) {
            lines.push(issueLine('bad', 'Crawl needs an operator',
                `${escapeHtml(errorKindLabel(source.last_crawl_error_kind))} · ${escapeHtml(String(source.crawl_failure_count))} failed crawls — fix the presentation URL in the admin API or disable crawling`));
        }
        const blocked = Number(source.blocked_urls) || 0;
        if (blocked > 0) {
            const total = Number(source.product_urls) || 0;
            const headline = source.blocking_review_id && REVIEW_ID_PATTERN.test(source.blocking_review_id)
                ? `<a href="#${new URLSearchParams({review: source.blocking_review_id})}">Blocked by a review</a>`
                : 'Blocked by a review';
            lines.push(issueLine('warn', headline,
                `${blocked === total ? `All ${blocked} URLs wait on it` : `${blocked} of ${total} URLs wait on it`}${source.blocked_since ? ` · since ${timeBadge(source.blocked_since, 'Blocked since', true)}` : ''}`));
        }
        if (scrapeFailureStreak(source) > 0) {
            const paused = source.scrape_paused_until
                ? ` · retries ${Date.parse(source.scrape_paused_until) <= Date.now() ? 'now' : untilText(source.scrape_paused_until)}`
                : '';
            lines.push(issueLine('bad', 'Failing to scrape',
                `${scrapeFailureStreak(source)} in a row · ${escapeHtml(errorKindLabel(source.last_scrape_error_kind))}${source.scrape_failing_since ? ` · since ${timeBadge(source.scrape_failing_since, 'Failing since', true)}` : ''}${paused}`));
        }
        const held = Number(source.held_removal_urls) || 0;
        const confirmed = Boolean(source.mass_removal_confirmed_until);
        if (held > 0) {
            const offer = confirmed ? '' : ` · <button class="toggle-link" aria-expanded="${openMassRemovalConfirmations.has(source.listing_source_id)}" onclick="toggleMassRemovalConfirmation('${id}')" type="button">Confirm bulk removal</button>`;
            lines.push(issueLine('warn', `${held} removal${held === 1 ? '' : 's'} held`,
                `Kept by the mass-removal guard${source.held_since ? ` · held ${timeBadge(source.held_since, 'Last held')}` : ''}${offer}`));
        }
        if (confirmed) {
            lines.push(issueLine('ok', 'Bulk removal confirmed',
                `Removals are captured until ${escapeHtml(formatDate(source.mass_removal_confirmed_until))} (${escapeHtml(untilText(source.mass_removal_confirmed_until))}) · by ${escapeHtml(actorWord(source.mass_removal_confirmed_by))} · <button class="toggle-link" onclick="revokeMassRemovalConfirmation('${id}')" type="button">Revoke</button>`));
        }
    }
    const missing = Number(source.missing_value_urls) || 0;
    if (source.crawl_enabled && missing > 0) {
        lines.push(issueLine('', `<button class="toggle-link" aria-expanded="${expandedMissingValueSources.has(source.listing_source_id)}" onclick="toggleMissingValuePages('${id}')" type="button">${missing} page${missing === 1 ? '' : 's'} missing a required value</button>`, ''));
    }
    const running = [];
    if (source.crawling_since) running.push(issueLine('ok', 'Crawling now', `started ${timeBadge(source.crawling_since, 'Crawling since')}`));
    if (source.scraping_since) running.push(issueLine('ok', 'Scraping now', `started ${timeBadge(source.scraping_since, 'Scraping since')}`));
    if (!lines.length && !running.length) {
        lines.push(!source.crawl_enabled ? issueLine('', 'Crawling disabled', '')
            : source.last_successful_scrape ? issueLine('ok', 'Scraping', '') : issueLine('', 'Not scraped yet', ''));
    }
    lines.push(...running);
    return `<div class="issues">${lines.join('')}</div>`;
}

// The last error kinds of the ListingSource's product URLs that no status line shows already.
const STATUS_ERROR_KINDS = new Set(['PendingSchemaReview', 'MassRemovalHeld', 'MissingRequiredValue']);

function renderRecentErrors(source) {
    const kinds = (source.url_error_kinds || []).filter(entry => !STATUS_ERROR_KINDS.has(entry.kind));
    if (!kinds.length) return '<span class="muted">—</span>';
    return `<span class="muted">${kinds.map(entry => `${escapeHtml(String(entry.count))} ${escapeHtml(errorKindLabel(entry.kind))}`).join(' · ')}</span>`;
}

function errorKindLabel(kind) {
    const text = String(kind || '');
    const known = {
        HttpError: 'HTTP error',
        RawCaptureFailed: 'capture failed',
        Timeout: 'timeout',
        BotProtection: 'bot protection',
        SourceListingIdConflict: 'identity conflict'
    };
    if (known[text]) return known[text];
    if (/^HTTP_/i.test(text)) return text.replace(/_/g, ' ');
    return text ? displayName(text).toLowerCase() : 'unknown failure';
}

// The ListingSource's next crawl.
function renderNextCrawl(source) {
    const next = source.next_crawl_at;
    if (!next) return '<span class="muted">—</span>';
    const text = Date.parse(next) <= Date.now() ? 'due' : untilText(next);
    return `<span class="time" title="${escapeHtmlAttr(`Next crawl: ${formatDate(next)}`)}">${escapeHtml(text)}</span>`;
}

// The count links to the review the source is blocked on, else to its first pending review in
// the queue.
function renderPendingReviewsCell(source) {
    const pending = Number(source.pending_reviews) || 0;
    if (!pending) return '<span class="muted">—</span>';
    const text = `${pending} pending`;
    const blocking = REVIEW_ID_PATTERN.test(source.blocking_review_id || '') ? source.blocking_review_id : null;
    const review = latestQueue.find(r => r.listing_source_id === source.listing_source_id && r.status === 'PENDING_REVIEW');
    const target = blocking || review?.review_id;
    return target ? `<a href="#${new URLSearchParams({review: target})}">${escapeHtml(text)}</a>` : escapeHtml(text);
}

// A refresh that changes nothing leaves the queue's elements alone, and one that does keeps the
// reviewer's scroll position and keyboard focus in it, such as on a group just toggled or a
// review just opened from the keyboard.
let shownQueueHtml = '';

function showQueue(html) {
    const list = document.getElementById('reviewList');
    if (html === shownQueueHtml && list.childElementCount) return;
    const scrollTop = list.scrollTop;
    replaceKeepingFocus(list, html);
    list.scrollTop = scrollTop;
    shownQueueHtml = html;
}

// Replaces what `container` shows with `html` and moves the keyboard focus it held to the
// control of the new render that opens or toggles the same thing.
function replaceKeepingFocus(container, html) {
    const focused = container.contains(document.activeElement) ? controlKey(document.activeElement) : null;
    container.innerHTML = html;
    if (focused) {
        Array.from(container.querySelectorAll('a, button'))
            .find(control => controlKey(control) === focused)
            ?.focus({preventScroll: true});
    }
}

// A control is named by what it opens or toggles, which a render does not change.
function controlKey(control) {
    return ['href', 'aria-controls', 'onclick']
        .map(name => control.getAttribute(name) || '')
        .join(' ');
}

function renderReviewGroups(reviews) {
    const query = reviewSearchTerm.trim().toLowerCase();
    const groups = [];
    const byListingSource = new Map();
    for (const review of reviews) {
        if (query && !reviewMatchesSearch(review, query)) continue;
        const listingSourceKey = review.listing_source_id || 'unknown-listing-source';
        if (!byListingSource.has(listingSourceKey)) {
            const group = {
                listingSourceId: listingSourceKey,
                listingSourceName: review.listing_source_name || review.listing_source_id || 'Unknown listing source',
                crawlEnabled: review.listing_source_crawl_enabled !== false,
                reviews: []
            };
            byListingSource.set(listingSourceKey, group);
            groups.push(group);
        }
        byListingSource.get(listingSourceKey).reviews.push(review);
    }

    if (!groups.length) {
        return query
            ? `<div class="empty">No reviews match “${escapeHtml(reviewSearchTerm.trim())}”.</div>`
            : '<div class="empty">No review yet.</div>';
    }

    return groups.map(group => {
        const pendingCount = group.reviews.filter(review => review.status === 'PENDING_REVIEW').length;
        if (selectedId && selectedId !== reviewWhoseGroupWasOpened
            && group.reviews.some(review => review.review_id === selectedId)) {
            reviewWhoseGroupWasOpened = selectedId;
            expandedListingSourceGroups.add(group.listingSourceId);
            storeExpandedGroups();
        }
        const collapsed = !expandedListingSourceGroups.has(group.listingSourceId);
        const listId = `queueGroup-${escapeHtmlAttr(group.listingSourceId)}`;
        return `<div class="group">
          <button type="button" class="group-head" aria-expanded="${!collapsed}" aria-controls="${listId}" onclick="toggleListingSourceGroup('${escapeHtmlAttr(group.listingSourceId)}')">
            <span class="chev" aria-hidden="true">${chevronIcon()}</span>
            <span class="group-text">
              <span class="nav-row">
                <span class="nav-title" title="${escapeHtmlAttr(group.listingSourceName)}">${escapeHtml(group.listingSourceName)}</span>
                ${pendingCount ? badge(`${pendingCount} pending`, 'warn') : '<span class="nav-meta">Reviewed</span>'}
              </span>
              <span class="nav-meta">${escapeHtml(reviewCountsByArtifactType(group.reviews))}${group.crawlEnabled ? '' : ' · crawling disabled'}</span>
            </span>
          </button>
          <div class="group-list ${collapsed ? 'collapsed' : ''}" id="${listId}">
            ${group.reviews.map(renderReviewQueueItem).join('')}
          </div>
        </div>`;
    }).join('');
}

const ARTIFACT_TYPE_COUNT_NAMES = {
    PRODUCT_SCHEMA: ['product schema', 'product schemas'],
    URL_PATTERN: ['URL pattern', 'URL patterns']
};

// How many of `reviews` there are of each artifact type, such as "2 product schemas · 1 URL
// pattern", in the order the types first appear.
function reviewCountsByArtifactType(reviews) {
    const counts = new Map();
    for (const review of reviews) {
        counts.set(review.artifact_type, (counts.get(review.artifact_type) || 0) + 1);
    }
    return [...counts].map(([type, count]) => {
        const [one, many] = ARTIFACT_TYPE_COUNT_NAMES[type] || [displayName(type), displayName(type)];
        return `${count} ${count === 1 ? one : many}`;
    }).join(' · ');
}

// A page whose last scrape left a required value empty opens and joins no review, such as a
// removal page that keeps an empty title element. A source's status line lists such pages, so a
// reviewer adds one to the product-schema review (opened for it when none is pending) and
// declares a removed-page template for it there.
const expandedMissingValueSources = new Map();

// The scrape's outcome message names the blank values in backticks, such as "`title` is blank";
// a message that names none is shown as it is.
function missingValuesText(message) {
    const fields = [...String(message || '').matchAll(/`([^`]+)`/g)].map(match => displayName(match[1]));
    if (!fields.length) return String(message || '').trim();
    return `${fields.join(' and ')} ${fields.length === 1 ? 'is' : 'are'} empty`;
}

// The handler names a page by its position in the listed pages, never by its URL: a shop's URL
// may hold a quote, which the browser decodes from an HTML entity before the handler runs, so a
// URL in the handler's source would let the shop run script in the console.
function renderMissingValuePages(listingSourceId, pages) {
    if (pages.error) return `<div class="subpages"><div class="subpage muted">${escapeHtml(pages.error)}</div></div>`;
    return `<div class="subpages">${pages.map((page, index) => {
        const label = missingValuesText(page.missing);
        return `<div class="subpage">${pageUrlLink(page.url)}${label ? ` <span class="muted">${escapeHtml(label)}</span>` : ''}
        <button onclick="reviewMissingValuePage('${escapeHtmlAttr(listingSourceId)}', ${index})" type="button">Add to review</button>
      </div>`;
    }).join('')}</div>`;
}

async function toggleMissingValuePages(listingSourceId) {
    if (expandedMissingValueSources.has(listingSourceId)) {
        expandedMissingValueSources.delete(listingSourceId);
    } else {
        try {
            const answer = await api(`/api/listing-sources/${encodeURIComponent(listingSourceId)}/missing-value-pages`);
            expandedMissingValueSources.set(listingSourceId, answer.pages || []);
        } catch (error) {
            expandedMissingValueSources.set(listingSourceId, {error: apiErrorMessage(error)});
        }
    }
    showSourceHealth();
}

async function reviewMissingValuePage(listingSourceId, index) {
    const pages = expandedMissingValueSources.get(listingSourceId);
    const url = Array.isArray(pages) ? pages[index]?.url : undefined;
    if (!url) return;
    await confirmDiscardingEdits('Adding the page', async () => {
        try {
            const added = await api(
                `/api/listing-sources/${encodeURIComponent(listingSourceId)}/missing-value-pages/review`,
                {method: 'POST', body: JSON.stringify({url})}
            );
            await loadReviews();
            await selectReview(added.review_id);
        } catch (error) {
            alert(apiErrorMessage(error));
        }
    });
}

// A source whose removals the mass-removal guard holds is confirmed as a real bulk clearance for
// a bounded time, which captures its removals, the held ones at once. The sources whose
// confirmation form is open; they stay open across the console's polls.
const openMassRemovalConfirmations = new Set();
const MASS_REMOVAL_CONFIRMATION_HOURS = [[6, '6 hours'], [24, '24 hours'], [72, '3 days'], [168, '7 days']];

// The handler names the source by id, read from the row's own select, never from the page's HTML.
function renderMassRemovalConfirmation(listingSourceId) {
    const id = escapeHtmlAttr(listingSourceId);
    return `<div class="subpages"><div class="subpage">
      <span>Capture every removal of this source, the held ones at once, for</span>
      <select aria-label="Confirmation duration" id="massRemovalHours-${id}">${MASS_REMOVAL_CONFIRMATION_HOURS.map(([hours, label]) =>
          `<option value="${hours}"${hours === 24 ? ' selected' : ''}>${label}</option>`).join('')}</select>
      <button class="primary" onclick="confirmMassRemoval('${id}')" type="button">Confirm</button>
      <button class="quiet" onclick="toggleMassRemovalConfirmation('${id}')" type="button">Cancel</button>
    </div></div>`;
}

function toggleMassRemovalConfirmation(listingSourceId) {
    if (!openMassRemovalConfirmations.delete(listingSourceId)) openMassRemovalConfirmations.add(listingSourceId);
    showSourceHealth();
}

async function confirmMassRemoval(listingSourceId) {
    const hours = Number(document.getElementById(`massRemovalHours-${listingSourceId}`)?.value);
    if (!hours) return;
    try {
        await api(`/api/listing-sources/${encodeURIComponent(listingSourceId)}/mass-removal-confirmation`,
            {method: 'POST', body: JSON.stringify({hours})});
        openMassRemovalConfirmations.delete(listingSourceId);
        await loadReviews();
    } catch (error) {
        alert(apiErrorMessage(error));
    }
}

async function revokeMassRemovalConfirmation(listingSourceId) {
    try {
        await api(`/api/listing-sources/${encodeURIComponent(listingSourceId)}/mass-removal-confirmation`, {method: 'DELETE'});
        await loadReviews();
    } catch (error) {
        alert(apiErrorMessage(error));
    }
}

function reviewMatchesSearch(review, query) {
    return [
        review.listing_source_name,
        review.listing_source_id,
        review.artifact_type,
        review.status,
        review.reason,
        review.review_id
    ].some(value => String(value || '').toLowerCase().includes(query));
}

function setReviewSearch(value) {
    reviewSearchTerm = value || '';
    loadReviews().catch(reportQueueFailure);
}

function readStoredList(key) {
    try {
        const list = JSON.parse(localStorage.getItem(key) || '[]');
        return Array.isArray(list) ? list : [];
    } catch (_) {
        return [];
    }
}

// Storage may be blocked; the groups then stay as toggled until the page reloads.
function storeExpandedGroups() {
    try {
        localStorage.setItem(EXPANDED_GROUPS_KEY, JSON.stringify([...expandedListingSourceGroups]));
    } catch (_) {
        // Nothing to keep.
    }
}

function toggleListingSourceGroup(listingSourceId) {
    if (expandedListingSourceGroups.has(listingSourceId)) {
        expandedListingSourceGroups.delete(listingSourceId);
    } else {
        expandedListingSourceGroups.add(listingSourceId);
    }
    storeExpandedGroups();
    loadReviews().catch(reportQueueFailure);
}

// A queue row is a summary without the candidate and validation summary, which only the selected
// review's detail carries.
function renderReviewQueueItem(r) {
    const active = r.review_id === selectedId;
    // A link to the review's fragment: following it goes through the same hashchange path as
    // browser back, so it asks before discarding unsaved edits, and it opens in a new tab too.
    return `<a class="nav-item ${active ? 'active' : ''}" href="#${new URLSearchParams({review: r.review_id})}" ${active ? 'aria-current="true"' : ''}>
      <span class="nav-row">
        <span class="nav-title">${escapeHtml(displayName(r.artifact_type))}</span>
        <span class="nav-badges">${repairBadge(r) || statusBadge(r.status)}</span>
      </span>
      <span class="nav-meta">${escapeHtml(displayName(r.reason))} · ${timeBadge(r.created, 'Created')}</span>
    </a>`;
}

// `requested` names the page and schema a URL fragment asks for; without it the review opens on
// its first page and schema.
async function selectReview(id, requested = null) {
    setView('reviews');
    selectedId = id;
    requestedSelection = requested;
    pickedSelector = '';
    pickedSample = '';
    recordSelectionInUrl();
    dirty = false;
    sampleLabelDraft = null;
    editNotice = null;
    pendingAction = null;
    activeWorkbenchPanel = 'selector';
    await loadReviews().catch(reportQueueFailure);
    await submitEdit(() => loadSelectedReview(true));
}

function reportQueueFailure(error) {
    transientStatus = {text: `Queue refresh failed: ${apiErrorMessage(error)}`, className: 'system-status stale'};
    updateReloadState();
}

const MATRIX_POLL_INTERVAL_MS = 3000;

// A refresh fetches every review page at the shop's robots.txt Crawl-delay, so the server
// answers 202 while it runs in the background; poll until the matrix or its error arrives. A read
// without refresh never fetches: without a stored matrix it is refused with `NO_MATRIX`, and the
// console asks for a refresh only when the reviewer does. `refreshed` tells whether a refresh,
// the console's own or one it joined, stored the matrix meanwhile.
async function loadSchemaMatrix(reviewId, refresh) {
    const matrix = await api(`/api/reviews/${reviewId}/matrix${refresh ? '?refresh=true' : ''}`);
    const refreshed = refresh || matrix?.status === 'refreshing';
    return {matrix: await pollSchemaMatrix(reviewId, matrix), refreshed};
}

async function pollSchemaMatrix(reviewId, matrix) {
    if (matrix?.status !== 'refreshing') return matrix;
    if (selectedId !== reviewId) return null;
    if (selectedDetail?.review.review_id !== reviewId) {
        document.getElementById('detail').innerHTML =
            `<div class="muted">${escapeHtml(matrix.message || 'Refreshing schema matrix…')}</div>`;
    }
    await new Promise((resolve) => setTimeout(resolve, MATRIX_POLL_INTERVAL_MS));
    return pollSchemaMatrix(reviewId, await api(`/api/reviews/${reviewId}/matrix`));
}

// A failed matrix refresh, or a review without a stored matrix, leaves the review workable: the
// workbench shows why, without a matrix, and Re-evaluate starts a new refresh. The console never
// fetches the shop's pages or retries on its own.
async function loadSelectedReview(resetSelection, refreshMatrix = false) {
    if (!selectedId) return;
    const reviewId = selectedId;
    let detail = await api(`/api/reviews/${reviewId}`);
    let matrix = null;
    let matrixFailure = '';
    let matrixMissing = false;
    let candidateChangedMeanwhile = false;
    if (detail.review.artifact_type === 'PRODUCT_SCHEMA') {
        matrixRefreshesInFlight.set(reviewId, (matrixRefreshesInFlight.get(reviewId) || 0) + 1);
        try {
            let refreshed = refreshMatrix;
            try {
                ({matrix, refreshed} = await loadSchemaMatrix(reviewId, refreshMatrix));
            } catch (error) {
                matrix = {review_id: reviewId, candidates: [], removed_page_matches: []};
                if (refusalReason(error) === 'NO_MATRIX') {
                    matrixMissing = true;
                } else {
                    matrixFailure = apiErrorMessage(error);
                }
            }
            // A refresh stores its matrix on the review and so moves the review's `updated`: the
            // review read after it is the baseline, so the refresh does not look like a change
            // made elsewhere. A candidate another writer changed meanwhile still does. The console
            // refuses to refresh a candidate that fails validation and stores nothing, so such a
            // review is not read again, which would log its candidate's failure a second time.
            if (refreshed && !detail.candidate_error) {
                const before = detail.review.candidate_version;
                detail = await api(`/api/reviews/${reviewId}`);
                candidateChangedMeanwhile = detail.review.candidate_version !== before;
            }
        } finally {
            const left = (matrixRefreshesInFlight.get(reviewId) || 1) - 1;
            if (left > 0) {
                matrixRefreshesInFlight.set(reviewId, left);
            } else {
                matrixRefreshesInFlight.delete(reviewId);
            }
        }
    }
    if (matrix && rawAttributeKeys === null) {
        rawAttributeKeys = await api('/api/raw-attribute-keys').catch(() => null);
    }
    if (selectedId !== reviewId) return;
    selectedDetail = detail;
    selectedMatrix = matrix;
    selectedMatrixFailure = matrixFailure;
    selectedMatrixMissing = matrixMissing;
    selectedReviewUpdatedAt = detail.review.updated;
    selectedReviewStatus = detail.review.status;
    selectedReviewNeedsRefresh = candidateChangedMeanwhile;
    selectedReviewRefreshReason = candidateChangedMeanwhile
        ? 'The candidate was changed elsewhere while the schema matrix was evaluated; it is shown as it is now.'
        : '';
    if (resetSelection) {
        const requested = requestedSelection || {};
        requestedSelection = null;
        const pages = detail.pages || [];
        selectedSchemaIndex = requested.schemaIndex ?? 0;
        compareSchemaIndex = 1;
        selectedField = 'title';
        selectedPageId = (pages.find(page => page.review_page_id === requested.pageId) || pages[0])?.review_page_id || null;
        previewUrlOverride = '';
    }
    normalizeSchemaSelection();
    renderDetail(detail, matrix);
}

// The one component for everything that is said next to the work: a tone (`bad`, `warn`, `ok`,
// `neutral`), a one- or two-word kind, the body as HTML its caller has escaped, and optionally
// actions or a dismiss button.
function notice(tone, kind, body, {role = '', actions = '', dismiss = ''} = {}) {
    return `<div class="notice notice-${tone}"${role ? ` role="${role}"` : ''}><span class="notice-kind">${escapeHtml(kind)}</span><span>${body}</span>${
        actions ? `<span class="actions">${actions}</span>` : ''}${
        dismiss ? `<button class="icon quiet dismiss" onclick="${dismiss}" title="Dismiss" aria-label="Dismiss" type="button">${closeIcon()}</button>` : ''}</div>`;
}

function renderDetail(detail, matrix) {
    const review = detail.review;
    const urls = detail.urls || [];
    const frameKept = writeDetail(`
    <div class="page">
      <div class="page-head">
        <div>
          <div class="page-title">
            <h1>${escapeHtml(review.listing_source_name || review.listing_source_id)}</h1>
            ${statusBadge(review.status)}
            ${repairBadge(review)}
            ${statusBadge(review.artifact_type)}
            ${review.listing_source_crawl_enabled === false ? badge('Source disabled') : ''}
          </div>
          <div class="page-meta">
            <span>${escapeHtml(displayName(review.reason))}</span><span>·</span>
            <span>opened ${timeBadge(review.created, 'Opened')}</span><span>·</span>
            <span>edited ${timeBadge(review.updated, 'Edited')}</span>
          </div>
        </div>
        <div class="page-actions">${renderApprovalPanel(detail)}</div>
      </div>
      <div id="reviewRefreshBanner" hidden></div>
      <div id="actionPanel">${renderActionPanel()}</div>
      <div id="editMessages">${renderEditMessages()}</div>
      ${renderCandidateError(detail)}
      ${renderHumanRequired(review)}
      ${renderRepairRequest(review)}
      ${matrix ? renderMatrixNotice(detail, matrix) : ''}
      ${renderAutoSchemaEvaluation(review)}
      ${renderReviewEvents(detail)}
      ${matrix ? renderSchemaWorkbench(detail, matrix) : renderNonSchemaReview(review, urls)}
    </div>`);
    if (matrix) highlightPreview(frameKept);
    syncPageActions();
    recordSelectionInUrl();
    updateRefreshBanner();
    updateReloadState();
}

// Loading the preview fetches the shop page live, sends the shop another request and loses the
// preview's scroll and picked element, so a render keeps the frame while it shows the same
// page and replaces only the markup around it. Detaching the frame would reload it too, so it
// stays in the document while its ancestors take the new attributes and siblings, unless the
// reviewer asked to load the preview again. Answers whether the frame was kept.
let previewReloadRequested = false;

function writeDetail(html) {
    const container = document.getElementById('detail');
    const fresh = document.createElement('div');
    fresh.innerHTML = html;
    const frame = document.getElementById('snapshotFrame');
    const slot = fresh.querySelector('#snapshotFrame');
    const reload = previewReloadRequested;
    previewReloadRequested = false;
    const kept = Boolean(!reload && frame && slot && frame.getAttribute('src') === slot.getAttribute('src')
        && graftAroundFrame(container, frame, fresh, slot));
    if (!kept) container.replaceChildren(...fresh.childNodes);
    return kept;
}

// Moves the fresh markup around the live frame, level by level up to the container, when the
// frame sits in the same element structure in both; otherwise it changes nothing.
function graftAroundFrame(container, frame, fresh, slot) {
    const liveAncestors = ancestorsWithin(frame, container);
    const freshAncestors = ancestorsWithin(slot, fresh);
    if (!liveAncestors || !freshAncestors || liveAncestors.length !== freshAncestors.length
        || liveAncestors.some((element, i) => element.tagName !== freshAncestors[i].tagName)) return false;
    let kept = frame;
    let replaced = slot;
    liveAncestors.forEach((live, i) => {
        const next = freshAncestors[i];
        if (live !== container) copyAttributes(next, live);
        for (const child of Array.from(live.childNodes)) {
            if (child !== kept) child.remove();
        }
        const children = Array.from(next.childNodes);
        const at = children.indexOf(replaced);
        kept.before(...children.slice(0, at));
        kept.after(...children.slice(at + 1));
        kept = live;
        replaced = next;
    });
    return true;
}

// The element's ancestors from its parent up to and including `root`, or null outside it.
function ancestorsWithin(element, root) {
    const ancestors = [];
    for (let node = element.parentElement; node; node = node.parentElement) {
        ancestors.push(node);
        if (node === root) return ancestors;
    }
    return null;
}

function copyAttributes(from, to) {
    for (const {name} of Array.from(to.attributes)) {
        if (!from.hasAttribute(name)) to.removeAttribute(name);
    }
    for (const {name, value} of Array.from(from.attributes)) {
        if (to.getAttribute(name) !== value) to.setAttribute(name, value);
    }
}

// The stored candidate fails the validation approval applies: the review still shows it so the
// reviewer can repair it, and approval stays refused until then.
function renderCandidateError(detail) {
    if (!detail.candidate_error) return '';
    return notice('bad', 'Cannot approve', `The candidate fails validation: ${escapeHtml(detail.candidate_error)}. Repair it before approving.`, {role: 'alert'});
}

function renderApprovalPanel(detail) {
    const review = detail.review;
    if (review.status !== 'PENDING_REVIEW') {
        if (rejectionRevocable(detail)) {
            return '<button onclick="revokeRejection()" title="Lift this rejection\'s non-product verdict, so the crawler scrapes its seed and triggering pages again" type="button">Revoke rejection</button>';
        }
        if (rejectionRevoked(detail)) {
            return '<span class="badge" title="The review stays rejected, but its rejection no longer classifies its seed and triggering pages as non-product pages">Rejection revoked</span>';
        }
        if (review.status !== 'APPROVED') return '';
        const live = review.artifact_type === 'PRODUCT_SCHEMA'
            ? (detail.candidate_is_live
                ? badge('Live schemas — editable', 'ok')
                : '<span class="badge" title="The live schemas changed since this review was approved">Read-only — not the live schemas</span>')
            : '';
        const restore = approvalRestorable(detail)
            ? '<button onclick="restoreApproval()" title="Bring back the live configuration this approval replaced" type="button">Restore previous</button>'
            : '';
        return `${live}${restore}`;
    }
    return `<button onclick="needsRepair()" type="button">Needs repair</button>
      <button class="danger" onclick="rejectReview()" type="button">Reject</button>
      <button class="primary" onclick="approveReview()" type="button">Approve</button>`;
}

// Edits go to a pending review's candidate, or to the live schema list through the one approved
// review whose candidate is that list; the server refuses every other edit.
function candidateEditable(detail) {
    const review = detail?.review;
    if (!review) return false;
    if (review.status === 'PENDING_REVIEW') return true;
    return review.status === 'APPROVED' && review.artifact_type === 'PRODUCT_SCHEMA' && Boolean(detail.candidate_is_live);
}

// A candidate that cannot be edited says so at the top of the tab that would edit it.
function renderCandidateEditHint(detail) {
    const review = detail.review;
    if (review.status === 'PENDING_REVIEW') return '';
    if (review.status !== 'APPROVED') {
        return notice('neutral', 'Read-only', 'This review is closed; its candidate is read-only.');
    }
    return detail.candidate_is_live
        ? notice('neutral', 'Live schemas', 'This approved review holds the live schema list: saving a field or the schema list changes the live schemas at once.')
        : notice('neutral', 'Read-only', 'The live schemas changed since this review was approved. Edit the review whose candidate is live.');
}

// The server takes reviewer pages on pending and approved product-schema reviews.
function reviewPagesEditable(review) {
    return review?.artifact_type === 'PRODUCT_SCHEMA' && ['PENDING_REVIEW', 'APPROVED'].includes(review.status);
}

function pageRemovable(page) {
    return Boolean(page) && REMOVABLE_PAGE_ROLES.has(page.role) && reviewPagesEditable(selectedDetail?.review);
}

function pageRoleLabel(role) {
    return PAGE_ROLE_LABELS[role] || displayName(role);
}

function reviewPageLabel(page) {
    return `${pageRoleLabel(page.role)} · ${urlPath(page.url)}`;
}

function isWaitingForRepair(review) {
    return review?.status === 'PENDING_REVIEW' && Boolean(review.repair_request);
}

// A review whose agent needs-repair rounds reached the cap waits for a human, which outranks
// waiting for the repair the agents asked each other for.
function repairBadge(review) {
    if (review?.human_required) return badge('Needs a human', 'bad');
    return isWaitingForRepair(review) ? badge('Waiting for repair', 'warn') : '';
}

function renderHumanRequired(review) {
    if (!review?.human_required) return '';
    const rounds = Number(review.agent_rounds) || 0;
    return notice('bad', 'Needs a human', `The review agents gave ${escapeHtml(rounds)} needs-repair verdict${rounds === 1 ? '' : 's'} since a human last touched this review and stopped. Edit it, change its pages or labels, or give needs-repair with instructions to hand it back; or approve or reject it.`);
}

function renderRepairRequest(review) {
    const request = review?.repair_request;
    if (!request) return '';
    return notice('warn', isWaitingForRepair(review) ? 'Waiting for repair' : 'Repair requested',
        `${escapeHtml(request.note)} ${timeBadge(request.requested, 'Repair requested')}`);
}

const REVIEW_EVENT_LABELS = {
    CANDIDATE_EDITED: 'Candidate edited',
    SCHEMA_FIELD_EDITED: 'Field edited',
    PAGE_ADDED: 'Page added',
    PAGE_REMOVED: 'Page removed',
    REPAIR_REQUESTED: 'Repair requested',
    APPROVED: 'Approved',
    REJECTED: 'Rejected',
    RESTORED: 'Restored',
    REJECTION_REVOKED: 'Rejection revoked'
};

// An approval whose event keeps what it made live can be restored once; the server also
// refuses the restore once a newer change is live.
function approvalRestorable(detail) {
    const events = detail?.events || [];
    const details = events.find(event => event.action === 'APPROVED')?.details || {};
    const keepsWhatWentLive = details.approved_product_schemas !== undefined
        || details.approved_url_pattern !== undefined;
    return keepsWhatWentLive && !events.some(event => event.action === 'RESTORED');
}

// Only rejecting a manual product-schema review classifies its seed and triggering pages as
// non-product pages, so only that rejection is revoked, once.
function rejectionRevocable(detail) {
    const review = detail?.review;
    return review?.status === 'REJECTED'
        && review.artifact_type === 'PRODUCT_SCHEMA'
        && review.reason === 'manual_schema_required'
        && !rejectionRevoked(detail);
}

// A revoked rejection keeps the review's REJECTED status, so the console says it no longer applies.
function rejectionRevoked(detail) {
    return detail?.review?.status === 'REJECTED'
        && (detail.events || []).some(event => event.action === 'REJECTION_REVOKED');
}

// The review whose history the reviewer opened, so a re-render, such as a tab or schema change,
// keeps it open.
let reviewEventsOpenReviewId = null;

function reviewEventsToggled(open) {
    reviewEventsOpenReviewId = open ? selectedId : null;
}

// The review's event trail, newest first: each decision and edit with the actor that made it.
function renderReviewEvents(detail) {
    const events = detail?.events || [];
    if (!events.length) return '';
    const open = reviewEventsOpenReviewId === detail.review.review_id;
    const latest = events[events.length - 1];
    const latestLabel = REVIEW_EVENT_LABELS[latest.action] || displayName(latest.action);
    return `<details class="card history" ${open ? 'open' : ''} ontoggle="reviewEventsToggled(this.open)">
      <summary><span class="chev" aria-hidden="true">${chevronIcon()}</span><strong>History</strong><span>${events.length} event${events.length === 1 ? '' : 's'} · latest: ${escapeHtml(latestLabel.toLowerCase())} ${timeBadge(latest.created, 'Latest change')} by ${escapeHtml(actorWord(latest.actor))}</span></summary>
      <ol class="events">${events.slice().reverse().map(renderReviewEvent).join('')}</ol>
    </details>`;
}

function renderReviewEvent(event) {
    const summary = reviewEventSummary(event);
    return `<li class="event">
      ${actorBadge(event.actor)}
      <strong>${escapeHtml(REVIEW_EVENT_LABELS[event.action] || displayName(event.action))}</strong>
      ${summary ? `<span class="muted">${escapeHtml(summary)}</span>` : ''}
      ${event.notes ? `<span class="notes">${escapeHtml(event.notes)}</span>` : ''}
      <span class="muted">v${escapeHtml(event.candidate_version)}</span>
      ${timeBadge(event.created, REVIEW_EVENT_LABELS[event.action] || displayName(event.action))}
    </li>`;
}

function reviewEventSummary(event) {
    const details = event.details || {};
    const live = details.replaced_product_schemas !== undefined ? 'live schemas changed' : '';
    switch (event.action) {
        case 'CANDIDATE_EDITED':
            if (details.sample_labels) return `sample labels: ${sampleLabelCountsText(details.sample_labels)}`;
            return [details.removed_page_schemas_changed ? 'removed-page templates changed' : '', live].filter(Boolean).join('; ');
        case 'SCHEMA_FIELD_EDITED':
            return [`${schemaLabel(Number(details.schema_index) || 0)}: ${details.field || ''}`, live].filter(Boolean).join('; ');
        case 'PAGE_ADDED':
        case 'PAGE_REMOVED':
            return details.url || '';
        case 'APPROVED': {
            const coverage = details.coverage;
            if (coverage) return coverageText({...coverage, notEvaluated: coverage.not_evaluated});
            return details.sample_labels ? `on sample labels: ${sampleLabelCountsText(labelCounts(details.sample_labels.labels))}` : '';
        }
        case 'RESTORED':
            return details.replaced_url_pattern !== undefined
                ? 'URL pattern the approval replaced is live again'
                : 'schemas the approval replaced are live again';
        case 'REJECTION_REVOKED': {
            const reclassified = (details.reclassified_urls || []).length;
            return `${reclassified} rejected page${reclassified === 1 ? '' : 's'} classified again`;
        }
        default:
            return '';
    }
}

const ACTOR_WORDS = {HUMAN: 'a human', AGENT_AUTHOR: 'the author agent', AGENT_REVIEWER: 'the reviewer agent', SYSTEM: 'the system'};

function actorWord(actor) {
    return ACTOR_WORDS[actor] || displayName(actor).toLowerCase();
}

function actorBadge(actor) {
    return badge(displayName(actor), actor === 'HUMAN' ? 'accent' : undefined);
}

// The LLM evaluation of the selected review, from its validation summary.
function autoSchemaEvaluation(review) {
    return (((review || {}).validation_summary || {}).auto_schema_evaluation) || null;
}

// The one outcome of the crawler's LLM evaluation of a generated schema: auto-approved when the
// crawler approved it itself (`approved_by_llm`: in `auto` mode, on the model's high-confidence
// approval and a passed deterministic gate), rejected when the model rejected it, otherwise left
// to a human. The model's own decision and confidence and the gate's result are shown apart from
// it, labelled, so they never read as a second outcome.
function autoSchemaOutcome(evaluation) {
    if (evaluation.approved_by_llm) return {text: 'Auto-approved', tone: 'ok'};
    if (evaluation.decision === 'REJECT') return {text: 'Rejected by the model', tone: 'bad'};
    return {text: 'Needs human review', tone: 'warn'};
}

function deterministicGateText(review) {
    const passed = review?.validation_summary?.schema_matrix_summary?.deterministic_approval_ok;
    if (passed === true) return 'passed';
    if (passed === false) return 'failed';
    return 'not recorded';
}

function renderAutoSchemaEvaluation(review) {
    const evaluation = autoSchemaEvaluation(review);
    if (!evaluation) return '';
    const risks = Array.isArray(evaluation.risks) ? evaluation.risks.filter(Boolean) : [];
    const outcome = autoSchemaOutcome(evaluation);
    return `<div class="card"><div class="card-body stack">
      <div class="section-head"><h3>LLM evaluation</h3>${badge(outcome.text, outcome.tone)}</div>
      <div class="small">${escapeHtml(evaluation.summary || 'No summary provided.')}</div>
      <div class="small muted">Model: ${escapeHtml(displayName(evaluation.decision || 'unknown').toLowerCase())} · confidence ${escapeHtml(String(evaluation.confidence || 'unknown').toLowerCase())} · deterministic gate ${escapeHtml(deterministicGateText(review))}</div>
      ${risks.length ? notice('warn', 'Risks', risks.map(risk => escapeHtml(risk)).join('<br>')) : ''}
    </div></div>`;
}

// A URL-pattern review: the decision, the sample URLs and the sample labels. They cover every
// edit the API accepts for this artifact.
function renderNonSchemaReview(review, urls) {
    return `<div class="card"><div class="tab-panel">
      ${renderUrlPatternReview(review, urls)}
      ${renderSampleLabels(review)}
    </div></div>`;
}

function urlPatternDecision(review) {
    return review?.candidate_payload?.decision || {type: 'UNDECIDED'};
}

function decisionPattern(decision) {
    return decision?.type === 'PATTERN' ? decision.value : '';
}

function savedPatternValue(review) {
    return decisionPattern(urlPatternDecision(review));
}

// The selected URL-pattern review's decision as the server holds it, which the server's
// classification of its sample describes, and the pattern the reviewer typed but has not saved.
// A refused save leaves its decision in the local candidate of the loaded `review` as an unsaved
// edit, so `stored` is the decision of that review as it was loaded or last rendered without
// local edits. Both start over with each review and with each rendering without local edits,
// after a save, a discard or a reload; a reload that keeps the local edits, such as after a
// verdict, takes the reloaded decision and keeps the typed pattern.
let urlPatternEdit = {review: null, stored: null, typed: null};

function urlPatternEditOf(review) {
    if (!dirty || urlPatternEdit.review?.review_id !== review.review_id) {
        urlPatternEdit = {review, stored: urlPatternDecision(review), typed: null};
    } else if (urlPatternEdit.review !== review) {
        urlPatternEdit = {...urlPatternEdit, review, stored: urlPatternDecision(review)};
    }
    return urlPatternEdit;
}

function urlPatternEditable(review) {
    return review?.artifact_type === 'URL_PATTERN' && review.status === 'PENDING_REVIEW';
}

// What the pattern input holds: the reviewer's unsaved typing, kept across re-renders, or else
// the candidate's pattern.
function urlPatternInputValue(review) {
    const typed = urlPatternEditable(review) ? urlPatternEditOf(review).typed : null;
    return typed ?? savedPatternValue(review);
}

// The URL table's filter; it starts over for each review shown.
let urlFilter = {reviewId: null, kind: 'all', text: ''};

const URL_FILTER_KINDS = [
    ['all', 'All'],
    ['product', 'Product'],
    ['not_product', 'Not product'],
    ['changed', 'Changed']
];

// The decision and the sample URLs of a URL-pattern review. The counts and the table follow the
// pattern input while the reviewer types, as a browser preview, and show the server's
// classification of the saved candidate otherwise.
function renderUrlPatternReview(review, urls) {
    if (urlFilter.reviewId !== review.review_id) urlFilter = {reviewId: review.review_id, kind: 'all', text: ''};
    const view = urlPatternView(review, urls, urlPatternInputValue(review).trim());
    return `${renderUrlPatternDecisionSection(review, view)}
      ${urls.length ? `<div class="section">
        <div class="section-head"><h3>Sample URLs</h3><span class="small muted" id="urlPatternStats">${escapeHtml(urlPatternStatsText(view.counts))}</span></div>
        <div class="url-filter-bar">
          <div class="chips" id="urlFilterChips">${renderUrlFilterChips(view)}</div>
          <input id="urlFilterText" type="search" value="${escapeHtmlAttr(urlFilter.text)}" placeholder="Filter paths" aria-label="Filter paths" oninput="setUrlFilterText(this.value)">
        </div>
        <div id="urlPatternTable">${renderUrls(review, view)}</div>
      </div>` : ''}`;
}

// How each sample URL classifies under the ListingSource's current pattern and under the candidate: the
// unsaved pattern `pattern` as the browser previews it when it differs from the saved one, else
// the server's classification of the saved candidate.
function urlPatternView(review, urls, pattern) {
    const preview = urlPatternPreview(pattern, decisionPattern(urlPatternEditOf(review).stored));
    const origin = sampleOrigin(urls);
    const rows = urls.map((u, index) => {
        const candidateMatch = preview.active ? regexTakesUrl(preview.regex, u.url) : u.candidate_pattern_match;
        const currentProduct = u.current_pattern_match === true;
        const candidateProduct = candidateMatch === true;
        return {
            index,
            url: u.url,
            path: relativeToOrigin(u.url, origin),
            currentMatch: u.current_pattern_match,
            candidateMatch,
            candidateClass: u.candidate_class,
            candidateProduct,
            changed: currentProduct !== candidateProduct
        };
    });
    return {
        preview,
        rows,
        counts: {
            all: rows.length,
            current_product: rows.filter(row => row.currentMatch === true).length,
            product: rows.filter(row => row.candidateProduct).length,
            not_product: rows.filter(row => !row.candidateProduct).length,
            changed: rows.filter(row => row.changed).length
        }
    };
}

// The pattern the browser previews: none while the input holds the saved pattern or nothing, so
// the view shows the server's classification. Nor while the browser cannot compile the input,
// which then names the `error`: the counts, the filters and the table keep showing the saved
// candidate instead of counting every URL as not taken.
function urlPatternPreview(pattern, savedPattern) {
    if (!pattern || pattern === savedPattern) return {active: false};
    try {
        return {active: true, pattern, regex: new RegExp(pattern)};
    } catch (error) {
        return {active: false, pattern, error: error.message};
    }
}

// The origin most sample URLs share, so the table shows their paths.
function sampleOrigin(urls) {
    const counts = new Map();
    for (const u of urls) {
        const origin = urlOrigin(u.url);
        if (origin) counts.set(origin, (counts.get(origin) || 0) + 1);
    }
    return [...counts].sort((a, b) => b[1] - a[1])[0]?.[0] || '';
}

function urlOrigin(url) {
    try {
        return new URL(url).origin;
    } catch (_) {
        return '';
    }
}

// Only an http(s) URL becomes a link, as `linkifyText` links them: the console's policy runs
// inline script, so a `javascript:` URL from a shop must never reach an href.
function isHttpUrl(url) {
    try {
        return ['http:', 'https:'].includes(new URL(url).protocol);
    } catch (_) {
        return false;
    }
}

// The origin's spelling may differ from the stored URL's, such as by credentials or a default
// port, so only a URL that starts with it is shortened.
function relativeToOrigin(url, origin) {
    if (!origin || urlOrigin(url) !== origin || !url.startsWith(`${origin}/`)) return url;
    return url.slice(origin.length);
}

function urlPatternStatsText(counts) {
    return `${counts.all} sample · ${counts.current_product} product now · ${counts.product} product with candidate · ${counts.changed} changed`;
}

function renderUrlPatternDecisionSection(review, view) {
    const decision = urlPatternEditOf(review).stored;
    const currentPattern = review.candidate_payload?.current_pattern;
    const editable = urlPatternEditable(review);
    return `<div class="section">
      <div class="section-head"><h3>Decision</h3>${renderUrlPatternDecision(decision)}</div>
      <div class="small muted">${currentPattern ? `Current pattern: <code>${escapeHtml(currentPattern)}</code>` : 'Current pattern: none'}</div>
      ${editable ? `<label class="field">Product URL regex
          <input id="urlPatternInput" class="mono" value="${escapeHtmlAttr(urlPatternInputValue(review))}" placeholder="e.g. /product/" oninput="markDirty(); previewUrlPattern()">
        </label>
        <div class="actions">
          <button class="primary-outline" onclick="saveUrlPattern()" type="button">Save pattern</button>
          <button onclick="saveNoUrlPattern()" type="button">Save as no pattern</button>
          <span class="small muted" id="urlPatternPreview">${escapeHtml(urlPatternPreviewText(view.preview))}</span>
        </div>
        ${decision.type === 'UNDECIDED' ? notice('warn', 'Undecided', 'Enter a product URL regex or save no pattern before approving.') : ''}` : ''}
    </div>`;
}

function renderUrlPatternDecision(decision) {
    if (decision.type === 'PATTERN') return `<span>${badge('Pattern', 'ok')} <code>${escapeHtml(decision.value)}</code></span>`;
    if (decision.type === 'NO_PATTERN') return badge('No pattern');
    return badge('Undecided', 'warn');
}

function urlPatternPreviewText(preview) {
    if (preview.error) return `Invalid pattern: ${preview.error}. The counts and the table show the saved candidate.`;
    if (!preview.active) return 'Showing the saved decision.';
    return 'Browser preview of the unsaved pattern; the server validates and classifies it on save.';
}

function renderUrlFilterChips(view) {
    return URL_FILTER_KINDS.map(([kind, label]) => `<button class="chip ${urlFilter.kind === kind ? 'active' : ''}" aria-pressed="${urlFilter.kind === kind}" onclick="setUrlFilterKind('${kind}')" type="button">${escapeHtml(label)} ${view.counts[kind]}</button>`).join('');
}

function urlRowShown(row) {
    if (urlFilter.kind === 'product' && !row.candidateProduct) return false;
    if (urlFilter.kind === 'not_product' && row.candidateProduct) return false;
    if (urlFilter.kind === 'changed' && !row.changed) return false;
    const text = urlFilter.text.trim().toLowerCase();
    return !text || row.path.toLowerCase().includes(text);
}

// Whether `regex` takes the page of `url` as the crawler matches a URL pattern: the URL as stored,
// or its path with or without the trailing slash (docs/crawler/networking.md#stored-urls).
function regexTakesUrl(regex, url) {
    return pageSpellings(url).some(spelling => regex.test(spelling));
}

function pageSpellings(url) {
    let parsed;
    try {
        parsed = new URL(url);
    } catch (_) {
        return [url];
    }
    const path = parsed.pathname.replace(/\/+$/, '');
    if (!path) return [url];
    const spellings = [url];
    for (const pathname of [path, `${path}/`]) {
        parsed.pathname = pathname;
        if (!spellings.includes(parsed.href)) spellings.push(parsed.href);
    }
    return spellings;
}

// Called after `markDirty`, so the typed pattern outlives a re-render of the review, such as the
// one a sample label change or a refused save makes.
function previewUrlPattern() {
    const input = document.getElementById('urlPatternInput');
    if (!selectedDetail || !input) return;
    urlPatternEditOf(selectedDetail.review).typed = input.value;
    rerenderUrlPatternView();
}

function setUrlFilterKind(kind) {
    urlFilter.kind = kind;
    rerenderUrlPatternView();
}

function setUrlFilterText(text) {
    urlFilter.text = text || '';
    rerenderUrlPatternView();
}

// Renders the counts, the filter chips, the table and the sample labels' "Decision takes it"
// cells again, leaving the inputs alone so they keep their focus and what was typed into them.
function rerenderUrlPatternView() {
    if (!selectedDetail) return;
    const review = selectedDetail.review;
    const view = urlPatternView(review, selectedDetail.urls || [], urlPatternInputValue(review).trim());
    const parts = {
        urlPatternStats: () => escapeHtml(urlPatternStatsText(view.counts)),
        urlPatternPreview: () => escapeHtml(urlPatternPreviewText(view.preview)),
        urlFilterChips: () => renderUrlFilterChips(view),
        urlPatternTable: () => renderUrls(review, view)
    };
    currentSampleLabels(review).forEach((label, index) => {
        parts[`sampleLabelTakes${index}`] = () => renderSampleLabelTakes(label);
    });
    for (const [id, render] of Object.entries(parts)) {
        const element = document.getElementById(id);
        if (element) element.innerHTML = render();
    }
}

async function saveUrlPatternDecision(decision) {
    const currentPattern = selectedDetail.review?.candidate_payload?.current_pattern ?? null;
    await saveCandidatePayload({decision, current_pattern: currentPattern});
}

async function saveUrlPattern() {
    const value = document.getElementById('urlPatternInput').value.trim();
    if (!value) return;
    await saveUrlPatternDecision({type: 'PATTERN', value});
}

async function saveNoUrlPattern() {
    await saveUrlPatternDecision({type: 'NO_PATTERN'});
}

// The handler names a URL by its index in the review's sample, never by the URL itself, which a
// shop controls (see `renderMissingValuePages`).
function renderUrls(review, view) {
    const editable = sampleLabelsEditable(review);
    const labels = currentSampleLabels(review);
    const rows = view.rows.filter(urlRowShown);
    // The class comes from the crawler's classifier, which the browser cannot run, so a preview
    // leaves it as the server classified the saved candidate.
    const [candidateHead, classHead] = view.preview.active ? ['Candidate (preview)', 'Class (saved)'] : ['Candidate', 'Class'];
    return `<div class="data-grid-wrap"><table class="url-table"><thead><tr><th>Path</th><th>Now</th><th>${candidateHead}</th><th>${classHead}</th><th>Label</th></tr></thead><tbody>${
        rows.map(row => {
            const label = labels.find(l => l.url === row.url)?.label || '';
            const cell = editable
                ? renderSampleLabelSelect(label, `setSampleLabelOfSampleUrl(${row.index}, this.value)`, true)
                : (label ? badge(displayName(label)) : '');
            const path = isHttpUrl(row.url)
                ? `<a class="product-url" href="${escapeHtmlAttr(row.url)}" target="_blank" rel="noopener noreferrer" title="${escapeHtmlAttr(row.url)}">${escapeHtml(row.path)}</a>`
                : escapeHtml(row.path);
            return `<tr class="${row.changed ? 'url-row-changed' : ''}">
              <td>${path}</td>
              <td>${patternMatchIcon(row.currentMatch)}</td>
              <td>${patternMatchIcon(row.candidateMatch)}</td>
              <td>${row.candidateClass ? badge(displayName(row.candidateClass)) : ''}</td>
              <td>${cell}</td>
            </tr>`;
        }).join('') || '<tr><td colspan="5" class="muted">No sample URL matches the filter.</td></tr>'
    }</tbody></table></div>`;
}

function patternMatchIcon(match) {
    if (match === true) return '<span class="match-icon match-yes" title="The pattern takes it">✓</span>';
    return `<span class="match-icon match-no" title="${match === false ? 'The pattern does not take it' : 'No pattern'}">–</span>`;
}

const SAMPLE_URL_LABELS = ['PRODUCT', 'NOT_PRODUCT'];

function sampleLabelsEditable(review) {
    return review?.artifact_type === 'URL_PATTERN' && review.status === 'PENDING_REVIEW';
}

function storedSampleLabels(review) {
    return review?.candidate_payload?.sample_labels || null;
}

// The labels shown: the unsaved edit, or else the candidate's stored labels.
function currentSampleLabels(review) {
    if (sampleLabelDraft) return sampleLabelDraft;
    return (storedSampleLabels(review)?.labels || []).map(label => ({...label}));
}

function labelCounts(labels) {
    const list = Array.isArray(labels) ? labels : [];
    return {
        product: list.filter(l => l.label === 'PRODUCT').length,
        not_product: list.filter(l => l.label === 'NOT_PRODUCT').length
    };
}

function sampleLabelCountsText(counts) {
    return `${counts?.product ?? 0} product, ${counts?.not_product ?? 0} not product`;
}

function renderSampleLabelSelect(selected, onchange, allowNone) {
    const options = (allowNone ? [''] : []).concat(SAMPLE_URL_LABELS);
    return `<select onchange="${onchange}">${options.map(value =>
        `<option value="${escapeHtmlAttr(value)}" ${value === selected ? 'selected' : ''}>${escapeHtml(value ? displayName(value) : '—')}</option>`
    ).join('')}</select>`;
}

// Whether the decision in the pattern input takes `url`, as the browser previews it; the server
// matches the crawler's spelling of the URL when it checks an agent approval. It reads the input's
// state, not the element: while the review renders, the element still belongs to the review shown
// before.
function decisionPreviewTakes(url) {
    const review = selectedDetail?.review;
    if (!review) return null;
    const pattern = urlPatternInputValue(review).trim() || decisionPattern(urlPatternEditOf(review).stored);
    if (!pattern) return null;
    try {
        return regexTakesUrl(new RegExp(pattern), url);
    } catch (_) {
        return null;
    }
}

// Whether the decision in the pattern input takes the labelled URL, and whether that agrees with
// its label.
function renderSampleLabelTakes(label) {
    const takes = decisionPreviewTakes(label.url);
    const takesText = takes === null ? '' : (takes ? 'Yes' : 'No');
    const consistent = takes === null || takes === (label.label === 'PRODUCT');
    return consistent ? escapeHtml(takesText) : badge(`${takesText} — inconsistent`, 'bad');
}

// The author agent's or a human's labels of URLs on the review's crawl host, its sample's among
// them: which are product pages. An agent approval of the pattern rests on them alone; the
// reviewer agent cannot change them.
function renderSampleLabels(review) {
    if (review.artifact_type !== 'URL_PATTERN') return '';
    const stored = storedSampleLabels(review);
    const labels = currentSampleLabels(review);
    const editable = sampleLabelsEditable(review);
    const counts = labelCounts(labels);
    const status = sampleLabelDraft
        ? 'unsaved label edits'
        : (stored ? `set ${timeBadge(stored.labelled_at, 'Labelled')}` : 'no labels yet');
    const rows = labels.map((label, index) => `<tr>
          <td>${linkifyText(label.url)}</td>
          <td>${editable ? renderSampleLabelSelect(label.label, `setSampleLabelKind(${index}, this.value)`, false) : badge(displayName(label.label))}</td>
          <td>${editable
            ? `<input value="${escapeHtmlAttr(label.reason || '')}" placeholder="Why" aria-label="Reason" oninput="setSampleLabelReason(${index}, this.value)">`
            : escapeHtml(label.reason || '')}</td>
          <td id="sampleLabelTakes${index}">${renderSampleLabelTakes(label)}</td>
          <td>${editable ? `<button class="quiet danger" onclick="removeSampleLabel(${index})" type="button">Remove</button>` : ''}</td>
        </tr>`).join('');
    const adder = editable ? `<div class="actions">
        <input id="sampleLabelUrl" placeholder="Another URL on the review's crawl host" aria-label="URL to label">
        ${renderSampleLabelSelect('PRODUCT', '', false).replace('<select', '<select id="sampleLabelKind" aria-label="Label"')}
        <input id="sampleLabelReason" placeholder="Why" aria-label="Reason">
        <button onclick="addSampleLabel()" type="button">Add label</button>
        <span class="spacer"></span>
        <button class="primary-outline" onclick="saveSampleLabels()" type="button">Save labels</button>
      </div>` : '';
    return `<div class="section">
      <div class="section-head"><h3>Sample labels</h3></div>
      <div class="small muted">${escapeHtml(sampleLabelCountsText(counts))} (${status}). An agent approval needs enough labels of each and a decision that takes every product URL and no not-product URL.</div>
      ${labels.length ? `<div class="data-grid-wrap"><table><thead><tr><th>URL</th><th>Label</th><th>Reason</th><th>Decision takes it</th><th></th></tr></thead><tbody>${rows}</tbody></table></div>` : ''}
      ${adder}
    </div>`;
}

function editSampleLabels(edit) {
    const labels = currentSampleLabels(selectedDetail.review);
    edit(labels);
    sampleLabelDraft = labels;
    markDirty();
    rerenderWorkbench();
}

function setSampleLabelOfUrl(url, label) {
    editSampleLabels(labels => {
        const index = labels.findIndex(l => l.url === url);
        if (!label) {
            if (index >= 0) labels.splice(index, 1);
        } else if (index >= 0) {
            labels[index].label = label;
        } else {
            labels.push({url, label, reason: ''});
        }
    });
}

function setSampleLabelOfSampleUrl(index, label) {
    const url = selectedDetail?.urls?.[index]?.url;
    if (url) setSampleLabelOfUrl(url, label);
}

function setSampleLabelKind(index, label) {
    editSampleLabels(labels => {
        if (labels[index]) labels[index].label = label;
    });
}

// Typing a reason keeps the input focused, so it changes the draft without rendering again.
function setSampleLabelReason(index, reason) {
    const labels = currentSampleLabels(selectedDetail.review);
    if (!labels[index]) return;
    labels[index].reason = reason;
    sampleLabelDraft = labels;
    markDirty();
}

function removeSampleLabel(index) {
    editSampleLabels(labels => labels.splice(index, 1));
}

function addSampleLabel() {
    const url = document.getElementById('sampleLabelUrl')?.value?.trim();
    if (!url) return;
    const label = document.getElementById('sampleLabelKind')?.value || 'PRODUCT';
    const reason = document.getElementById('sampleLabelReason')?.value || '';
    editSampleLabels(labels => {
        const index = labels.findIndex(l => l.url === url);
        if (index >= 0) labels.splice(index, 1);
        labels.push({url, label, reason});
    });
}

// The labels replace the candidate's labels as a whole, based on the candidate version shown; the
// server checks each URL and reason and refuses the save with a message otherwise.
async function saveSampleLabels() {
    const labels = currentSampleLabels(selectedDetail.review)
        .map(({url, label, reason}) => ({url, label, reason}));
    await submitMutation(async () => {
        const saved = await api(`/api/reviews/${selectedId}/sample-labels`, {
            method: 'POST',
            body: JSON.stringify({expected_candidate_version: expectedCandidateVersion(), labels})
        });
        recordSavedCandidateVersion(saved);
        sampleLabelDraft = null;
    }, 'Sample labels saved', async () => {
        await loadSelectedReview(false);
        await loadReviews();
    });
}

function renderSchemaWorkbench(detail, matrix) {
    const pages = detail.pages || [];
    const schemas = currentSchemas();
    const page = pages.find(p => p.review_page_id === selectedPageId) || pages[0];
    if (page && !selectedPageId) selectedPageId = page.review_page_id;
    const previewUrl = previewUrlOverride || (page ? page.url : '');
    const templates = currentRemovedPageTemplates().length;
    return `
    <div class="workbench">
      <div class="card preview">
        <div class="preview-bar">
          <div class="preview-row">
            <label for="reviewPageSelect">Page</label>
            <select id="reviewPageSelect" aria-label="Review page" onchange="selectedPageId=this.value; previewUrlOverride=''; rerenderWorkbench()">
              ${pages.map(p => `<option value="${escapeHtmlAttr(p.review_page_id)}" ${p.review_page_id === selectedPageId ? 'selected' : ''} title="${escapeHtmlAttr(p.url)}">${escapeHtml(reviewPageLabel(p))}</option>`).join('')}
            </select>
            <button class="quiet danger" ${pageRemovable(page) ? '' : 'disabled'} onclick="removeReviewPage()" title="Removes a page a reviewer added or a page waiting on the review; seed pages stay" type="button">Remove page</button>
          </div>
          <div class="preview-row">
            <input id="livePreviewUrl" class="mono" value="${escapeHtmlAttr(previewUrl)}" placeholder="https://shop.example/product" aria-label="Preview URL" onkeydown="if(event.key==='Enter') loadPreviewUrl()">
            <button class="icon" onclick="openPreviewUrl()" title="Open in a new tab" aria-label="Open in a new tab" type="button">${externalIcon()}</button>
            ${previewUrlOverride ? '<button class="quiet" onclick="resetPreviewUrl()" type="button">Back to review page</button>' : ''}
            <button ${reviewPagesEditable(detail.review) ? '' : 'disabled'} onclick="addReviewPage()" title="Fetches this URL of the source and adds it to the review pages" type="button">Add to review</button>
          </div>
        </div>
        ${renderPreviewFrame(page, previewUrl)}
      </div>
      <div class="card inspector">
        <div class="tabs" role="tablist">
          ${workbenchTab('selector', 'Selector')}
          ${workbenchTab('data', 'Data')}
          ${workbenchTab('diff', 'Diff')}
          ${workbenchTab('removed', `Removed pages${templates ? `<span class="count">${templates}</span>` : ''}`)}
          ${workbenchTab('json', 'JSON')}
        </div>
        ${renderWorkbenchPanel(detail, matrix, schemas)}
      </div>
    </div>`;
}

function renderPreviewFrame(page, previewUrl) {
    if (!previewUrl) return '';
    return `<iframe id="snapshotFrame" src="${escapeHtmlAttr(previewFrameSrc(page, previewUrl))}" sandbox="allow-scripts"></iframe>`;
}

function previewFrameSrc(page, previewUrl) {
    if (!previewUrlOverride && page) return `/api/review-pages/${page.review_page_id}/inspect`;
    return `/api/live-inspect?url=${encodeURIComponent(previewUrl)}`;
}

// The reviewer asks for the preview with these controls, so they load it again even when it
// already shows that URL, for example after the shop refused the first fetch.
function loadPreviewUrl() {
    const input = document.getElementById('livePreviewUrl');
    previewUrlOverride = (input && input.value || '').trim();
    previewReloadRequested = true;
    rerenderWorkbench();
}

// Loads the typed URL into the preview and opens it in a new tab, when it is an http(s) URL.
function openPreviewUrl() {
    const url = (document.getElementById('livePreviewUrl')?.value || '').trim();
    loadPreviewUrl();
    if (!isHttpUrl(url)) return;
    const link = document.createElement('a');
    link.href = url;
    link.target = '_blank';
    link.rel = 'noopener noreferrer';
    link.click();
}

function resetPreviewUrl() {
    previewUrlOverride = '';
    previewReloadRequested = true;
    rerenderWorkbench();
}

function workbenchTab(key, label) {
    const active = activeWorkbenchPanel === key;
    return `<button class="tab ${active ? 'active' : ''}" role="tab" aria-selected="${active}" onclick="setWorkbenchPanel('${key}')" type="button">${label}</button>`;
}

function renderWorkbenchPanel(detail, matrix, schemas) {
    if (activeWorkbenchPanel === 'data') return renderDataPanel(matrix);
    if (activeWorkbenchPanel === 'diff') return renderDiffPanel(schemas);
    if (activeWorkbenchPanel === 'removed') return renderRemovedPagePanel(detail, matrix);
    if (activeWorkbenchPanel === 'json') return renderJsonPanel(detail);
    return renderSelectorPanel(schemas, matrix);
}

function renderSelectorPanel(schemas, matrix) {
    const rule = currentRule();
    const effectiveRule = rule || defaultRuleFor(selectedField);
    const matrixPage = currentMatrixPage(matrix);
    const disabled = candidateEditable(selectedDetail) ? '' : 'disabled';
    const cardinality = effectiveRule.cardinality || 'first';
    const type = effectiveRule.type || 'text';
    const additional = Array.isArray(effectiveRule.additional_selectors) ? effectiveRule.additional_selectors : [];
    const findings = fieldFindings(matrixPage, selectedField);
    return `<div class="tab-panel">
      ${renderCandidateEditHint(selectedDetail)}
      ${renderSchemaChips(schemas, disabled)}
      <div class="section">
        <div class="section-head"><h3>Rule</h3></div>
        <label class="field">Field
          <select id="selectedFieldSelect" onchange="selectedField=this.value; rerenderWorkbench()">
            ${schemaFields(currentSchema()).map(field => `<option value="${escapeHtmlAttr(field)}" ${field === selectedField ? 'selected' : ''}>${escapeHtml(fieldLabel(field))}${fieldStateSuffix(matrixPage, field)}</option>`).join('')}
          </select>
        </label>
        <label class="field">Selector
          <span class="selector-row">
            <input id="selectedSelector" class="mono" ${disabled} value="${escapeHtmlAttr(currentSelector())}" oninput="setCurrentSelector(this.value); postHighlight(this.value)">
            <button class="icon" onclick="postHighlight(currentSelector())" title="Highlight in the preview" aria-label="Highlight in the preview" type="button">${eyeIcon()}</button>
          </span>
        </label>
        ${findings.length ? notice('bad', 'Failed check', `${findings.map(finding => escapeHtml(findingDescription(finding))).join('; ')} on this page.`) : ''}
        <div class="rule-grid">
          <label class="field">Matches
            <select ${disabled} onchange="setCurrentRuleCardinality(this.value)">
              <option value="first" ${cardinality === 'first' ? 'selected' : ''}>First match</option>
              <option value="all" ${cardinality === 'all' ? 'selected' : ''}>All matches</option>
            </select>
          </label>
          <label class="field">Read
            <select ${disabled} onchange="setCurrentRuleType(this.value)">
              <option value="text" ${type === 'text' ? 'selected' : ''}>Text</option>
              <option value="attribute" ${type === 'attribute' ? 'selected' : ''}>Attribute</option>
              <option value="image_url" ${type === 'image_url' ? 'selected' : ''} title="The best full-size image URL candidate of each matched element">Image URL</option>
              <option value="attribute_or_text" ${type === 'attribute_or_text' ? 'selected' : ''} title="The attribute when it is not blank, the element's text otherwise">Attribute or text</option>
              <option value="json_ld_offer" ${type === 'json_ld_offer' ? 'selected' : ''} title="availability or price from the offers in the matched ld+json script">JSON-LD offer</option>
              <option value="json_ld_product" ${type === 'json_ld_product' ? 'selected' : ''} title="productID or sku of the Product in the matched ld+json script">JSON-LD product</option>
            </select>
          </label>
          ${ruleTypeTakesAttributeName(type) ? `<label class="field">Attribute name
            <input ${disabled} value="${escapeHtmlAttr(effectiveRule.name || '')}" oninput="setCurrentRuleAttributeName(this.value)">
          </label>` : ''}
          ${type === 'json_ld_offer' ? `<label class="field">Offer property
            <select ${disabled} onchange="setCurrentRuleOfferProperty(this.value)">
              <option value="availability" ${effectiveRule.property === 'availability' ? 'selected' : ''}>Availability</option>
              <option value="price" ${effectiveRule.property === 'price' ? 'selected' : ''}>Price</option>
            </select>
          </label>` : ''}
          ${type === 'json_ld_product' ? `<label class="field">Product property
            <select ${disabled} onchange="setCurrentRuleOfferProperty(this.value)">
              <option value="product_id" ${effectiveRule.property === 'product_id' ? 'selected' : ''}>Product ID</option>
              <option value="sku" ${effectiveRule.property === 'sku' ? 'selected' : ''}>SKU</option>
            </select>
          </label>` : ''}
        </div>
        ${renderFallbackSelectors(additional, disabled)}
        <div class="actions">
          <button class="primary-outline" ${disabled} onclick="saveCurrentField()" type="button">Save field</button>
          <button class="quiet danger" ${disabled || (fieldDeletable(selectedField) ? '' : 'disabled')} onclick="deleteCurrentRule()" type="button">Delete rule</button>
          <span class="spacer"></span>
          ${dirty ? '<button class="quiet" onclick="discardEdits()" type="button">Discard edits</button>' : ''}
        </div>
        <div id="pickedElement">${renderPickedElement(true)}</div>
      </div>
      ${renderRawAttributeAdder(disabled)}
    </div>`;
}

// " · failed check" or " · empty" after the field the selected page has something to say about.
function fieldStateSuffix(page, field) {
    if (fieldFindings(page, field).length) return ' · failed check';
    if (optionalFieldMisses(page).some(miss => miss.field === fieldKey(field))) return ' · empty';
    return '';
}

// Raw attribute keys come from the crawler's registry: an edit may add only those, while a key
// the candidate already holds stays editable and removable.
function renderRawAttributeAdder(disabled) {
    const configured = new Set(Object.keys(currentSchema().raw_attributes || {}));
    const available = (rawAttributeKeys || []).filter(entry => !configured.has(entry.key));
    if (!available.length) return '';
    const groups = [...new Set(available.map(entry => entry.group))];
    return `<div class="section">
      <div class="section-head"><h3 title="A registry key the schema does not configure yet; adding it selects it as the field to edit">Raw attributes</h3></div>
      <div class="selector-row">
        <select id="newRawAttributeKey" aria-label="Raw attribute key" ${disabled}>
          ${groups.map(group => `<optgroup label="${escapeHtmlAttr(displayName(group))}">${
            available.filter(entry => entry.group === group).map(entry =>
                `<option value="${escapeHtmlAttr(entry.key)}" title="${escapeHtmlAttr(entry.description)}">${escapeHtml(entry.key)}</option>`
            ).join('')}</optgroup>`).join('')}
        </select>
        <button ${disabled} onclick="addRawAttribute()" title="Starts from the picked element when one is picked; save the field to store it" type="button">Add</button>
      </div>
    </div>`;
}

function addRawAttribute() {
    const select = document.getElementById('newRawAttributeKey');
    if (!select || !select.value) return;
    selectedField = RAW_ATTRIBUTE_FIELD_PREFIX + select.value;
    const rule = ensureCurrentRule();
    if (pickedSelector) rule.selector = pickedSelector;
    markDirty();
    rerenderWorkbench();
}

// The element last clicked in the preview, kept across side-panel renders; the Selector tab
// offers it as the selector or a fallback, the Removed pages tab has its own buttons per template.
function renderPickedElement(withActions) {
    if (!pickedSelector) return '';
    return `<div class="picked">
      <div class="picked-head">Picked in the preview</div>
      <code>${escapeHtml(pickedSelector)}</code>
      ${pickedSample ? `<span class="sample">${escapeHtml(pickedSample)}</span>` : ''}
      ${withActions ? `<div class="actions">
        <button onclick="applyPickedSelector()" type="button">Use as selector</button>
        <button onclick="addPickedAdditionalSelector()" type="button">Add as fallback</button>
      </div>` : ''}
    </div>`;
}

// The schemas as chips, in the order approval writes them; the toolbar acts on the selected one.
function renderSchemaChips(schemas, disabled) {
    if (!schemas.length) return notice('neutral', 'No schemas', 'The candidate holds no schema.');
    const last = selectedSchemaIndex >= schemas.length - 1;
    return `<div class="section">
      <div class="section-head"><h3>Schemas</h3><span class="small muted">applied in this order</span></div>
      <div class="chips">
        ${schemas.map((_, i) => `<button class="chip ${i === selectedSchemaIndex ? 'active' : ''}" aria-pressed="${i === selectedSchemaIndex}" onclick="selectedSchemaIndex=${i}; normalizeSchemaSelection(); rerenderWorkbench()" type="button">${schemaLabel(i)}</button>`).join('')}
        <button class="chip add" ${disabled} onclick="addNewSchema()" title="Adds a copy of the selected schema to rewrite for another template" type="button">+ Add schema</button>
      </div>
      <div class="toolbar">
        <button class="quiet" ${disabled || (selectedSchemaIndex === 0 ? 'disabled' : '')} onclick="moveCurrentSchema(-1)" type="button">Move up</button>
        <button class="quiet" ${disabled || (last ? 'disabled' : '')} onclick="moveCurrentSchema(1)" type="button">Move down</button>
        <button class="quiet danger" ${disabled || (schemas.length > 1 ? '' : 'disabled')} onclick="deleteCurrentSchema()" type="button">Delete</button>
        <span class="spacer"></span>
        <button class="quiet" onclick="postHighlightSchema()" title="Highlights every rule of the selected schema in the preview" type="button">Highlight all</button>
      </div>
    </div>`;
}

// The handlers let postHighlight name the selected field: a raw attribute key comes from the
// candidate, and HTML escaping does not make it safe inside an inline handler's JS string.
// The fallback selectors of the selected rule. The handlers name a fallback by its index, never
// by a value the candidate holds.
function renderFallbackSelectors(additional, disabled) {
    return `<div class="fallbacks">
      <h4 title="Fallback or extra selectors for the selected field">Fallback selectors</h4>
      ${additional.map((selector, index) => `<div class="fallback">
        <input class="mono" ${disabled} value="${escapeHtmlAttr(selector)}" aria-label="Fallback selector ${index + 1}" oninput="setAdditionalSelector(${index}, this.value); postHighlight(this.value)">
        <button class="icon" onclick="postHighlight(additionalSelectorAt(${index}))" title="Highlight in the preview" aria-label="Highlight in the preview" type="button">${eyeIcon()}</button>
        <button class="icon" ${disabled} onclick="removeAdditionalSelector(${index})" title="Remove this fallback" aria-label="Remove this fallback" type="button">${closeIcon()}</button>
      </div>`).join('')}
      <div class="actions"><button class="quiet" ${disabled} onclick="addAdditionalSelector()" type="button">+ Add fallback</button></div>
    </div>`;
}

function renderDataPanel(matrix) {
    return `<div class="tab-panel">
      ${renderCoverageSection(matrix)}
      ${renderFailedChecksSection(matrix)}
      ${renderNotesSection(matrix)}
      ${renderExtractedSection(matrix)}
    </div>`;
}

function renderDiffPanel(schemas) {
    if (schemas.length < 2) {
        return `<div class="tab-panel"><div class="empty">Add a second schema to compare.</div></div>`;
    }
    normalizeSchemaSelection();
    return `<div class="tab-panel">
      <div class="rule-grid">
        <label class="field">Schema
          <select onchange="selectedSchemaIndex=Number(this.value); normalizeSchemaSelection(); rerenderWorkbench()">
            ${schemas.map((_, i) => `<option value="${i}" ${i === selectedSchemaIndex ? 'selected' : ''}>${schemaLabel(i)}</option>`).join('')}
          </select>
        </label>
        <label class="field">Compare with
          <select onchange="compareSchemaIndex=Number(this.value); rerenderWorkbench()">
            ${schemas.map((_, i) => `<option value="${i}" ${i === compareSchemaIndex ? 'selected' : ''} ${i === selectedSchemaIndex ? 'disabled' : ''}>${schemaLabel(i)}</option>`).join('')}
          </select>
        </label>
      </div>
      ${renderSchemaDiff(schemas[selectedSchemaIndex], schemas[compareSchemaIndex])}
    </div>`;
}

// Removed-page templates for pages a shop serves at HTTP 200 with a removal notice and no
// product markup. The list opens with the ListingSource's stored templates and approval
// replaces them with it; the matrix shows which review pages they match.
function currentRemovedPageTemplates() {
    return (((selectedDetail.review || {}).candidate_payload || {}).removed_page_schemas) || [];
}

function removedPageTemplatesEditable(detail) {
    return detail.review.status === 'PENDING_REVIEW';
}

function renderRemovedPagePanel(detail, matrix) {
    const editable = removedPageTemplatesEditable(detail);
    const disabled = editable ? '' : 'disabled';
    const templates = currentRemovedPageTemplates();
    return `<div class="tab-panel">
      <div class="section">
        <div class="section-head"><h3>Removed-page templates</h3><span class="small muted">${templates.length} template${templates.length === 1 ? '' : 's'}</span></div>
        <div class="small muted">${editable
            ? 'A template declares a page removed: a selector bound to the notice, with text it contains or a regex. The list starts from the stored templates; approval replaces them, so removing one here retires it.'
            : detail.review.status === 'APPROVED'
                ? 'Stored when the review was approved; edit on a pending review.'
                : 'Closed without approval; the stored templates are unchanged.'}</div>
        ${templates.map((template, index) => renderRemovedPageTemplate(template, index, disabled)).join('')
          || '<div class="small muted">No removed-page template.</div>'}
        <div class="actions">
          <button ${disabled} onclick="addRemovedPageTemplate()" title="Starts from the picked element when one is picked" type="button">+ Add template</button>
          <button class="primary-outline" ${disabled} onclick="saveRemovedPageTemplates()" type="button">Save templates</button>
          <span class="spacer"></span>
          ${dirty && editable ? '<button class="quiet" onclick="discardEdits()" type="button">Discard edits</button>' : ''}
        </div>
        <div id="pickedElement">${renderPickedElement(false)}</div>
      </div>
      <div class="section">
        <div class="section-head"><h3>Matched review pages</h3></div>
        ${renderRemovedPageMatches(matrix, templates)}
      </div>
    </div>`;
}

function renderRemovedPageTemplate(template, index, disabled) {
    const mode = template.regex !== undefined && template.regex !== null ? 'regex' : 'text';
    const value = mode === 'regex' ? template.regex : (template.text || '');
    return `<div class="template">
      <label class="field">Template ${index + 1} selector
        <span class="selector-row">
          <input class="mono" ${disabled} value="${escapeHtmlAttr(template.selector || '')}" oninput="setRemovedPageTemplate(${index}, {selector: this.value})">
          <button class="icon" onclick="postHighlight(currentRemovedPageTemplates()[${index}].selector, 'Removed notice', '#dc2626')" title="Highlight in the preview" aria-label="Highlight in the preview" type="button">${eyeIcon()}</button>
        </span>
      </label>
      <div class="rule-grid">
        <label class="field">Match
          <select ${disabled} onchange="setRemovedPageTemplateMode(${index}, this.value)">
            <option value="text" ${mode === 'text' ? 'selected' : ''}>Text</option>
            <option value="regex" ${mode === 'regex' ? 'selected' : ''}>Regex</option>
          </select>
        </label>
        <label class="field">${mode === 'regex' ? 'Notice regex' : 'Notice text'}
          <input ${disabled} value="${escapeHtmlAttr(value)}" oninput="setRemovedPageTemplateValue(${index}, this.value)">
        </label>
      </div>
      <div class="actions">
        <button class="quiet" ${disabled} onclick="usePickedForRemovedPageTemplate(${index})" type="button">Use picked element</button>
        <button class="quiet danger" ${disabled} onclick="removeRemovedPageTemplate(${index})" type="button">Remove</button>
      </div>
    </div>`;
}

// Mirrors `removed_page_matches` in the schema matrix: the review pages the templates match,
// and whether the scraper would capture each as removed or keep scraping it as a product
// because a candidate schema still extracts it. A template that takes a plausibly extracted
// title for its notice asks for a breadth check: it would withdraw that page, which is right only
// when the page is a removal notice that keeps the product template.
function renderRemovedPageMatches(matrix, templates) {
    if (!templates.length) return '<div class="small muted">Declare a template to see the pages it matches.</div>';
    if (dirty) return '<div class="small muted">Save the templates to evaluate them on the review pages.</div>';
    const matches = (matrix && matrix.removed_page_matches) || [];
    if (!matches.length) return '<div class="small muted">No review page matches the templates.</div>';
    return `<div class="data-grid-wrap"><table>
      <thead><tr><th>Page</th><th>Role</th><th>Outcome</th><th>Templates</th></tr></thead>
      <tbody>${matches.map(match => `<tr>
        <td>${pageUrlLink(match.url)}</td>
        <td>${escapeHtml(pageRoleLabel(match.role))}</td>
        <td>${match.outcome === 'REMOVED' ? badge('Removed', 'ok') : badge('Still a product', 'warn')}</td>
        <td>${(match.templates || []).map(t => `<div>Template ${t.template_index + 1}: ${escapeHtml((t.notices || []).join(' | '))}${t.product_title
            ? notice('warn', 'Check breadth', `takes the title “${escapeHtml(t.product_title)}” a candidate schema extracts for its notice: too broad unless this page is a removal notice`)
            : ''}</div>`).join('')}</td>
      </tr>`).join('')}</tbody>
    </table></div>`;
}

// An emptied list stays in the payload as `[]`: the server keeps the candidate's templates when
// the key is left out, so retiring the last template must send the empty list.
function editRemovedPageTemplates(edit) {
    const payload = schemasPayload();
    payload.removed_page_schemas = payload.removed_page_schemas || [];
    edit(payload.removed_page_schemas);
    selectedDetail.review.candidate_payload = payload;
    markDirty();
}

function setRemovedPageTemplate(index, changes) {
    editRemovedPageTemplates(templates => Object.assign(templates[index], changes));
}

function setRemovedPageTemplateMode(index, mode) {
    editRemovedPageTemplates(templates => {
        const template = templates[index];
        const value = template.regex ?? template.text ?? '';
        delete template.text;
        delete template.regex;
        template[mode] = value;
    });
    rerenderWorkbench();
}

function setRemovedPageTemplateValue(index, value) {
    editRemovedPageTemplates(templates => {
        const template = templates[index];
        if (template.regex !== undefined && template.regex !== null) template.regex = value;
        else template.text = value;
    });
}

function addRemovedPageTemplate() {
    editRemovedPageTemplates(templates => templates.push({selector: pickedSelector || '', text: ''}));
    rerenderWorkbench();
}

function usePickedForRemovedPageTemplate(index) {
    if (!pickedSelector) return;
    setRemovedPageTemplate(index, {selector: pickedSelector});
    rerenderWorkbench();
    postHighlight(pickedSelector, 'Removed notice', '#dc2626');
}

function removeRemovedPageTemplate(index) {
    editRemovedPageTemplates(templates => templates.splice(index, 1));
    rerenderWorkbench();
}

async function saveRemovedPageTemplates() {
    await saveCandidatePayload(schemasPayload());
}

// Every request the console makes for the selected review goes through here, so none fails
// unreported. The server refuses an edit or a verdict with a message (an invalid rule or template,
// a stale approved review, a page outside the ListingSource, a closed review, incomplete
// coverage); the message, with the HTTP status, or the network failure stays above the workbench
// with the unsaved edits so the reviewer can correct them. A request with a `successMessage`
// confirms itself with it. Returns whether the request went through.
async function submitEdit(edit, successMessage = '') {
    const reviewId = selectedId;
    editRefusal = null;
    if (successMessage) editNotice = null;
    try {
        await edit();
        if (successMessage && !editRefusal && selectedId === reviewId) {
            editNotice = {reviewId, message: successMessage};
            updateEditMessages();
        }
        return true;
    } catch (error) {
        editRefusal = {reviewId, message: apiErrorMessage(error)};
        const currentVersion = changedCandidateVersion(error);
        if (currentVersion !== null && selectedId === reviewId) {
            selectedReviewNeedsRefresh = true;
            selectedReviewRefreshReason = `The candidate changed elsewhere and is at version ${currentVersion}; your edits are not saved. Refresh the review and apply them again.`;
        }
        const coverage = refusedCoverage(error);
        if (coverage && selectedId === reviewId) {
            selectedReviewNeedsRefresh = true;
            selectedReviewRefreshReason = `Not approved: the review's coverage is ${coverageText({...coverage, notEvaluated: coverage.not_evaluated})}. Refresh the review and confirm its current coverage.`;
        }
        if (selectedId === reviewId) {
            if (selectedDetail?.review.review_id === reviewId) {
                rerenderWorkbench();
            } else {
                renderReviewLoadFailure();
            }
        }
        return false;
    }
}

// Sends `mutation`, confirms it with `successMessage`, and then reloads the review with `reload`:
// a reload that fails is reported apart from the mutation, which went through. Returns whether
// the mutation went through.
async function submitMutation(mutation, successMessage, reload) {
    if (!await submitEdit(mutation, successMessage)) return false;
    await submitEdit(reload);
    return true;
}

// Reloading the review discards its local edits; a reload that fails keeps them, as they are
// still on screen.
async function reloadDiscardingEdits(reload) {
    const edits = {dirty, sampleLabelDraft};
    dirty = false;
    sampleLabelDraft = null;
    const loaded = await submitEdit(reload);
    if (!loaded) {
        ({dirty, sampleLabelDraft} = edits);
        updateReloadState();
    }
    return loaded;
}

function renderReviewLoadFailure() {
    selectedDetail = null;
    selectedMatrix = null;
    document.getElementById('detail').innerHTML = notice('bad', 'Not loaded', `The review could not be loaded: ${escapeHtml(editRefusal?.message || '')}`,
        {role: 'alert', actions: '<button onclick="selectReview(selectedId)" type="button">Retry</button>'});
}

// Every candidate edit names the candidate version it was based on, so the server refuses it
// once another writer has changed the candidate.
function expectedCandidateVersion() {
    return selectedDetail.review.candidate_version;
}

// The saved edit is the candidate version the next edit builds on, even when reloading the
// review afterwards fails.
function recordSavedCandidateVersion(saved) {
    dirty = false;
    if (selectedDetail && Number.isInteger(saved?.candidate_version)) {
        selectedDetail.review.candidate_version = saved.candidate_version;
    }
}

function renderMatrixNotice(detail, matrix) {
    if (selectedMatrixMissing) {
        return notice('neutral', 'Not evaluated', 'The candidate has not been evaluated on the live review pages. Evaluating fetches every review page from the shop at its crawl delay.',
            {actions: '<button onclick="reevaluateMatrix()" type="button">Evaluate on live pages</button>'});
    }
    if (selectedMatrixFailure) {
        return notice('bad', 'Evaluation failed', escapeHtml(selectedMatrixFailure),
            {actions: '<button onclick="reevaluateMatrix()" type="button">Evaluate again</button>'});
    }
    const unevaluated = unevaluatedReviewPages(detail, matrix);
    if (!unevaluated.length) return '';
    return notice('warn', 'Not evaluated', `${unevaluated.length} page${unevaluated.length === 1 ? '' : 's'} joined the review after the last evaluation (${unevaluated.map(page => escapeHtml(reviewPageLabel(page))).join(', ')}). Evaluate again before approving.`,
        {actions: '<button onclick="reevaluateMatrix()" type="button">Evaluate on live pages</button>'});
}

function renderEditMessages() {
    const refusal = editRefusal && editRefusal.reviewId === selectedId
        ? notice('bad', 'Refused', escapeHtml(editRefusal.message), {role: 'alert', dismiss: "dismissEditMessage('refusal')"})
        : '';
    const saved = editNotice && editNotice.reviewId === selectedId
        ? notice('ok', 'Saved', escapeHtml(editNotice.message), {role: 'status', dismiss: "dismissEditMessage('notice')"})
        : '';
    return refusal + saved;
}

// Updates the messages alone, so unsaved edits in the workbench's inputs stay as typed.
function updateEditMessages() {
    const messages = document.getElementById('editMessages');
    if (messages) messages.innerHTML = renderEditMessages();
}

function dismissEditMessage(kind) {
    if (kind === 'refusal') {
        editRefusal = null;
    } else {
        editNotice = null;
    }
    updateEditMessages();
}

// The server's `error` with the HTTP status, or the network failure of a request that got no answer.
function apiErrorMessage(error) {
    const message = String((error && error.message) || error);
    let text = message;
    try {
        text = JSON.parse(message).error || message;
    } catch (_) {}
    if (error?.network) return `Request failed, the console server did not answer: ${text}`;
    if (Number.isInteger(error?.status)) {
        const status = `HTTP ${error.status}${error.statusText ? ` ${error.statusText}` : ''}`;
        return text ? `${status}: ${text}` : status;
    }
    return text;
}

function renderJsonPanel(detail) {
    return `<div class="tab-panel">
      <div class="section">
        <div class="section-head"><h3>Candidate JSON</h3></div>
        <div class="small muted">Use the Selector tab for normal edits; this is the raw candidate.</div>
        <textarea id="candidatePayload" aria-label="Candidate JSON" oninput="markDirty()">${escapeHtml(JSON.stringify(detail.review.candidate_payload, null, 2))}</textarea>
        <div class="actions">
          <button class="primary-outline" ${candidateEditable(detail) ? '' : 'disabled'} onclick="saveCandidate()" type="button">Save JSON</button>
          ${dirty ? '<button class="quiet" onclick="discardEdits()" type="button">Discard edits</button>' : ''}
        </div>
      </div>
      <div class="section">
        <div class="section-head"><h3>Validation summary</h3></div>
        <pre>${escapeHtml(JSON.stringify(detail.review.validation_summary, null, 2))}</pre>
      </div>
    </div>`;
}

// The review pages the candidate resolves, out of every review page, counted as the server
// counts them on approval: a page the removed-page templates capture as removed, or else one at
// least one schema applies to. A review page the matrix did not evaluate is not covered until the
// next evaluation, and without a matrix no page is evaluated.
function reviewPageCoverage(detail, matrix) {
    const evaluatedPages = matrix?.candidates?.[0]?.pages || [];
    const coverage = {covered: 0, removed: 0, total: 0, notEvaluated: 0};
    for (const page of detail?.pages || []) {
        coverage.total += 1;
        const evaluation = evaluatedPages.find(evaluated => evaluatesReviewPage(evaluated, page));
        if (!evaluation) {
            coverage.notEvaluated += 1;
        } else if (capturedAsRemoved(matrix, evaluation.page_reference)) {
            coverage.covered += 1;
            coverage.removed += 1;
        } else if (pageCovered(matrix, evaluation.page_reference)) {
            coverage.covered += 1;
        }
    }
    return coverage;
}

// A live matrix names a review page by its id; a generated matrix names the pages it was
// generated from by position, stored as the review's pages under the same URL.
function evaluatesReviewPage(evaluation, page) {
    const reference = evaluation.page_reference || {};
    if (reference.kind === 'persisted') return reference.review_page_id === page.review_page_id;
    return reference.kind === 'input' && evaluation.url === page.url;
}

// The candidate's removed-page templates match the page and no candidate schema still extracts
// it, so the scraper captures it as removed once the candidate is live.
function capturedAsRemoved(matrix, pageReference) {
    const referenceKey = pageReferenceKey(pageReference);
    return referenceKey !== '' && (matrix.removed_page_matches || []).some(match =>
        match.outcome === 'REMOVED' && pageReferenceKey(match.page_reference) === referenceKey
    );
}

// A review without pages shows nothing the candidate extracts, so its coverage is never complete.
function coverageComplete({covered, total}) {
    return total > 0 && covered === total;
}

function coverageText({covered, removed, total, notEvaluated}) {
    const resolved = removed
        ? ` (${covered - removed} by a schema, ${removed} captured as removed)`
        : ' by at least one schema';
    return `${covered} / ${total} live page${total === 1 ? '' : 's'} covered${resolved}${notEvaluated ? `; ${notEvaluated} not evaluated` : ''}`;
}

// The coverage an approval acknowledges, as the server reports it in a refusal.
function acknowledgedCoverage({covered, removed, total, notEvaluated}) {
    return {covered, removed, total, not_evaluated: notEvaluated};
}

// A 409 that carries `coverage` refused an approval because the review's coverage is not the one
// the console showed and the reviewer confirmed; the reviewer reloads the review to see it.
function refusedCoverage(error) {
    if (error?.status !== 409) return null;
    try {
        const coverage = JSON.parse(error.message).coverage;
        return coverage && Number.isInteger(coverage.total) ? coverage : null;
    } catch (_) {
        return null;
    }
}

function renderCoverageSection(matrix) {
    if (!matrix || !matrix.candidates || !matrix.candidates.length) {
        return '<div class="section"><div class="section-head"><h3>Coverage</h3></div><div class="small muted">No schema coverage data available.</div></div>';
    }
    const coverage = reviewPageCoverage(selectedDetail, matrix);
    const tone = coverageComplete(coverage) ? 'ok' : (coverage.covered === 0 ? 'bad' : 'warn');
    const evaluated = matrix.live_evaluation?.fetched_at;
    return `<div class="section">
      <div class="section-head"><h3>Coverage</h3>${badge(`${coverage.covered} of ${coverage.total} page${coverage.total === 1 ? '' : 's'}${coverage.notEvaluated ? ` · ${coverage.notEvaluated} not evaluated` : ''}`, tone)}</div>
      <div class="small muted">${escapeHtml(coverageSentences(matrix, coverage))}</div>
      <div class="actions">
        <button onclick="reevaluateMatrix()" title="Evaluate the saved candidate on the live review pages" type="button">Evaluate on live pages</button>
        ${matrix.live_evaluation ? `<span class="small muted">evaluated ${evaluated ? timeBadge(evaluated, 'Evaluated') : 'never'}</span>` : ''}
      </div>
    </div>`;
}

// Whether every live page is covered, how the selected schema fares and which schemas apply to
// no page at all.
function coverageSentences(matrix, coverage) {
    const parts = [coverageComplete(coverage)
        ? 'Every live page is covered by at least one schema.'
        : `${coverage.covered} of ${coverage.total} pages are covered${coverage.notEvaluated ? `; ${coverage.notEvaluated} not evaluated` : ''}.`];
    const candidate = selectedCandidate(matrix);
    const pages = candidate?.pages || [];
    if (pages.length) {
        const failures = pages.filter(page => !page.apply_ok);
        const label = schemaLabel(selectedSchemaIndex);
        parts.push(failures.length
            ? `${label} applies to ${pages.length - failures.length} of ${pages.length}; it fails on ${failures.map(page => pageRoleLabel(page.role)).join(', ')}.`
            : `${label} applies to all ${pages.length}.`);
    }
    for (const unused of matrix.candidates.filter(c => (c.pages || []).length && c.pages.every(page => !page.apply_ok))) {
        parts.push(`${schemaLabel(unused.schema_index)} applies to none — add a page of its template or delete it.`);
    }
    return parts.join(' ');
}

// Check codes of `plausibility_findings`, `cross_page_findings` and
// `identity_agreement_findings` in the schema matrix.
const plausibilityCheckDescriptions = {
    BLANK: 'is blank',
    TOO_LONG: 'is longer than a label',
    ALL_ZERO_DIGITS: 'has only zero digits',
    MARKUP_TOKENS: 'contains CSS or script tokens',
    DUPLICATE_VALUES: 'repeats a value',
    INVALID_IDENTIFIER: 'is not a valid source listing id',
    WORD_LIST: 'is a list of words, not an identifier',
    PRICE_TEXT: 'is a price, not an availability label',
    REPEATED_ACROSS_PAGES: 'repeats a value of another product page',
    NO_IMAGE_ON_ANY_PAGE: 'yields no capturable image on any review page',
    DIFFERS_FROM_OTHER_SCHEMAS: 'differs from the identity another schema derives from this page',
    DIFFERS_FROM_CAPTURED_IDENTITY: 'differs from the identity the listing was captured under'
};

// The per-page, cross-page and identity agreement checks a matrix page failed. Any of
// them keeps a generated schema out of auto-approval.
function pageFindings(page) {
    return [
        ...(page?.plausibility_findings || []),
        ...(page?.cross_page_findings || []),
        ...(page?.identity_agreement_findings || [])
    ];
}

function fieldFindings(page, field) {
    return pageFindings(page).filter(finding => finding.field === fieldKey(field));
}

function findingDescription(finding) {
    return `${displayName(finding.field)} ${plausibilityCheckDescriptions[finding.check] || finding.check}`;
}

// The short words of the checks and notes, for the badge next to a field.
const CHECK_LABELS = {
    BLANK: 'Blank',
    TOO_LONG: 'Too long',
    ALL_ZERO_DIGITS: 'Zero price',
    MARKUP_TOKENS: 'Markup',
    DUPLICATE_VALUES: 'Duplicate',
    INVALID_IDENTIFIER: 'Invalid ID',
    WORD_LIST: 'Word list',
    PRICE_TEXT: 'Price text',
    REPEATED_ACROSS_PAGES: 'Repeated across pages',
    NO_IMAGE_ON_ANY_PAGE: 'No image on any page',
    DIFFERS_FROM_OTHER_SCHEMAS: 'Differs from other schema',
    DIFFERS_FROM_CAPTURED_IDENTITY: 'Differs from captured identity'
};

function checkLabel(check) {
    return CHECK_LABELS[check] || displayName(check);
}

function findingBadge(finding) {
    return `<span class="badge badge-bad" title="${escapeHtmlAttr(findingDescription(finding))}">${escapeHtml(checkLabel(finding.check))}</span>`;
}

// The selected schema's pages that failed a check, by the check's description, in page order.
function groupFindingsByDescription(candidate) {
    const groups = new Map();
    for (const page of candidate?.pages || []) {
        for (const description of new Set(pageFindings(page).map(findingDescription))) {
            groups.set(description, [...(groups.get(description) || []), page]);
        }
    }
    return groups;
}

// A page named by its role and path, with its full URL as the tooltip.
function pageTag(page) {
    const url = matrixPageUrl(page);
    return `<span class="page-tag"><span class="role">${escapeHtml(pageRoleLabel(page.role))}</span>${url ? `<code title="${escapeHtmlAttr(url)}">${escapeHtml(urlPath(url))}</code>` : ''}</span>`;
}

function checkEntry(tone, description, detail, pages) {
    return `<div class="check">
      <div class="check-head ${tone}"><strong>${escapeHtml(description)}</strong><span class="small muted">${escapeHtml(detail)}</span></div>
      <div class="check-pages">${pages.map(pageTag).join('')}</div>
    </div>`;
}

function renderFailedChecksSection(matrix) {
    const candidate = selectedCandidate(matrix);
    const groups = groupFindingsByDescription(candidate);
    if (!groups.size) return '';
    const total = (candidate.pages || []).length;
    const failing = (candidate.pages || []).filter(page => pageFindings(page).length > 0).length;
    return `<div class="section">
      <div class="section-head"><h3><span class="dot dot-bad"></span>Failed checks</h3>${badge(`${groups.size} check${groups.size === 1 ? '' : 's'} · ${failing} page${failing === 1 ? '' : 's'}`, 'bad')}</div>
      <div class="small muted">A failed check keeps a generated schema out of auto-approval; a human approves it knowingly.</div>
      <div class="check-list">${[...groups].map(([description, pages]) =>
          checkEntry('bad', description, `${schemaLabel(selectedSchemaIndex)} · ${pages.length} of ${total} page${total === 1 ? '' : 's'}`, pages)).join('')}</div>
    </div>`;
}

// The optional fields a page left empty and no failed check names: a field that failed a check on
// the page is listed under Failed checks already.
function notedOptionalMisses(page) {
    return optionalFieldMisses(page).filter(field => fieldFindings(page, field.field).length === 0);
}

// Optional rules that left their field empty and rules that skipped matched elements, grouped by
// field and kind: informational lines next to the notes, none of which blocks an approval.
function optionalMissLines(candidate) {
    const lines = new Map();
    const add = (key, text, page) => {
        const entry = lines.get(key) || {text, pages: []};
        entry.pages.push(page);
        lines.set(key, entry);
    };
    for (const page of candidate?.pages || []) {
        for (const field of notedOptionalMisses(page)) {
            add(`${field.field}:${matchedOnlyEmptyElements(field)}`, {field: displayName(field.field), empty: matchedOnlyEmptyElements(field)}, page);
        }
    }
    return [...lines.values()].map(({text, pages}) => {
        const roles = [...new Set(pages.map(page => pageRoleLabel(page.role)))].join(', ');
        const count = `${pages.length} page${pages.length === 1 ? '' : 's'}`;
        return text.empty
            ? `${text.field} left empty on ${count} (matched only empty elements): ${roles}. The page still applies.`
            : `${text.field} matched nothing on ${count}: ${roles}. The page still applies.`;
    });
}

function skippedElementLines(candidate) {
    const lines = new Map();
    for (const page of candidate?.pages || []) {
        for (const field of fieldsWithSkippedElements(page)) {
            const gallery = field.skipped_element_count === matchedElementCount(field);
            const key = `${field.field}:${gallery}`;
            const entry = lines.get(key) || {field: displayName(field.field), gallery, skipped: 0, matched: 0, pages: 0};
            entry.skipped += field.skipped_element_count;
            entry.matched += matchedElementCount(field);
            entry.pages += 1;
            lines.set(key, entry);
        }
    }
    return [...lines.values()].map(entry =>
        `${entry.field} skipped ${entry.skipped} of ${entry.matched} matched elements without an image URL on ${entry.pages} page${entry.pages === 1 ? '' : 's'}${entry.gallery ? ' — an empty gallery' : ' (loose selector)'}`);
}

function renderNotesSection(matrix) {
    const candidate = selectedCandidate(matrix);
    const lines = [...optionalMissLines(candidate), ...skippedElementLines(candidate)];
    if (!lines.length) return '';
    const pageCount = (candidate.pages || []).filter(page =>
        notedOptionalMisses(page).length || fieldsWithSkippedElements(page).length).length;
    const notes = lines.length;
    return `<div class="section">
      <div class="section-head"><h3><span class="dot dot-warn"></span>Notes</h3>${badge(`${notes} note${notes === 1 ? '' : 's'} · ${pageCount} page${pageCount === 1 ? '' : 's'}`, 'warn')}</div>
      <div class="small muted">Information next to its field; notes block no approval.</div>
      <div class="check-list">
        ${lines.map(line => `<div class="small muted">${escapeHtml(line)}</div>`).join('')}
      </div>
    </div>`;
}

function renderExtractedSection(matrix) {
    const candidate = selectedCandidate(matrix);
    if (!candidate) return '<div class="section"><div class="small muted">No extracted data for this schema.</div></div>';
    const page = selectedMatrixPage(candidate);
    if (!page) return '<div class="section"><div class="small muted">No extracted data for the selected page.</div></div>';
    const findings = pageFindings(page);
    const status = page.apply_ok
        ? badge('Applied', 'ok')
        : (findings.length ? badge(`Failed · ${findings.length} check${findings.length === 1 ? '' : 's'}`, 'bad') : badge('Did not apply', 'bad'));
    const url = matrixPageUrl(page);
    // A rejecting plausibility finding fails the page but keeps its extraction, so the
    // reviewer sees the failing check next to the field it concerns.
    return `<div class="section">
      <div class="section-head"><h3>Extracted from this page</h3>${status}</div>
      <div class="small muted">${escapeHtml(schemaLabel(selectedSchemaIndex))} · ${escapeHtml(pageRoleLabel(page.role))}${url ? ` · <code title="${escapeHtmlAttr(url)}">${escapeHtml(urlPath(url))}</code>` : ''}</div>
      ${page.apply_ok ? '' : `<div class="small bad-text">${expandableValue(page.error || 'Schema did not apply.')}</div>`}
      ${page.extracted ? rawDataCell(page.extracted, page) : '<div class="small muted">No extracted data.</div>'}
    </div>`;
}
// Cross-page findings name the values another product page repeats; for a list such as
// the images they point out the default share image among the product photos, which lists
// like the field's own images, by its resolved first candidate.
function renderSharedValues(findings, key) {
    const values = findings.flatMap(finding => finding.values || []);
    if (!values.length) return '';
    return `<div class="small muted">Also extracted from another product page: ${key === 'images'
        ? renderImageCandidateGroups(values)
        : values.map(value => expandableValue(value)).join(', ')}</div>`;
}

// Identity agreement findings name the identities that contradict the extracted one: the
// other schemas' on the same page, or the one the listing was captured under.
function renderContradictingIdentities(findings) {
    const values = findings
        .filter(finding => finding.check === 'DIFFERS_FROM_OTHER_SCHEMAS' || finding.check === 'DIFFERS_FROM_CAPTURED_IDENTITY')
        .flatMap(finding => (finding.values || []).map(value =>
            `${finding.check === 'DIFFERS_FROM_CAPTURED_IDENTITY' ? 'captured' : 'other schema'}: ${expandableValue(value)}`
        ));
    if (!values.length) return '';
    return `<div class="small muted">Contradicting identities: ${values.join(', ')}</div>`;
}

// Mirrors `optional_field_misses` in the persisted schema matrix summary: a soft
// rule (not required, no selector error, not `images`, whose empty-container case is
// decided by the page-level apply result) whose primary and additional selectors all
// matched nothing with visible text on a page that still applied.
function optionalFieldMisses(page) {
    if (!page?.apply_ok) return [];
    return (page.fields || []).filter(field =>
        !field.required
        && !field.error
        && field.field !== 'images'
        && field.selector_match_count === 0
        && (field.additional_selector_match_counts || []).every(count => count === 0)
    );
}

// Fields whose rule skipped matched elements without an image URL (no image URL candidate, or
// no attribute of an attribute rule) on the page (`skipped_element_count`, set for the `images`
// rule only). Next to extracted images, skips show a loose selector.
function fieldsWithSkippedElements(page) {
    return (page?.fields || []).filter(field => field.skipped_element_count > 0);
}

function matchedElementCount(field) {
    return [field.selector_match_count || 0, ...(field.additional_selector_match_counts || [])]
        .reduce((sum, count) => sum + count, 0);
}

function pageReferenceKey(reference) {
    if (!reference || typeof reference !== 'object') return '';
    if (reference.kind === 'persisted' && reference.review_page_id) {
        return `persisted:${reference.review_page_id}`;
    }
    if (reference.kind === 'input' && Number.isInteger(reference.page_index)) {
        return `input:${reference.page_index}`;
    }
    return '';
}

function pageCovered(matrix, pageReference) {
    const referenceKey = pageReferenceKey(pageReference);
    return referenceKey !== '' && matrix.candidates.some(candidate =>
        (candidate.pages || []).some(page =>
            pageReferenceKey(page.page_reference) === referenceKey && page.apply_ok
        )
    );
}

function currentMatrixPage(matrix) {
    const candidate = matrix && selectedCandidate(matrix);
    return candidate ? selectedMatrixPage(candidate) : null;
}

// A missed rule whose selectors found elements, all without visible text: the page leaves
// the field empty where the rule expects it, rather than the rule not fitting the page.
function matchedOnlyEmptyElements(field) {
    return field.empty_element_count > 0;
}

// A live matrix names the review pages it evaluated and leaves out a page that joined while it
// was evaluated; a generated one references the pages it was given by position.
function selectedMatrixPage(candidate) {
    const pages = candidate.pages || [];
    const selected = pages.find(page => pageReferenceKey(page.page_reference) === `persisted:${selectedPageId}`);
    if (selected) return selected;
    return isLiveMatrixCandidate(candidate) ? null : pages[0] || null;
}

function isLiveMatrixCandidate(candidate) {
    return (candidate.pages || []).some(page => page.page_reference?.kind === 'persisted');
}

function unevaluatedReviewPages(detail, matrix) {
    const candidate = (matrix.candidates || [])[0];
    if (!candidate || !isLiveMatrixCandidate(candidate)) return [];
    const evaluated = new Set(candidate.pages.map(page => pageReferenceKey(page.page_reference)));
    return (detail.pages || []).filter(page => !evaluated.has(`persisted:${page.review_page_id}`));
}

function selectedCandidate(matrix) {
    return matrix.candidates.find(c => c.schema_index === selectedSchemaIndex);
}

// A field with a failed plausibility check stays listed even when it is blank.
function rawDataCell(raw, page) {
    if (!raw) return '<div class="small muted">No extracted data.</div>';
    const fields = Object.entries(raw).filter(([key, value]) =>
        hasRawValue(value) || fieldFindings(page, key).length > 0
    );
    if (!fields.length) return '<div class="small muted">No extracted data.</div>';
    return `<dl class="kv">${
        fields.map(([key, value]) => {
            const findings = fieldFindings(page, key);
            const badges = findings.map(findingBadge).join('');
            const shown = hasRawValue(value)
                ? (key === 'images' && Array.isArray(value) ? renderImageCandidateGroups(value) : rawValue(value))
                : '<span class="blank">blank</span>';
            return `<dt${findings.length ? ' class="bad"' : ''}>${escapeHtml(displayName(key))}</dt>
        <dd>${shown}${badges ? ` ${badges}` : ''}${Array.isArray(value) ? renderSharedValues(findings, key) : renderContradictingIdentities(findings)}</dd>`;
        }).join('')
    }</dl>`;
}

// How the diff words each field status; `same` is plain muted text, not a badge.
function diffBadge(status) {
    if (status === 'changed') return badge('Changed', 'accent');
    if (status === 'missing current') return badge('Only in compare');
    if (status === 'missing compare') return badge('Only in selected');
    return '<span class="small muted">same</span>';
}

function renderSchemaDiff(current, compare) {
    const rawAttributeFields = Array.from(new Set(Object.keys(current?.raw_attributes || {}).concat(Object.keys(compare?.raw_attributes || {}))))
        .map(field => `raw_attributes.${field}`);
    const fields = [...selectorFields, ...rawAttributeFields];
    return `<div class="diff">${
        fields.map(field => {
            const left = normalizeRuleValue(schemaFieldValue(current, field));
            const right = normalizeRuleValue(schemaFieldValue(compare, field));
            const status = diffStatus(left, right);
            return `<div class="diff-row">
              <div class="diff-head"><span>${escapeHtml(fieldLabel(field))}</span>${diffBadge(status)}</div>
              ${status === 'same' && !left ? '' : `<div class="diff-values">
                <div class="diff-value"><span class="diff-label">Selected</span><code>${expandableValue(left || 'not configured')}</code></div>
                <div class="diff-value"><span class="diff-label">Compare</span><code>${expandableValue(right || 'not configured')}</code></div>
              </div>`}
            </div>`;
        }).join('')
    }</div>`;
}

function schemaFieldValue(schema, field) {
    if (!schema) return undefined;
    if (!field.startsWith('raw_attributes.')) return schema[field];
    return (schema.raw_attributes || {})[field.slice('raw_attributes.'.length)];
}

function diffStatus(left, right) {
    if (!left && !right) return 'same';
    if (!left) return 'missing current';
    if (!right) return 'missing compare';
    return left === right ? 'same' : 'changed';
}

function normalizeRuleValue(value) {
    if (value === undefined || value === null) return '';
    return stableStringify(value);
}

function stableStringify(value) {
    if (Array.isArray(value)) return JSON.stringify(value.map(stableValue), null, 2);
    return JSON.stringify(stableValue(value), null, 2);
}

function stableValue(value) {
    if (Array.isArray(value)) return value.map(stableValue);
    if (!value || typeof value !== 'object') return value;
    return Object.keys(value).sort().reduce((acc, key) => {
        acc[key] = stableValue(value[key]);
        return acc;
    }, {});
}

// An extracted image is a candidate group: the address the page shows first, then its
// fallbacks (srcset sizes, data attributes), joined by the scraper's U+001F separator. The list
// shows one image per line by its first candidate, resolved against the selected page as the
// crawler resolves it, and keeps the whole group in the tooltip; a long list collapses.
const IMAGE_CANDIDATE_SEPARATOR = '\u001f';
const IMAGES_SHOWN_BEFORE_COLLAPSE = 5;

function renderImageCandidateGroups(groups) {
    const rows = groups.map(renderImageCandidateGroup);
    const shown = rows.slice(0, IMAGES_SHOWN_BEFORE_COLLAPSE).join('');
    const rest = rows.slice(IMAGES_SHOWN_BEFORE_COLLAPSE);
    return `<ol class="image-list">${shown}</ol>${rest.length
        ? `<details class="expandable"><summary>${rest.length} more image${rest.length === 1 ? '' : 's'}</summary><ol class="image-list" start="${IMAGES_SHOWN_BEFORE_COLLAPSE + 1}">${rest.join('')}</ol></details>`
        : ''}`;
}

function renderImageCandidateGroup(group) {
    const candidates = imageCandidates(group);
    const [first = ''] = candidates;
    const address = resolvedPageAddress(first);
    const fallbacks = candidates.length - 1;
    const tooltip = escapeHtmlAttr(candidates.map(resolvedPageAddress).join('\n'));
    const label = escapeHtml(shortImageAddress(address));
    return `<li title="${tooltip}">${/^https?:\/\//i.test(address)
        ? `<a href="${escapeHtmlAttr(address)}" target="_blank" rel="noopener noreferrer">${label}</a>`
        : `<span>${label}</span>`}${fallbacks > 0 ? ` <span class="xs muted">+${fallbacks} fallback${fallbacks === 1 ? '' : 's'}</span>` : ''}</li>`;
}

// Image addresses of one shop share their whole prefix, so the list shows the file name, which
// tells them apart; the tooltip and the link keep the full address.
function shortImageAddress(address) {
    try {
        const file = new URL(address).pathname.split('/').filter(Boolean).pop() || '';
        return file ? `…/${decodeURIComponent(file)}` : address;
    } catch (_) {
        return address;
    }
}

// The review page a matrix page was evaluated on; a generated matrix names its pages by URL.
function matrixPageUrl(page) {
    const reference = page.page_reference;
    if (reference?.kind !== 'persisted') return page.url || '';
    const reviewPage = (selectedDetail?.pages || []).find(candidate => candidate.review_page_id === reference.review_page_id);
    return reviewPage ? reviewPage.url : '';
}

function imageCandidates(group) {
    return String(group ?? '').split(IMAGE_CANDIDATE_SEPARATOR).map(candidate => candidate.trim()).filter(Boolean);
}

function resolvedPageAddress(value) {
    const page = (selectedDetail?.pages || []).find(candidate => candidate.review_page_id === selectedPageId);
    try {
        return page ? new URL(value, page.url).href : value;
    } catch (_) {
        return value;
    }
}

function hasRawValue(value) {
    if (value === null || value === undefined) return false;
    if (Array.isArray(value)) return value.length > 0;
    if (typeof value === 'string') return value.trim().length > 0;
    return true;
}

function rawValue(value) {
    if (Array.isArray(value)) {
        return `<div class="raw-list">${value.map(item => `<div>${expandableValue(item)}</div>`).join('')}</div>`;
    }
    if (typeof value === 'object' && value !== null) {
        return expandableValue(JSON.stringify(value, null, 2));
    }
    return expandableValue(value);
}

function expandableValue(value) {
    const text = String(value ?? '');
    if (text.length <= 140) return linkifyText(text);
    return `<details class="expandable">
    <summary>${linkifyText(text.slice(0, 140))}<span class="ellipsis">...</span></summary>
    <pre class="full-values">${linkifyText(text)}</pre>
  </details>`;
}

function linkifyText(text) {
    const urlPattern = /\bhttps?:\/\/[^\s<>"']+/g;
    let html = '';
    let lastIndex = 0;
    for (const match of text.matchAll(urlPattern)) {
        const url = trimUrlPunctuation(match[0]);
        const start = match.index;
        const end = start + url.length;
        html += escapeHtml(text.slice(lastIndex, start));
        html += `<a href="${escapeHtmlAttr(url)}" target="_blank" rel="noopener noreferrer">${escapeHtml(url)}</a>`;
        lastIndex = end;
    }
    html += escapeHtml(text.slice(lastIndex));
    return html;
}

function trimUrlPunctuation(url) {
    return url.replace(/[),.;\]]+$/, '');
}

// The path of `url`, which tells the pages of one shop apart; a value that is no URL stays as it is.
function urlPath(url) {
    try {
        const parsed = new URL(url);
        return `${parsed.pathname}${parsed.search}`;
    } catch (_) {
        return String(url ?? '');
    }
}

// A page URL shown as its path, with the full URL as the tooltip and, for an http(s) URL, the link.
function pageUrlLink(url) {
    const label = escapeHtml(urlPath(url));
    if (!isHttpUrl(url)) return `<span class="url mono" title="${escapeHtmlAttr(url)}">${label}</span>`;
    return `<a class="url mono" href="${escapeHtmlAttr(url)}" target="_blank" rel="noopener noreferrer" title="${escapeHtmlAttr(url)}">${label}</a>`;
}

function setWorkbenchPanel(panel) {
    activeWorkbenchPanel = panel;
    rerenderWorkbench();
}

function rerenderWorkbench() {
    renderDetail(selectedDetail, selectedMatrix);
}

async function reloadMatrix() {
    dirty = false;
    await loadSelectedReview(false, true);
    await loadReviews();
}

// Re-evaluating reloads the review's candidate, so it asks before discarding unsaved edits,
// and with them a refusal of those edits. The matrix, or why there is none, shows its outcome.
async function reevaluateMatrix() {
    await confirmDiscardingEdits('Re-evaluating', () => {
        editNotice = null;
        return reloadDiscardingEdits(reloadMatrix);
    });
}

// Loading the current review discards the local edits, and with them a refusal of those edits.
async function refreshSelectedReview() {
    editNotice = null;
    pendingAction = null;
    return reloadDiscardingEdits(async () => {
        await loadSelectedReview(false);
        await loadReviews();
    });
}

function schemasPayload() {
    const textarea = document.getElementById('candidatePayload');
    if (textarea) return JSON.parse(textarea.value);
    return selectedDetail.review.candidate_payload;
}

function currentSchemas() {
    return (((selectedDetail.review || {}).candidate_payload || {}).schemas) || [];
}

function currentSchema() {
    return currentSchemas()[selectedSchemaIndex] || {};
}

function currentRule() {
    return schemaFieldValue(currentSchema(), selectedField) || null;
}

function isRawAttributeField(field) {
    return field.startsWith(RAW_ATTRIBUTE_FIELD_PREFIX);
}

// The key of a field in its schema object, and in the matrix: a raw attribute's registry key.
function fieldKey(field) {
    return isRawAttributeField(field) ? field.slice(RAW_ATTRIBUTE_FIELD_PREFIX.length) : field;
}

function fieldLabel(field) {
    return isRawAttributeField(field) ? `Raw attribute ${fieldKey(field)}` : displayName(field);
}

function fieldDeletable(field) {
    return optionalFields.has(field) || isRawAttributeField(field);
}

function fieldHighlightColor(field) {
    return isRawAttributeField(field) ? rawAttributeHighlightColor : (schemaHighlightColors[field] || '#2563eb');
}

// The core fields, then the raw attributes the schema configures.
function schemaFields(schema) {
    return [
        ...selectorFields,
        ...Object.keys(schema.raw_attributes || {}).sort().map(key => RAW_ATTRIBUTE_FIELD_PREFIX + key)
    ];
}

function currentSelector() {
    return (currentRule() || {}).selector || '';
}

function setCurrentSelector(selector) {
    const rule = ensureCurrentRule();
    rule.selector = selector;
    const textarea = document.getElementById('candidatePayload');
    if (textarea) textarea.value = JSON.stringify(selectedDetail.review.candidate_payload, null, 2);
    markDirty();
}

function defaultRuleFor(field) {
    if (field === 'images') return {
        selector: '',
        additional_selectors: [],
        type: 'attribute',
        name: 'src',
        cardinality: 'all'
    };
    return {selector: '', additional_selectors: [], type: 'text', cardinality: 'first'};
}

function defaultSchema() {
    return {
        source_listing_id: defaultRuleFor('source_listing_id'),
        title: defaultRuleFor('title'),
        description: null,
        price: null,
        price_estimate_min: null,
        price_estimate_max: null,
        state: defaultRuleFor('state'),
        images: defaultRuleFor('images'),
        raw_attributes: {}
    };
}

function ensureCurrentRule() {
    const payload = schemasPayload();
    payload.schemas = payload.schemas || [];
    const schema = payload.schemas[selectedSchemaIndex] = payload.schemas[selectedSchemaIndex] || defaultSchema();
    const rules = isRawAttributeField(selectedField)
        ? (schema.raw_attributes = schema.raw_attributes || {})
        : schema;
    const key = fieldKey(selectedField);
    rules[key] = rules[key] || defaultRuleFor(selectedField);
    rules[key].additional_selectors = rules[key].additional_selectors || [];
    selectedDetail.review.candidate_payload = payload;
    return rules[key];
}

function applyPickedSelector() {
    if (!pickedSelector) return;
    setCurrentSelector(pickedSelector);
    const input = document.getElementById('selectedSelector');
    if (input) input.value = pickedSelector;
    postHighlight(pickedSelector);
}

function setCurrentRuleCardinality(cardinality) {
    const rule = ensureCurrentRule();
    rule.cardinality = cardinality;
    markDirty();
}

// The extraction kinds (`ExtractionKind` in rule.rs) that read a named attribute.
function ruleTypeTakesAttributeName(type) {
    return type === 'attribute' || type === 'attribute_or_text';
}

function setCurrentRuleType(type) {
    const rule = ensureCurrentRule();
    rule.type = type;
    if (type === 'attribute') {
        rule.name = rule.name || (selectedField === 'images' ? 'src' : 'href');
    } else if (type === 'attribute_or_text') {
        rule.name = rule.name || 'value';
    } else {
        delete rule.name;
    }
    if (type === 'json_ld_offer') {
        rule.property = ['availability', 'price'].includes(rule.property) ? rule.property : (selectedField === 'state' ? 'availability' : 'price');
    } else if (type === 'json_ld_product') {
        rule.property = rule.property === 'sku' ? 'sku' : 'product_id';
    } else {
        delete rule.property;
    }
    markDirty();
    rerenderWorkbench();
}

function setCurrentRuleOfferProperty(property) {
    const rule = ensureCurrentRule();
    rule.property = property;
    markDirty();
}

function setCurrentRuleAttributeName(name) {
    const rule = ensureCurrentRule();
    rule.name = name;
    markDirty();
}

function additionalSelectorAt(index) {
    const rule = currentRule();
    return ((rule && rule.additional_selectors) || [])[index] || '';
}

function addAdditionalSelector() {
    const rule = ensureCurrentRule();
    rule.additional_selectors.push('');
    markDirty();
    rerenderWorkbench();
}

function addPickedAdditionalSelector() {
    if (!pickedSelector) return;
    const rule = ensureCurrentRule();
    rule.additional_selectors.push(pickedSelector);
    markDirty();
    rerenderWorkbench();
    postHighlight(pickedSelector);
}

function setAdditionalSelector(index, selector) {
    const rule = ensureCurrentRule();
    rule.additional_selectors[index] = selector;
    markDirty();
}

function removeAdditionalSelector(index) {
    const rule = ensureCurrentRule();
    rule.additional_selectors.splice(index, 1);
    markDirty();
    rerenderWorkbench();
}

async function saveCurrentField() {
    const rule = schemaFieldValue(schemasPayload().schemas[selectedSchemaIndex], selectedField) || null;
    await saveSchemaField(rule);
}

async function deleteCurrentRule() {
    if (!fieldDeletable(selectedField)) return;
    await saveSchemaField(null);
}

async function saveSchemaField(rule) {
    await submitMutation(async () => {
        const saved = await api(`/api/reviews/${selectedId}/schema-field`, {
            method: 'POST',
            body: JSON.stringify({
                expected_candidate_version: expectedCandidateVersion(),
                schema_index: selectedSchemaIndex,
                field: selectedField,
                rule
            })
        });
        recordSavedCandidateVersion(saved);
    }, rule ? 'Field saved' : 'Rule deleted', reloadSavedCandidate);
}

const DISCARDS_EDITS = 'reloads the review and discards its unsaved edits.';

// Re-evaluating the matrix, or adding or removing a review page, reloads the review and
// evaluates the matrix on its page set, so unsaved edits would be lost: `run` waits for the
// reviewer to discard them.
function confirmDiscardingEdits(what, run) {
    if (!dirty) return run();
    openActionPanel({
        title: 'Discard unsaved edits?',
        message: `${what} ${DISCARDS_EDITS}`,
        confirmLabel: 'Discard edits and continue',
        danger: true,
        run
    });
}

async function addReviewPage() {
    const input = document.getElementById('livePreviewUrl');
    const url = ((input && input.value) || '').trim();
    if (!url) return;
    await confirmDiscardingEdits('Adding the page', () => submitMutation(async () => {
        const page = await api(`/api/reviews/${selectedId}/pages`, {method: 'POST', body: JSON.stringify({url})});
        selectedPageId = page.review_page_id;
        previewUrlOverride = '';
    }, 'Page added', reloadMatrix));
}

function removeReviewPage() {
    const page = (selectedDetail.pages || []).find(p => p.review_page_id === selectedPageId);
    if (!pageRemovable(page)) return;
    openActionPanel({
        title: 'Remove review page?',
        message: `Remove ${page.url} from the review pages?${dirty ? ` Removing it ${DISCARDS_EDITS}` : ''}`,
        confirmLabel: 'Remove page',
        danger: true,
        run: () => submitMutation(async () => {
            await api(`/api/review-pages/${page.review_page_id}`, {method: 'DELETE'});
            selectedPageId = null;
        }, 'Page removed', reloadMatrix)
    });
}

function postHighlight(selector, label = fieldLabel(selectedField), color = fieldHighlightColor(selectedField)) {
    const frame = document.getElementById('snapshotFrame');
    if (frame && frame.contentWindow) frame.contentWindow.postMessage({
        type: 'crawler-review-highlight-selector',
        selector,
        label,
        color
    }, '*');
}

function postHighlightSchema() {
    const frame = document.getElementById('snapshotFrame');
    if (!frame || !frame.contentWindow) return;
    frame.contentWindow.postMessage({
        type: 'crawler-review-highlight-schema',
        rules: schemaHighlightRules(currentSchema())
    }, '*');
}

function schemaHighlightRules(schema) {
    const rules = [];
    for (const field of selectorFields) {
        const rule = schema && schema[field];
        if (!rule || !rule.selector) continue;
        const selectors = [rule.selector].concat(rule.additional_selectors || []).filter(Boolean);
        if (!selectors.length) continue;
        rules.push({field: displayName(field), selectors, color: schemaHighlightColors[field] || '#2563eb'});
    }
    for (const [field, rule] of Object.entries((schema && schema.raw_attributes) || {})) {
        if (!rule || !rule.selector) continue;
        const selectors = [rule.selector].concat(rule.additional_selectors || []).filter(Boolean);
        if (!selectors.length) continue;
        rules.push({field: fieldLabel(RAW_ATTRIBUTE_FIELD_PREFIX + field), selectors, color: rawAttributeHighlightColor});
    }
    return rules;
}

// A kept frame takes the highlight at once; a new frame once its page has loaded.
function highlightPreview(frameKept) {
    const frame = document.getElementById('snapshotFrame');
    if (!frame) return;
    const highlightSelectedField = () => {
        if (activeWorkbenchPanel === 'selector') postHighlight(currentSelector());
    };
    if (frameKept) {
        highlightSelectedField();
    } else {
        frame.addEventListener('load', highlightSelectedField, {once: true});
    }
}

// Approve takes the stored candidate, so unsaved edits are saved or discarded first; it never
// approves past them.
function approveReview() {
    if (dirty) {
        openActionPanel({
            title: 'Unsaved edits',
            message: 'Approve takes the stored candidate, not your unsaved edits. Save them with their Save button, or discard them, before approving.',
            confirmLabel: 'Discard edits',
            danger: true,
            run: discardEdits
        });
        return;
    }
    // A product-schema candidate that does not cover every review page is approved only once the
    // reviewer confirms the counts; the approval names them, and the server refuses it without
    // them or once the review's coverage differs from them.
    const shownCoverage = incompleteApprovalCoverage();
    openActionPanel({
        title: 'Approve?',
        message: shownCoverage
            ? `Coverage is incomplete: ${coverageText(shownCoverage)}. Approving confirms these counts.`
            : '',
        notes: 'optional',
        confirmLabel: shownCoverage ? 'Approve anyway' : 'Approve',
        // The panel stays open while the reviewer works on: edits made since it opened, or a
        // coverage that changed, ask again instead of approving past them.
        run: notes => {
            const coverage = incompleteApprovalCoverage();
            if (dirty || JSON.stringify(coverage) !== JSON.stringify(shownCoverage)) {
                pendingAction = null;
                approveReview();
                return false;
            }
            const verdict = {notes, expected_candidate_version: expectedCandidateVersion()};
            if (coverage) verdict.acknowledged_coverage = acknowledgedCoverage(coverage);
            return submitVerdict('approve', verdict, 'Approved');
        }
    });
}

// The selected product-schema review's coverage while it is incomplete, or null.
function incompleteApprovalCoverage() {
    if (selectedDetail?.review.artifact_type !== 'PRODUCT_SCHEMA') return null;
    const coverage = reviewPageCoverage(selectedDetail, selectedMatrix);
    return coverageComplete(coverage) ? null : coverage;
}

// Brings back what this review's approval replaced, while what it made live is still live.
function restoreApproval() {
    const what = selectedDetail?.review.artifact_type === 'URL_PATTERN'
        ? 'the URL pattern'
        : 'the schema list and removed-page templates';
    openActionPanel({
        title: 'Restore previous configuration?',
        message: `Restore ${what} this approval replaced? Pages waiting on a review are retried with it.`,
        notes: 'optional',
        notesLabel: 'Why restore? (optional)',
        confirmLabel: 'Restore previous',
        run: notes => submitVerdict('restore', {notes}, 'Previous configuration restored')
    });
}

// Lets the crawler scrape this review's seed and triggering pages again.
function revokeRejection() {
    openActionPanel({
        title: 'Revoke rejection?',
        message: 'Revoke this rejection? Its seed and triggering pages are classified again by their URL pattern and scraped again; a page no cached schema extracts opens a new review.',
        notes: 'optional',
        notesLabel: 'Why revoke? (optional)',
        confirmLabel: 'Revoke rejection',
        run: notes => submitVerdict('revoke-rejection', {notes}, 'Rejection revoked')
    });
}

function rejectReview() {
    openActionPanel({
        title: 'Reject?',
        notes: 'optional',
        confirmLabel: 'Reject',
        danger: true,
        run: notes => submitVerdict('reject', {notes}, 'Rejected')
    });
}

function needsRepair() {
    openActionPanel({
        title: 'Needs repair',
        notes: 'required',
        notesLabel: 'What needs repair?',
        confirmLabel: 'Request repair',
        run: notes => submitVerdict('needs-repair', {notes}, 'Repair requested')
    });
}

// A refused verdict leaves the review on screen as it was and shows why; only an accepted one
// reloads the review and the queue.
async function submitVerdict(verdict, body, successMessage) {
    const reviewId = selectedId;
    return submitMutation(() => api(`/api/reviews/${reviewId}/${verdict}`, {
        method: 'POST',
        body: JSON.stringify(body)
    }), successMessage, async () => {
        await loadReviews();
        await loadSelectedReview(false);
    });
}

// Review mutations take the reviewer's confirmation and notes in a panel on the page, never in
// prompt() or confirm(): browsers embedded in other apps or run headless do not implement them,
// and the panel shows which review it acts on. Cancelling it sends nothing; a refused request
// keeps it open with the notes as typed. `notes` is null, 'optional' or 'required'.
function openActionPanel(action) {
    if (pendingAction?.busy) return;
    pendingAction = {notesLabel: 'Notes (optional)', ...action, reviewId: selectedId, notesText: '', busy: false};
    updateActionPanel();
    const panel = document.getElementById('actionPanel');
    if (!panel) return;
    panel.scrollIntoView({block: 'nearest'});
    (document.getElementById('actionPanelNotes') || document.getElementById('actionPanelConfirm'))?.focus();
}

function renderActionPanel() {
    const action = pendingAction;
    if (!action || action.reviewId !== selectedId) return '';
    const busy = action.busy ? 'disabled' : '';
    const blocked = action.notes === 'required' && !action.notesText.trim() ? 'disabled' : '';
    return `<div class="card action-panel ${action.danger ? 'action-panel-danger' : ''}" role="dialog" aria-label="${escapeHtmlAttr(action.title)}" onkeydown="actionPanelKeydown(event)">
      <div class="card-body">
        <h3>${escapeHtml(action.title)}</h3>
        ${action.message ? `<p class="small">${escapeHtml(action.message)}</p>` : ''}
        ${action.notes ? `<label class="field">${escapeHtml(action.notesLabel)}
          <textarea id="actionPanelNotes" rows="3" ${busy} oninput="setActionPanelNotes(this.value)">${escapeHtml(action.notesText)}</textarea>
        </label>` : ''}
        <div class="actions">
          <button id="actionPanelConfirm" class="${action.danger ? 'danger' : 'primary'}" ${busy || blocked} onclick="confirmActionPanel()" type="button">${escapeHtml(action.busy ? 'Sending…' : action.confirmLabel)}</button>
          <button class="quiet" ${busy} onclick="cancelActionPanel()" type="button">Cancel</button>
        </div>
      </div>
    </div>`;
}

// Updates the panel alone, so unsaved edits in the workbench's inputs stay as typed.
function updateActionPanel() {
    const panel = document.getElementById('actionPanel');
    if (panel) panel.innerHTML = renderActionPanel();
    syncPageActions();
}

// While the action panel waits for its answer, its confirm button is the only action: the
// verdict buttons in the header stay visible but off, so no second primary competes with it.
function syncPageActions() {
    const waiting = Boolean(pendingAction && pendingAction.reviewId === selectedId);
    document.querySelectorAll('.page-actions button').forEach(button => {
        button.disabled = waiting;
    });
}

// Typing keeps the textarea focused, so it changes the notes without rendering the panel again.
function setActionPanelNotes(value) {
    if (!pendingAction) return;
    pendingAction.notesText = value;
    const confirmButton = document.getElementById('actionPanelConfirm');
    if (confirmButton && pendingAction.notes === 'required') confirmButton.disabled = !value.trim();
}

function actionPanelKeydown(event) {
    if (event.key === 'Escape') {
        event.preventDefault();
        cancelActionPanel();
    } else if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) {
        event.preventDefault();
        confirmActionPanel();
    }
}

function cancelActionPanel() {
    if (pendingAction?.busy) return;
    pendingAction = null;
    updateActionPanel();
}

async function confirmActionPanel() {
    const action = pendingAction;
    if (!action || action.busy || action.reviewId !== selectedId) return;
    // Notes go as before the panel: required notes trimmed, optional ones as typed or null.
    const notes = action.notes === 'required' ? action.notesText.trim() : (action.notesText || null);
    if (action.notes === 'required' && !notes) return;
    action.busy = true;
    updateActionPanel();
    let done = false;
    try {
        done = (await action.run(notes)) !== false;
    } catch (error) {
        editRefusal = {reviewId: action.reviewId, message: apiErrorMessage(error)};
        updateEditMessages();
    } finally {
        if (pendingAction === action) {
            action.busy = false;
            if (done) pendingAction = null;
            updateActionPanel();
        }
    }
}

async function saveCandidate() {
    await submitEdit(async () => {
        const payload = JSON.parse(document.getElementById('candidatePayload').value);
        await saveCandidatePayload(payload);
    });
}

// The edited payload stays the local candidate, so a refused save keeps it on screen as an
// unsaved edit.
async function saveCandidatePayload(payload) {
    selectedDetail.review.candidate_payload = payload;
    markDirty();
    return submitMutation(async () => {
        const saved = await api(`/api/reviews/${selectedId}/candidate`, {
            method: 'POST',
            body: JSON.stringify({...payload, expected_candidate_version: expectedCandidateVersion()})
        });
        recordSavedCandidateVersion(saved);
    }, 'Candidate saved', reloadSavedCandidate);
}

// A saved candidate drops the stored matrix, so the review is evaluated again on its pages.
async function reloadSavedCandidate() {
    await loadSelectedReview(false, true);
    await loadReviews();
}

async function moveCurrentSchema(delta) {
    const payload = schemasPayload();
    const schemas = payload.schemas || [];
    const nextIndex = selectedSchemaIndex + delta;
    if (nextIndex < 0 || nextIndex >= schemas.length) return;

    [schemas[selectedSchemaIndex], schemas[nextIndex]] = [schemas[nextIndex], schemas[selectedSchemaIndex]];
    payload.schemas = schemas;
    selectedSchemaIndex = nextIndex;
    if (compareSchemaIndex === nextIndex) {
        compareSchemaIndex = nextIndex - delta;
    }
    selectedDetail.review.candidate_payload = payload;
    await saveCandidatePayload(payload);
}

async function addNewSchema() {
    const payload = schemasPayload();
    payload.schemas = payload.schemas || [];
    const base = currentSchemas()[selectedSchemaIndex] || defaultSchema();
    payload.schemas.push(JSON.parse(JSON.stringify(base)));
    selectedSchemaIndex = payload.schemas.length - 1;
    compareSchemaIndex = Math.max(0, selectedSchemaIndex - 1);
    selectedDetail.review.candidate_payload = payload;
    await saveCandidatePayload(payload);
}

function deleteCurrentSchema() {
    if (currentSchemas().length <= 1) return;
    const deletedIndex = selectedSchemaIndex;
    openActionPanel({
        title: `Delete ${schemaLabel(deletedIndex)}?`,
        message: 'Saves the candidate\'s schema list without it.',
        confirmLabel: 'Delete schema',
        danger: true,
        run: () => {
            const payload = schemasPayload();
            const schemas = payload.schemas || [];
            if (schemas.length <= 1 || deletedIndex >= schemas.length) return false;
            schemas.splice(deletedIndex, 1);
            payload.schemas = schemas;
            selectedSchemaIndex = Math.min(deletedIndex, schemas.length - 1);
            compareSchemaIndex = Math.min(compareSchemaIndex, Math.max(0, schemas.length - 1));
            if (compareSchemaIndex === selectedSchemaIndex && schemas.length > 1) {
                compareSchemaIndex = selectedSchemaIndex === 0 ? 1 : 0;
            }
            selectedDetail.review.candidate_payload = payload;
            // A refused save keeps the deletion as an unsaved edit, so the panel closes either
            // way and confirming again never deletes another schema.
            return saveCandidatePayload(payload).then(() => true);
        }
    });
}

async function discardEdits() {
    editNotice = null;
    return reloadDiscardingEdits(() => loadSelectedReview(false));
}

function markDirty() {
    dirty = true;
    updateReloadState();
}

// The status is a live region that every render updates, so it changes its text only when the
// state changes: rewriting the same text would have a screen reader announce it again. It is
// empty while nothing needs saying.
function updateReloadState() {
    const state = document.getElementById('reloadState');
    const [text, className] = dirty
        ? ['Unsaved edits', 'system-status dirty']
        : selectedReviewNeedsRefresh
            ? ['Changed elsewhere — refresh', 'system-status stale']
            : transientStatus
                ? [transientStatus.text, transientStatus.className]
                : ['', 'system-status'];
    if (state.textContent !== text) state.textContent = text;
    state.className = className;
}

function toggleSidebar() {
    setSidebarCollapsed(!sidebarCollapsed);
}

function setSidebarCollapsed(collapsed) {
    sidebarCollapsed = collapsed;
    localStorage.setItem('crawlerReviewSidebarCollapsed', String(collapsed));
    const shell = document.getElementById('appShell');
    const toggle = document.getElementById('sidebarToggle');
    if (shell) shell.classList.toggle('sidebar-collapsed', collapsed);
    if (toggle) {
        toggle.setAttribute('aria-expanded', String(!collapsed));
        toggle.setAttribute('aria-label', collapsed ? 'Expand the queue' : 'Collapse the queue');
        toggle.setAttribute('title', collapsed ? 'Expand the queue' : 'Collapse the queue');
    }
}

function updateSelectedReviewFreshness(reviews) {
    if (!selectedId || !selectedDetail) {
        selectedReviewNeedsRefresh = false;
        selectedReviewRefreshReason = '';
        updateReloadState();
        return;
    }

    const current = reviews.find(review => review.review_id === selectedId);
    if (!current) {
        selectedReviewNeedsRefresh = true;
        selectedReviewRefreshReason = 'Selected review is no longer in the queue.';
    } else if (selectedReviewStatus && current.status !== selectedReviewStatus) {
        selectedReviewNeedsRefresh = true;
        selectedReviewRefreshReason = `Status changed from ${selectedReviewStatus} to ${current.status}.`;
    } else if (!matrixRefreshesInFlight.has(selectedId) && isNewerTimestamp(current.updated, selectedReviewUpdatedAt)) {
        selectedReviewNeedsRefresh = true;
        selectedReviewRefreshReason = 'Selected review was updated elsewhere.';
    } else {
        selectedReviewNeedsRefresh = false;
        selectedReviewRefreshReason = '';
    }

    updateRefreshBanner();
    updateReloadState();
}

function updateRefreshBanner() {
    const banner = document.getElementById('reviewRefreshBanner');
    if (!banner) return;
    if (!selectedReviewNeedsRefresh) {
        banner.hidden = true;
        banner.innerHTML = '';
        return;
    }
    banner.hidden = false;
    banner.innerHTML = notice('warn', 'Changed elsewhere', escapeHtml(selectedReviewRefreshReason || 'Selected review changed elsewhere.'),
        {actions: '<button onclick="refreshSelectedReview()" type="button">Refresh review</button>'});
}

function normalizeSchemaSelection() {
    const schemas = currentSchemas();
    if (!schemas.length) {
        selectedSchemaIndex = 0;
        compareSchemaIndex = 0;
        return;
    }
    selectedSchemaIndex = Math.min(selectedSchemaIndex, schemas.length - 1);
    if (isRawAttributeField(selectedField) && !schemaFieldValue(currentSchema(), selectedField)) {
        selectedField = 'title';
    }
    if (schemas.length === 1) {
        compareSchemaIndex = 0;
    } else if (compareSchemaIndex === selectedSchemaIndex || compareSchemaIndex >= schemas.length) {
        compareSchemaIndex = selectedSchemaIndex === 0 ? 1 : 0;
    }
}

function isNewerTimestamp(candidate, baseline) {
    if (!candidate || !baseline) return false;
    const candidateTime = Date.parse(candidate);
    const baselineTime = Date.parse(baseline);
    return Number.isFinite(candidateTime) && Number.isFinite(baselineTime) && candidateTime > baselineTime;
}

// The tone of each review status the console knows by its exact value. Any other text renders neutral unless its caller names a tone, so a
// negative text such as "Not approved" can never take the approved style by a substring.
const STATUS_TONES = {
    PENDING_REVIEW: 'warn',
    APPROVED: 'ok',
    REJECTED: 'bad'
};

// Status values shown in other words than their persisted name.
const STATUS_LABELS = {PENDING_REVIEW: 'Pending'};

const BADGE_TONES = new Set(['warn', 'ok', 'bad', 'accent']);

function statusBadge(status) {
    return badge(STATUS_LABELS[status] || displayName(status), STATUS_TONES[status]);
}

// A badge with the text `text` in the tone `tone`, one of `BADGE_TONES`; neutral without one.
function badge(text, tone) {
    const cls = BADGE_TONES.has(tone) ? `badge badge-${tone}` : 'badge';
    return `<span class="${cls}">${escapeHtml(text)}</span>`;
}

const DISPLAY_ACRONYMS = {url: 'URL', id: 'ID', html: 'HTML', json: 'JSON', llm: 'LLM'};

// A persisted value such as `PENDING_REVIEW` or `source_listing_id` as the console shows it: in
// sentence case, like the console's headings, with its acronyms kept ("Pending review",
// "Source listing ID", "URL pattern").
function displayName(value) {
    const text = String(value ?? '')
        .replace(/[_-]+/g, ' ')
        .replace(/\s+/g, ' ')
        .trim()
        .toLowerCase()
        .split(' ')
        .map(word => DISPLAY_ACRONYMS[word] || word)
        .join(' ');
    return text.charAt(0).toUpperCase() + text.slice(1);
}

function formatDate(value) {
    if (!value) return '';
    const date = new Date(value);
    if (Number.isNaN(date.getTime())) return String(value);
    return date.toLocaleString();
}

// The elapsed time of `value` with the full date as its tooltip; `bare` drops the "ago" for a
// sentence that says "since" itself.
function timeBadge(value, label, bare = false) {
    if (!value) return '';
    const text = bare ? elapsedTime(value).replace(/ ago$/, '') : elapsedTime(value);
    return `<span class="time" title="${escapeHtmlAttr(`${label}: ${formatDate(value)}`)}">${escapeHtml(text)}</span>`;
}

// The inline icons of the console, 16 px strokes in the current colour; the chevron points down
// and the CSS turns it while its section is closed.
function icon(paths, size = 16) {
    return `<svg viewBox="0 0 24 24" width="${size}" height="${size}" aria-hidden="true" focusable="false" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${paths}</svg>`;
}

function chevronIcon() {
    return '<svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true" focusable="false"><path d="M4 6l4 4 4-4" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>';
}

function moonIcon() {
    return icon('<path d="M21 12.8A9 9 0 1 1 11.2 3 7 7 0 0 0 21 12.8z"/>', 18);
}

function sunIcon() {
    return icon('<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M2 12h2M20 12h2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/>', 18);
}

function externalIcon() {
    return icon('<path d="M14 4h6v6M20 4l-9 9M19 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1h5"/>');
}

function eyeIcon() {
    return icon('<path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/>');
}

function closeIcon() {
    return icon('<path d="M6 6l12 12M18 6L6 18"/>');
}

// The time until a future `value`, in the units of `elapsedTime`, or "now" once it has passed.
function untilText(value) {
    const date = new Date(value);
    if (Number.isNaN(date.getTime())) return String(value);
    const seconds = Math.floor((date.getTime() - Date.now()) / 1000);
    if (seconds <= 0) return 'now';
    if (seconds < 60) return `in ${seconds}s`;
    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) return `in ${minutes}m`;
    const hours = Math.floor(minutes / 60);
    if (hours < 48) return `in ${hours}h`;
    const days = Math.floor(hours / 24);
    if (days < 60) return `in ${days}d`;
    const months = Math.floor(days / 30);
    if (months < 24) return `in ${months}mo`;
    return `in ${Math.floor(days / 365)}y`;
}

function elapsedTime(value) {
    const date = new Date(value);
    if (Number.isNaN(date.getTime())) return String(value);
    const seconds = Math.max(0, Math.floor((Date.now() - date.getTime()) / 1000));
    if (seconds < 60) return `${seconds}s ago`;
    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) return `${minutes}m ago`;
    const hours = Math.floor(minutes / 60);
    if (hours < 48) return `${hours}h ago`;
    const days = Math.floor(hours / 24);
    if (days < 60) return `${days}d ago`;
    const months = Math.floor(days / 30);
    if (months < 24) return `${months}mo ago`;
    return `${Math.floor(days / 365)}y ago`;
}

function escapeHtml(s) {
    return String(s ?? '').replace(/[&<>"']/g, ch => ({
        '&': '&amp;',
        '<': '&lt;',
        '>': '&gt;',
        '"': '&quot;',
        "'": '&#39;'
    }[ch]));
}

function escapeHtmlAttr(s) {
    return escapeHtml(s).replace(/`/g, '&#96;');
}

window.addEventListener('message', event => {
    const frame = document.getElementById('snapshotFrame');
    if (!frame || event.source !== frame.contentWindow) return;
    const data = event.data || {};
    if (data.type !== 'crawler-review-selector-picked') return;
    pickedSelector = data.selector || '';
    // The sample is what the element says, or for an image or link where it points: the selector
    // already names the tag.
    pickedSample = data.text ? `“${data.text}”` : `${data.tag || ''} ${data.src || data.href || ''}`.trim();
    const picked = document.getElementById('pickedElement');
    if (picked) picked.innerHTML = renderPickedElement(activeWorkbenchPanel === 'selector');
});

// The URL fragment names the selected review and, for a schema review, its page and schema, so
// reload, browser back and a shared link restore them: `#review=<id>&page=<id>&schema=<index>`,
// with the 0-based schema index that `schemaLabel` shows from 1. `#sources` and `#reviews` open
// the console's two views without a review, and no fragment opens Sources.
// A link is untrusted and the review id becomes part of API paths, so only an id of the review
// id's characters is taken; anything else, such as `/` or `?`, selects no review.
const REVIEW_ID_PATTERN = /^[A-Za-z0-9_-]+$/;

function selectionFromUrl() {
    const hash = location.hash.slice(1);
    const params = new URLSearchParams(hash);
    const review = params.get('review');
    const schema = params.get('schema');
    const reviewId = REVIEW_ID_PATTERN.test(review || '') ? review : null;
    return {
        view: reviewId || hash === 'reviews' ? 'reviews' : 'sources',
        reviewId,
        pageId: params.get('page') || null,
        schemaIndex: /^\d+$/.test(schema || '') ? Number(schema) : null
    };
}

function selectionUrlHash() {
    if (!selectedId) return currentView === 'reviews' ? '#reviews' : '#sources';
    const params = new URLSearchParams({review: selectedId});
    if (selectedMatrix && selectedDetail?.review.review_id === selectedId) {
        if (selectedPageId) params.set('page', selectedPageId);
        params.set('schema', String(selectedSchemaIndex));
    }
    return `#${params}`;
}

// Selecting another review or view adds a history entry, so browser back returns to the previous
// one; a page or schema change within the review only updates the current entry.
function recordSelectionInUrl() {
    if (selectionUrlHash() === location.hash) return;
    const shown = selectionFromUrl();
    if (shown.reviewId === selectedId && shown.view === currentView) {
        replaceSelectionInUrl();
    } else {
        history.pushState(null, '', selectionUrl());
    }
}

function replaceSelectionInUrl() {
    history.replaceState(null, '', selectionUrl());
}

function selectionUrl() {
    return selectionUrlHash() || `${location.pathname}${location.search}`;
}

function applySelectionFromUrl() {
    const requested = selectionFromUrl();
    if (requested.reviewId === selectedId && requested.view === currentView) {
        if (selectedId) applyRequestedPageAndSchema(requested);
        return;
    }
    const follow = () => {
        setView(requested.view);
        return requested.reviewId
            ? selectReview(requested.reviewId, requested)
            : clearSelectedReview();
    };
    if (!dirty) return follow();
    const requestedUrl = location.href;
    replaceSelectionInUrl();
    confirmDiscardingEdits(requested.reviewId ? 'Opening another review' : 'Leaving the review', () => {
        history.replaceState(null, '', requestedUrl);
        return follow();
    });
}

// A review still loading takes the request when it has loaded.
function applyRequestedPageAndSchema({pageId, schemaIndex}) {
    if (selectedDetail?.review.review_id !== selectedId) {
        requestedSelection = {pageId, schemaIndex};
        return;
    }
    if (!selectedMatrix) return;
    const page = (selectedDetail.pages || []).find(candidate => candidate.review_page_id === pageId);
    if (page && page.review_page_id !== selectedPageId) {
        selectedPageId = page.review_page_id;
        previewUrlOverride = '';
    }
    if (schemaIndex !== null) selectedSchemaIndex = schemaIndex;
    normalizeSchemaSelection();
    rerenderWorkbench();
}

function clearSelectedReview() {
    selectedId = null;
    selectedDetail = null;
    selectedMatrix = null;
    dirty = false;
    editNotice = null;
    pendingAction = null;
    selectedReviewNeedsRefresh = false;
    selectedReviewRefreshReason = '';
    shownDetailHtml = '';
    setView(currentView);
    showIdleDetail();
    updateReloadState();
    return loadReviews().catch(reportQueueFailure);
}

window.addEventListener('hashchange', applySelectionFromUrl);

// The queue refreshes calmly and not at all while the tab is hidden; it refreshes at once when
// the tab is shown again. A refresh only updates the queue, never the selected review.
const QUEUE_REFRESH_INTERVAL_MS = 15000;
let queueRefreshTimer = null;

async function refreshQueue() {
    clearTimeout(queueRefreshTimer);
    queueRefreshTimer = null;
    try {
        await loadReviews();
    } catch (err) {
        reportQueueFailure(err);
    }
    scheduleQueueRefresh();
}

function scheduleQueueRefresh() {
    clearTimeout(queueRefreshTimer);
    queueRefreshTimer = document.hidden ? null : setTimeout(refreshQueue, QUEUE_REFRESH_INTERVAL_MS);
}

document.addEventListener('visibilitychange', () => {
    if (document.hidden) {
        clearTimeout(queueRefreshTimer);
        queueRefreshTimer = null;
    } else {
        refreshQueue();
    }
});

scheduleQueueRefresh();

setSidebarCollapsed(sidebarCollapsed);
updateSignOutButton();
setView(selectionFromUrl().view);
showIdleDetail();
loadReviews().catch(reportQueueFailure);
if (selectionFromUrl().reviewId) applySelectionFromUrl();
