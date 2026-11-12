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
let theme = document.documentElement.dataset.theme || localStorage.getItem('crawlerReviewTheme') || 'light';
// A status the header shows until it is replaced: `Signed out`, or why the refresh failed.
let transientStatus = null;
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

// ───────────────────────────── Routes ─────────────────────────────
// The URL fragment is the state of the console: its first key names the route and the others are
// its parameters (`#sources&status=needs_attention`, `#source=<id>`, `#review=<id>&page=&schema=`).
// A link is untrusted and ids become part of API paths, so only an id of the id characters is taken.
const REVIEW_ID_PATTERN = /^[A-Za-z0-9_-]+$/;
const ROUTE_NAMES = ['overview', 'sources', 'source', 'reviews', 'review'];
const PAGE_SIZE = 100;
// The header view each route belongs to.
const NAV_OF_ROUTE = {overview: 'overview', sources: 'sources', source: 'sources', reviews: 'reviews', review: 'reviews'};

let route = {name: 'overview', params: new URLSearchParams()};
// The last Reviews list the reviewer used, so a review's breadcrumb returns to it with its filters.
let lastReviewsHash = '#reviews';

function parseRoute() {
    const params = new URLSearchParams(location.hash.slice(1));
    const first = params.keys().next().value || 'overview';
    const name = ROUTE_NAMES.includes(first) ? first : 'overview';
    if ((name === 'review' || name === 'source') && !REVIEW_ID_PATTERN.test(params.get(name) || '')) {
        return {name: 'overview', params: new URLSearchParams()};
    }
    return {name, params};
}

// `#<name>[=<id>]&<other parameters>` for a route, without empty parameters.
function routeHash(name, params, id = '') {
    const rest = new URLSearchParams();
    for (const [key, value] of params) {
        if (key !== name && value !== '') rest.append(key, value);
    }
    const text = rest.toString();
    return `#${id ? `${name}=${encodeURIComponent(id)}` : name}${text ? `&${text}` : ''}`;
}

// A review's fragment. `queue` ({key, index}) says which list it was opened from and at which
// place, so the review can offer the next one; `key` is that list's fragment without its offset.
function reviewHash(reviewId, queue = null) {
    const params = new URLSearchParams({review: reviewId});
    if (queue) {
        params.set('queue', queue.key);
        params.set('i', String(queue.index));
    }
    return `#${params}`;
}

function sourceHash(listingSourceId) {
    return `#source=${encodeURIComponent(listingSourceId)}`;
}

// The fragment of the list a review was opened from, without its offset: the queue it belongs to.
function queueKeyOfList() {
    const params = new URLSearchParams(route.params);
    params.delete('offset');
    return routeHash('reviews', params).slice(1);
}

function listRouteKind() {
    if (route.name === 'reviews') return 'reviews';
    return route.name === 'sources' || route.name === 'overview' ? 'sources' : null;
}

// ───────────────────────────── Shared pieces ─────────────────────────────
// One tone per meaning, for every badge, tile and banner of the console.
const TONES = {
    PENDING_REVIEW: 'warn', APPROVED: 'ok', REJECTED: 'bad', SUPERSEDED: 'neutral',
    human_required: 'bad', waiting_for_repair: 'warn', live: 'ok', rejection_revoked: 'neutral', source_disabled: 'neutral',
    rejected_by_model: 'bad',
    crawl_needs_operator: 'bad', crawl_failing: 'bad', failing_to_scrape: 'bad', blocked_by_review: 'warn', removals_held: 'warn',
    healthy: 'ok', not_scraped_yet: 'neutral', crawling_disabled: 'neutral', labelled: 'neutral',
    crawling: 'info', scraping: 'info',
    HUMAN: 'info', AGENT_AUTHOR: 'neutral', AGENT_REVIEWER: 'neutral', SYSTEM: 'neutral',
    applied: 'ok', failed_check: 'bad', did_not_apply: 'bad', not_evaluated: 'warn', changed: 'info'
};

const STATUS_LABELS = {PENDING_REVIEW: 'Pending'};

function statusBadge(status) {
    return badge(STATUS_LABELS[status] || displayName(status), TONES[status]);
}

function formatCount(value) {
    return (Number(value) || 0).toLocaleString();
}

function pluralize(count, one, many) {
    return `${formatCount(count)} ${count === 1 ? one : many}`;
}

function listingSourceName(source) {
    return String(source.listing_source_name || 'Unnamed source');
}

function sourceHost(source) {
    try {
        return new URL(source.crawl_root_url).hostname.replace(/^www\./, '');
    } catch (_) {
        return 'No crawl root';
    }
}

function artifactLabel(type) {
    return displayName(type);
}

// A persisted reason as a lower-case phrase that keeps its acronyms: "manual URL pattern required".
function reasonWord(reason) {
    const text = displayName(reason);
    return text.charAt(0).toLowerCase() + text.slice(1);
}

// The flags that change what to do with a review, after its status badge.
function reviewFlags(review, {live = false, revoked = false} = {}) {
    const flags = [];
    const pending = review.status === 'PENDING_REVIEW';
    if (pending && review.human_required) {
        flags.push(badge('Needs a human', TONES.human_required));
    } else if (pending && review.repair_request) {
        flags.push(badge('Waiting for repair', TONES.waiting_for_repair));
    }
    const evaluation = review.auto_schema_evaluation;
    if (pending && evaluation?.decision === 'REJECT' && !evaluation.approved_by_llm) {
        flags.push(badge('Rejected by the model', TONES.rejected_by_model));
    }
    if (pending && review.listing_source_crawl_enabled === false) flags.push(badge('Source disabled', TONES.source_disabled));
    if (live) flags.push(badge('Live', TONES.live));
    if (revoked) flags.push(badge('Revoked', TONES.rejection_revoked));
    return flags.join(' ');
}

function stateBlock(kind, title, text = '', actions = '') {
    return `<div class="state state-${kind}"><div class="state-title">${escapeHtml(title)}</div>${
        text ? `<div class="state-text">${escapeHtml(text)}</div>` : ''}${
        actions ? `<div class="state-actions">${actions}</div>` : ''}</div>`;
}

function searchIcon() {
    return icon('<circle cx="11" cy="11" r="7"/><path d="m20 20-3.5-3.5"/>', 14);
}

function sortIcon() {
    return '<svg class="sort-icon" viewBox="0 0 16 16" width="12" height="12" aria-hidden="true" focusable="false" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M8 3v10M4 7l4-4 4 4"/></svg>';
}

// A render that changes nothing leaves the view alone, and one that does keeps the keyboard focus
// on the control of the new render that opens or toggles the same thing.
function replaceKeepingFocus(container, html) {
    const focused = container.contains(document.activeElement) ? controlKey(document.activeElement) : null;
    container.innerHTML = html;
    if (focused) {
        Array.from(container.querySelectorAll('a, button'))
            .find(control => controlKey(control) === focused)
            ?.focus({preventScroll: true});
    }
}

function controlKey(control) {
    return ['href', 'aria-controls', 'onclick']
        .map(name => control.getAttribute(name) || '')
        .join(' ');
}

function showPage(html) {
    const detail = document.getElementById('detail');
    detail.dataset.route = route.name;
    replaceKeepingFocus(detail, html);
}

// ───────────────────────────── Refresh state ─────────────────────────────
// What the header says about the console's own refresh: when it last succeeded, that it pauses
// while the tab is hidden, and what failed. The failures are per read, so one that recovers does
// not hide another that has not.
const refreshFailures = new Map();
let lastRefreshAt = null;

function noteRefresh(read, failure = null) {
    if (failure) {
        refreshFailures.set(read, failure);
    } else {
        refreshFailures.delete(read);
        lastRefreshAt = Date.now();
    }
    updateRefreshState();
}

function clockText(time) {
    return new Date(time).toLocaleTimeString([], {hour: '2-digit', minute: '2-digit', hour12: false});
}

function updateRefreshState() {
    const element = document.getElementById('refreshState');
    if (!element) return;
    let html;
    let paused = false;
    if (document.hidden) {
        html = 'paused — tab hidden';
        paused = true;
    } else if (refreshFailures.size) {
        html = `⚠ not refreshed${lastRefreshAt ? ` since ${clockText(lastRefreshAt)}` : ''} (${escapeHtml([...refreshFailures.values()][0])})`;
        paused = true;
    } else if (lastRefreshAt) {
        const seconds = Math.floor((Date.now() - lastRefreshAt) / 1000);
        html = `<span class="dot dot-info dot-live"></span>refreshed ${seconds < 45 ? 'just now' : `${elapsedTime(lastRefreshAt)}`}`;
    } else {
        html = '';
    }
    const title = `Lists and counts refresh every 15 s while this tab is visible${lastRefreshAt ? `; last refresh ${new Date(lastRefreshAt).toLocaleTimeString()}` : ''}`;
    if (element.innerHTML !== html) element.innerHTML = html;
    element.classList.toggle('paused', paused);
    element.title = title;
    document.getElementById('detail')?.toggleAttribute('data-stale', refreshFailures.size > 0);
}

let tickCount = 0;
setInterval(() => {
    updateRefreshState();
    tickCount += 1;
    if (tickCount % 6 === 0) retickTimes();
}, 5000);

// ───────────────────────────── Header ─────────────────────────────
// `undefined` until the first answer, `null` when the counts could not be read.
let overviewCounts;
let overviewFailure = '';

async function loadOverview() {
    try {
        overviewCounts = await api('/api/overview');
        overviewFailure = '';
        noteRefresh('overview');
    } catch (error) {
        overviewFailure = apiErrorMessage(error);
        // The counts shown stay, dimmed, until a read answers; the tiles say so only before one has.
        if (overviewCounts === undefined) overviewCounts = null;
        noteRefresh('overview', overviewFailure);
    }
    updateNav();
    if (route.name === 'overview') renderOverviewTiles();
}

function pendingReviewCount() {
    const reviews = overviewCounts?.reviews;
    return reviews ? Math.max(0, reviews.pending_total - reviews.pending_disabled_sources) : 0;
}

// The header's three views, with the one number an operator wants from the other two: the sources
// that need them and the reviews waiting for a verdict. Both are omitted at 0.
function updateNav() {
    const active = NAV_OF_ROUTE[route.name];
    const needing = overviewCounts?.sources.needs_attention || 0;
    const pending = pendingReviewCount();
    const links = [
        ['navOverview', 'overview', 'Overview', ''],
        ['navSources', 'sources', 'Sources', needing
            ? `<span class="badge badge-bad badge-count" title="${escapeHtmlAttr(`${pluralize(needing, 'source needs', 'sources need')} you`)}">${formatCount(needing)}</span>` : ''],
        ['navReviews', 'reviews', 'Reviews', pending
            ? `<span class="badge badge-warn badge-count" title="${escapeHtmlAttr(`${pluralize(pending, 'pending review', 'pending reviews')} of crawl-enabled sources`)}">${formatCount(pending)}</span>` : '']
    ];
    for (const [id, name, label, count] of links) {
        const link = document.getElementById(id);
        if (!link) continue;
        if (active === name) link.setAttribute('aria-current', 'page');
        else link.removeAttribute('aria-current');
        const html = `${label}${count ? ` ${count}` : ''}`;
        if (link.innerHTML !== html) link.innerHTML = html;
    }
}

// ───────────────────────────── Overview tiles ─────────────────────────────
function renderOverviewTiles() {
    const tiles = document.getElementById('overviewTiles');
    if (tiles) tiles.innerHTML = overviewTilesHtml();
}

// The counts at the reviewer's previous visit, kept in this browser, so a tile can say how it
// moved since they last looked. A visit ends when the tab is hidden; coming back after ten minutes
// starts a new one.
const VISIT_KEY = 'crawlerReviewVisit';
const NEW_VISIT_AFTER_MS = 10 * 60 * 1000;
let visitBaseline = readVisit();
let hiddenAt = null;

function readVisit() {
    try {
        return JSON.parse(localStorage.getItem(VISIT_KEY) || 'null');
    } catch (_) {
        return null;
    }
}

function countsSnapshot() {
    if (!overviewCounts) return null;
    return {
        at: Date.now(),
        needs_attention: overviewCounts.sources.needs_attention,
        pending: pendingReviewCount(),
        waiting_for_human: overviewCounts.reviews.waiting_for_human
    };
}

function storeVisit() {
    const snapshot = countsSnapshot();
    if (!snapshot) return;
    try {
        localStorage.setItem(VISIT_KEY, JSON.stringify(snapshot));
    } catch (_) {
        // Nothing to keep.
    }
}

document.addEventListener('visibilitychange', () => {
    if (document.hidden) {
        hiddenAt = Date.now();
        storeVisit();
    } else if (hiddenAt && Date.now() - hiddenAt > NEW_VISIT_AFTER_MS) {
        visitBaseline = readVisit();
    }
    updateRefreshState();
});
window.addEventListener('pagehide', storeVisit);

// How `count` moved since the previous visit, as a small mark on the tile.
function tileDelta(count, key) {
    if (!visitBaseline || typeof visitBaseline[key] !== 'number' || count === undefined) return '';
    const delta = count - visitBaseline[key];
    if (!delta) return '';
    const title = `${delta > 0 ? '+' : '−'}${formatCount(Math.abs(delta))} since you last looked at ${clockText(visitBaseline.at)}`;
    return `<span class="tile-delta${delta < 0 ? ' down' : ''}" title="${escapeHtmlAttr(title)}">${delta > 0 ? '+' : '−'}${formatCount(Math.abs(delta))}</span>`;
}

function overviewTilesHtml() {
    const failure = overviewCounts === null
        ? `<div class="banner banner-bad" role="alert"><span class="banner-kind">Counts unavailable</span><div class="banner-body">${escapeHtml(overviewFailure)} · retried every 15 s</div></div>`
        : '';
    const sources = overviewCounts?.sources;
    const reviews = overviewCounts?.reviews;
    const value = (count, deltaKey) => {
        if (!sources) return '<span class="tile-value">—</span>';
        return `<span class="tile-value${count === 0 ? ' zero' : ''}">${formatCount(count)}${deltaKey ? tileDelta(count, deltaKey) : ''}</span>`;
    };
    const label = (tone, text, live) => `<span class="tile-label"><span class="dot dot-${tone}${live ? ' dot-live' : ''}"></span>${escapeHtml(text)}</span>`;
    const tile = (tone, href, count, text, {sub = '', live = false, delta = ''} = {}) =>
        `<a class="tile tile-${tone}" href="${escapeHtmlAttr(href)}">${value(count, delta)}${label(tone, text, live)}${sub ? `<span class="tile-sub">${escapeHtml(sub)}</span>` : ''}</a>`;
    // The breakdown of the sources that need a human is a link per condition, which a tile that is
    // itself a link cannot hold, so this tile's number and label link and its breakdown does apart.
    const breakdown = sources ? [
        ['crawl_failing', `${sources.crawl_failing} crawl failing`], ['crawl_needs_operator', `${sources.crawl_needs_operator} crawl stuck`], ['blocked', `${sources.blocked_by_review} blocked`],
        ['failing', `${sources.failing_to_scrape} failing`], ['held', `${sources.held_removals} held`]
    ].map(([status, text]) => `<a href="#sources&status=${status}">${escapeHtml(text)}</a>`).join(' · ') : '';
    const needing = `<div class="tile tile-bad"><a class="tile-link" href="#sources&status=needs_attention">${value(sources?.needs_attention, 'needs_attention')}${label('bad', 'Sources need attention')}</a>${breakdown ? `<span class="tile-sub">${breakdown}</span>` : ''}</div>`;
    const byType = reviews?.pending_by_artifact;
    const pending = pendingReviewCount();
    const disabled = reviews?.pending_disabled_sources || 0;
    const pendingSub = !byType ? '' : disabled
        ? `${formatCount(reviews.pending_total)} in all · ${formatCount(disabled)} of disabled sources`
        : `${formatCount(byType.PRODUCT_SCHEMA)} product schemas · ${formatCount(byType.URL_PATTERN)} URL patterns`;
    return `${failure}<div class="tiles" role="group" aria-label="Right now">
      ${tile('info', '#sources&activity=crawling', sources?.crawling_now, 'Crawling now', {live: true})}
      ${tile('info', '#sources&activity=scraping', sources?.scraping_now, 'Scraping now', {live: true})}
      ${needing}
      ${tile('warn', '#reviews&status=PENDING_REVIEW', reviews ? pending : undefined, 'Pending reviews', {sub: pendingSub, delta: 'pending'})}
      ${overviewCounts?.agent_mode ? tile('bad', '#reviews&waiting=human', reviews.waiting_for_human, 'Waiting for a human', {sub: 'the agents stopped on these', delta: 'waiting_for_human'}) : ''}
    </div>`;
}

// ───────────────────────────── Recent activity ─────────────────────────────
// The Overview's feed of what happened in the last day: the decisions and hand-overs of the review
// trail and what the crawler did to sources, newest first, those since the reviewer's previous
// visit marked. Each of its two reads fails on its own.
const FEED_WINDOW_MS = 24 * 60 * 60 * 1000;
const FEED_LIMIT = 100;
const FEED_SHOWN = 40;
let activity = {items: undefined, failures: []};

const REVIEW_FEED_ACTIONS = {
    APPROVED: ['approved', 'ok'], REJECTED: ['rejected', 'bad'], REPAIR_REQUESTED: ['asked for a repair of', 'warn'],
    HANDED_TO_HUMAN: ['handed to a human', 'bad'], RESTORED: ['restored what an approval replaced on', 'neutral'],
    REJECTION_REVOKED: ['revoked the rejection of', 'neutral']
};

async function loadActivity() {
    const since = encodeURIComponent(new Date(Date.now() - FEED_WINDOW_MS).toISOString());
    const [reviewEvents, sourceEvents] = await Promise.allSettled([
        api(`/api/review-events?since=${since}&dir=desc&limit=${FEED_LIMIT}`),
        api(`/api/source-events?since=${since}&limit=${FEED_LIMIT}`)
    ]);
    const failures = [reviewEvents, sourceEvents].filter(result => result.status === 'rejected')
        .map(result => apiErrorMessage(result.reason));
    const items = [
        ...(reviewEvents.status === 'fulfilled' ? reviewEvents.value.events : []).filter(event => REVIEW_FEED_ACTIONS[event.action]).map(event => ({when: event.created, review: event})),
        ...(sourceEvents.status === 'fulfilled' ? sourceEvents.value.events : []).map(event => ({when: event.occurred, source: event}))
    ].sort((a, b) => Date.parse(b.when) - Date.parse(a.when));
    activity = {items, failures};
    noteRefresh('activity', failures.length === 2 ? failures[0] : null);
    renderActivity();
}

function renderActivity() {
    const feed = document.getElementById('activityFeed');
    if (feed) feed.innerHTML = activityHtml();
}

function activityHtml() {
    const head = '<div class="card-head"><h2>Recent activity</h2><span class="small muted">agents, humans and the crawler · last 24 h</span></div>';
    if (activity.items === undefined) return `${head}${stateBlock('loading', 'Loading')}`;
    const problems = activity.failures.map(message => `<div class="banner banner-bad" role="alert"><span class="banner-kind">Not read</span><div class="banner-body">${escapeHtml(message)}</div></div>`).join('');
    if (!activity.items.length) {
        return `${head}${problems}${activity.failures.length ? '' : stateBlock('empty', 'Nothing happened in the last day')}`;
    }
    const since = visitBaseline?.at || null;
    const shown = activity.items.slice(0, FEED_SHOWN);
    const fresh = shown.filter(item => since && Date.parse(item.when) > since);
    const rows = (items, isNew) => items.map(item => renderFeedItem(item, isNew)).join('');
    const earlier = shown.filter(item => !fresh.includes(item));
    return `${head}${problems}<div class="feed">${fresh.length ? `<div class="feed-day">Since you last looked · ${clockText(since)}</div>${rows(fresh, true)}` : ''}${earlier.length ? `<div class="feed-day">${fresh.length ? 'Earlier' : 'Last 24 hours'}</div>${rows(earlier, false)}` : ''}</div>`;
}

function feedLink(href, text) {
    return `<a href="${escapeHtmlAttr(href)}">${escapeHtml(text || 'Unnamed source')}</a>`;
}

function renderFeedItem(item, isNew) {
    const time = `<span class="feed-time" title="${escapeHtmlAttr(formatDate(item.when))}">${clockText(item.when)}</span>`;
    return `<div class="feed-item${isNew ? ' new' : ''}">${time}<span class="feed-text">${item.review ? reviewFeedText(item.review) : sourceFeedText(item.source)}</span></div>`;
}

function reviewFeedText(event) {
    const [verb, tone] = REVIEW_FEED_ACTIONS[event.action];
    const actor = {HUMAN: 'A human', AGENT_AUTHOR: 'The author agent', AGENT_REVIEWER: 'The reviewer agent', SYSTEM: 'The crawler'}[event.actor] || 'Someone';
    const target = `the ${artifactLabel(event.artifact_type).toLowerCase().replace('url', 'URL')} of ${feedLink(reviewHash(event.review_id), event.listing_source_name)}`;
    const note = event.notes ? ` · “${escapeHtml(event.notes.length > 90 ? `${event.notes.slice(0, 90)}…` : event.notes)}”` : '';
    const handed = event.action === 'HANDED_TO_HUMAN';
    return `${escapeHtml(actor)} <span class="badge badge-${tone}">${escapeHtml(handed ? 'handed to a human' : verb)}</span> ${handed ? feedLink(reviewHash(event.review_id), event.listing_source_name) : target}${note}`;
}

function sourceFeedText(event) {
    const link = feedLink(sourceHash(event.listing_source_id), event.listing_source_name);
    if (event.kind === 'SCRAPE_FAILING') {
        return `${link} <span class="badge badge-bad">failing to scrape</span> · ${escapeHtml(describeFailure(event.error_kind).label)}, ${formatCount(event.count)} in a row`;
    }
    const failed = event.outcome === 'FAILED' || event.outcome === 'NEEDS_OPERATOR';
    const outcome = reasonWord(event.outcome);
    return `Crawl of ${link} ${failed ? `<span class="badge badge-bad">${escapeHtml(outcome)}</span>` : `finished · ${escapeHtml(outcome)}`}${event.error_kind ? ` · ${escapeHtml(describeFailure(event.error_kind).label)}` : ''}`;
}

// ───────────────────────────── Lists ─────────────────────────────
// The two paginated tables, ListingSources (Overview and Sources) and reviews, each with the
// rows of the window shown, read from the server with the filters, order and offset of the
// fragment.
const lists = {
    sources: {items: undefined, total: 0, failure: '', sequence: 0, loading: false, unfilteredTotal: null},
    reviews: {items: undefined, total: 0, failure: '', sequence: 0, loading: false, unfilteredTotal: null}
};

const SOURCE_STATUS_CHIPS = [['', 'All'], ['needs_attention', 'Needs attention'], ['healthy', 'Healthy'], ['disabled', 'Disabled'], ['labelled', 'Labelled']];
const REVIEW_STATUS_CHIPS = [['', 'All'], ['PENDING_REVIEW', 'Pending'], ['APPROVED', 'Approved'], ['REJECTED', 'Rejected'], ['SUPERSEDED', 'Superseded']];
const REVIEW_TYPE_CHIPS = [['', 'Both types'], ['PRODUCT_SCHEMA', 'Product schema'], ['URL_PATTERN', 'URL pattern']];
// The sort keys of the tables' columns, and whether a key sorts ascending first.
const SOURCE_SORTS = {attention: 'desc', name: 'asc', pending_reviews: 'desc', last_successful_scrape: 'desc', next_crawl_at: 'desc'};
const REVIEW_SORTS = {queue: 'asc', created: 'desc', updated: 'desc', source: 'asc'};

function listParameter(name) {
    return route.params.get(name) || '';
}

// The filters and order of a list as its API route takes them, from the fragment's `params`.
function listApiQuery(kind, params) {
    const query = new URLSearchParams();
    const copy = name => {
        if (params.get(name)) query.set(name, params.get(name));
    };
    ['q', 'status', 'sort', 'dir'].forEach(copy);
    if (kind === 'sources') {
        copy('activity');
    } else {
        copy('artifact');
        copy('waiting');
        const source = params.get('listing_source') || '';
        if (REVIEW_ID_PATTERN.test(source)) query.set('listing_source', source);
    }
    return query;
}

// The query a list's API route is asked for the page the fragment names.
function listApiPath(kind) {
    const query = listApiQuery(kind, route.params);
    const offset = Number(listParameter('offset'));
    query.set('limit', String(PAGE_SIZE));
    if (Number.isInteger(offset) && offset > 0) query.set('offset', String(offset));
    return `/api/${kind === 'sources' ? 'listing_sources' : 'reviews'}?${query}`;
}

function listFiltersActive(kind) {
    const names = kind === 'sources' ? ['q', 'status', 'activity'] : ['q', 'status', 'artifact', 'waiting', 'listing_source'];
    return names.some(name => listParameter(name));
}

async function loadList() {
    const kind = listRouteKind();
    if (!kind) return;
    const list = lists[kind];
    const sequence = ++list.sequence;
    list.loading = true;
    renderListBody(kind);
    try {
        const answer = await api(listApiPath(kind));
        if (sequence !== list.sequence) return;
        list.changes = listChanges(list, answer.items);
        list.items = answer.items;
        list.total = answer.total;
        list.failure = '';
        if (!listFiltersActive(kind)) list.unfilteredTotal = answer.total;
        noteRefresh(kind);
    } catch (error) {
        if (sequence !== list.sequence) return;
        list.failure = apiErrorMessage(error);
        noteRefresh(kind, list.failure);
    }
    list.loading = false;
    renderListBody(kind);
}

// What a refresh changed in the rows of the list the reviewer has open: how many rows left it and
// which are new, said for one cycle, so a list that changes under the reviewer says so.
function listChanges(list, items) {
    const key = routeHash(route.name, route.params);
    let changes = null;
    if (list.shownKey === key && list.items) {
        const before = new Set(list.items.map(rowKey));
        const now = new Set(items.map(rowKey));
        const left = [...before].filter(id => !now.has(id)).length;
        const added = new Set(items.map(rowKey).filter(id => !before.has(id)));
        if (left || added.size) changes = {left, added};
    }
    list.shownKey = key;
    return changes;
}

// Where the reviewer was in a list when they opened a row, and which row it was, so coming back
// finds the list at the same place with that row marked.
let listReturn = null;
let leftList = null;

function rememberListReturn(next) {
    if (!listRouteKind() || next.name === 'overview' || next.name === 'sources' || next.name === 'reviews') return;
    leftList = {
        hash: routeHash(route.name, route.params),
        scrollY: window.scrollY,
        id: next.name === 'review' ? next.params.get('review') : next.params.get('source')
    };
}

function listReturnId() {
    return listReturn?.id || null;
}

// The list's rows are on screen: a list the reviewer returned to scrolls back, once.
function restoreListScroll() {
    if (!listReturn || listReturn.restored) return;
    listReturn.restored = true;
    window.scrollTo(0, listReturn.scrollY);
}

function rowKey(item) {
    return item.review_id || item.listing_source_id;
}

function renderListPage() {
    const kind = listRouteKind();
    const table = `<div class="card table-card">
        <div class="filters" id="listFilters" role="search">${renderListFilters(kind)}<div class="pager-buttons" id="listPagerTop" hidden></div></div>
        <div id="listBanner"></div>
        <table class="data-table" id="listTable"></table>
        <div class="pager" id="listPager" hidden></div>
      </div>`;
    showPage(route.name === 'overview'
        ? `<div id="overviewTiles">${overviewTilesHtml()}</div><div class="overview-grid">${table}<div class="card" id="activityFeed">${activityHtml()}</div></div>`
        : table);
    renderListBody(kind);
}

// Changes the fragment's parameters in place: a filter, sort or page change replaces the history
// entry. Every change but the page itself starts again from the first page.
function setListParams(changes, {filters = true} = {}) {
    const params = new URLSearchParams(route.params);
    for (const [name, value] of Object.entries(changes)) {
        if (value === null || value === '') params.delete(name);
        else params.set(name, String(value));
    }
    if (!('offset' in changes)) params.delete('offset');
    route = {name: route.name, params};
    history.replaceState(null, '', routeHash(route.name, route.params));
    if (route.name === 'reviews') lastReviewsHash = location.hash;
    const container = document.getElementById('listFilters');
    if (container && filters) {
        container.innerHTML = `${renderListFilters(listRouteKind())}<div class="pager-buttons" id="listPagerTop" hidden></div>`;
    }
    container?.querySelector('.reset')?.toggleAttribute('hidden', !listFiltersActive(listRouteKind()));
    loadList();
}

let searchTimer = null;

function searchList(value) {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => setListParams({q: value.trim()}, {filters: false}), 300);
}

function renderChips(label, options, current, name) {
    return `<div class="chips" aria-label="${escapeHtmlAttr(label)}">${options.map(([value, text]) =>
        `<button class="chip" aria-pressed="${value === current}" onclick="setListParams({${name}: '${value}'})" type="button">${escapeHtml(text)}</button>`).join('')}</div>`;
}

function renderSelect(label, name, options, current) {
    return `<select class="sm" aria-label="${escapeHtmlAttr(label)}" onchange="setListParams({${name}: this.value})">${options.map(([value, text]) =>
        `<option value="${escapeHtmlAttr(value)}"${value === current ? ' selected' : ''}>${escapeHtml(text)}</option>`).join('')}</select>`;
}

function renderListFilters(kind) {
    const active = listFiltersActive(kind);
    const search = `<label class="search"><span class="visually-hidden">${kind === 'sources' ? 'Search sources' : 'Search reviews'}</span>${searchIcon()}<input type="search" value="${escapeHtmlAttr(listParameter('q'))}" placeholder="${kind === 'sources' ? 'Search sources' : 'Search by source'}" oninput="searchList(this.value)"></label>`;
    const reset = `<a class="reset btn-link small" href="#${route.name}"${active ? '' : ' hidden'}>Reset</a>`;
    if (kind === 'sources') {
        return `${search}
          ${renderChips('Status', SOURCE_STATUS_CHIPS, listParameter('status'), 'status')}
          ${renderSelect('Activity', 'activity', [['', 'Any activity'], ['crawling', 'Crawling now'], ['scraping', 'Scraping now'], ['idle', 'Idle']], listParameter('activity'))}
          ${reset}`;
    }
    const sourceId = listParameter('listing_source');
    const sourceChip = REVIEW_ID_PATTERN.test(sourceId)
        ? `<button class="chip badge-kind" onclick="setListParams({listing_source: ''})" title="Show the reviews of every source" type="button">Source: ${escapeHtml(reviewSourceName(sourceId))} ${closeIcon()}</button>`
        : '';
    return `${sourceChip}${search}
      ${renderChips('Status', REVIEW_STATUS_CHIPS, listParameter('status'), 'status')}
      ${renderChips('Type', REVIEW_TYPE_CHIPS, listParameter('artifact'), 'artifact')}
      ${renderSelect('Waiting', 'waiting', [['', 'Any'], ['human', 'Waiting for a human'], ['repair', 'Waiting for repair']], listParameter('waiting'))}
      ${reset}`;
}

// The name of the ListingSource a review list is filtered to, from the rows shown or else its
// page, which is read once.
const sourceNames = new Map();

function reviewSourceName(listingSourceId) {
    const row = (lists.reviews.items || []).find(review => review.listing_source_id === listingSourceId);
    if (row?.listing_source_name) sourceNames.set(listingSourceId, row.listing_source_name);
    if (!sourceNames.has(listingSourceId)) {
        sourceNames.set(listingSourceId, 'this source');
        api(`/api/listing-sources/${encodeURIComponent(listingSourceId)}`).then(detail => {
            sourceNames.set(listingSourceId, listingSourceName(detail));
            const container = document.getElementById('listFilters');
            if (container && route.name === 'reviews') {
                container.innerHTML = `${renderListFilters('reviews')}<div class="pager-buttons" id="listPagerTop" hidden></div>`;
                renderListBody('reviews');
            }
        }).catch(() => {});
    }
    return sourceNames.get(listingSourceId);
}

function renderListBody(kind) {
    const table = document.getElementById('listTable');
    if (!table || listRouteKind() !== kind) return;
    const list = lists[kind];
    const columns = kind === 'sources' ? sourceColumns() : reviewColumns();
    const head = `<thead><tr>${columns.map(renderColumnHead).join('')}</tr></thead>`;
    const span = columns.length;
    let rows;
    if (list.items === undefined && !list.failure) {
        rows = `<tr><td colspan="${span}" class="state-cell">${stateBlock('loading', 'Loading')}</td></tr>`;
    } else if (list.items === undefined) {
        rows = `<tr><td colspan="${span}" class="state-cell">${stateBlock('error', `${kind === 'sources' ? 'Sources' : 'Reviews'} unavailable`, `${list.failure} · retried every 15 s`)}</td></tr>`;
    } else if (!list.items.length) {
        rows = `<tr><td colspan="${span}" class="state-cell">${stateBlock('empty', kind === 'sources' ? 'No source matches' : 'No review matches',
            '', listFiltersActive(kind) ? `<a class="btn btn-sm" href="#${route.name}">Clear filters</a>` : '')}</td></tr>`;
    } else {
        rows = kind === 'sources' ? renderSourceRows(list) : list.items.map((review, index) => renderReviewRow(review, index)).join('');
    }
    const failure = list.items !== undefined && list.failure
        ? `<div class="banner banner-bad" role="alert"><span class="banner-kind">Not refreshed</span><div class="banner-body">${escapeHtml(list.failure)} · retried every 15 s</div></div>` : '';
    document.getElementById('listBanner').innerHTML = failure;
    table.setAttribute('aria-busy', String(list.loading && list.items !== undefined));
    table.innerHTML = `${head}<tbody>${rows}</tbody>`;
    const pager = document.getElementById('listPager');
    pager.innerHTML = renderPager(kind);
    pager.hidden = !pager.innerHTML;
    if (list.items !== undefined) restoreListScroll();
    const top = document.getElementById('listPagerTop');
    if (top) {
        const buttons = pager.querySelector('.pager-buttons');
        top.innerHTML = buttons ? buttons.innerHTML : '';
        top.hidden = !buttons;
    }
}

// A header cell that sorts, or one that only names its column. `key` is the sort the API knows.
function renderColumnHead({label, key, className = '', sortable = true}) {
    const sort = currentSort();
    const sortKey = key && sortable ? key : '';
    const sorted = sortKey && sort.key === sortKey;
    const classes = [className, sortKey ? 'sortable' : ''].filter(Boolean).join(' ');
    const aria = sorted ? ` aria-sort="${sort.dir === 'asc' ? 'ascending' : 'descending'}"` : '';
    return `<th scope="col"${classes ? ` class="${classes}"` : ''}${aria}>${sortKey
        ? `<button type="button" onclick="sortList('${sortKey}')">${escapeHtml(label)} ${sortIcon()}</button>` : escapeHtml(label)}</th>`;
}

// The column the table is sorted by and its direction; the review queue order reads as the
// oldest first on the Opened column.
function currentSort() {
    const sources = listRouteKind() === 'sources';
    const defaults = sources ? SOURCE_SORTS : REVIEW_SORTS;
    const fallback = sources ? 'attention' : 'queue';
    let key = listParameter('sort');
    if (!(key in defaults)) key = fallback;
    const dir = listParameter('dir') === 'asc' || listParameter('dir') === 'desc' ? listParameter('dir') : defaults[key];
    return {key: key === 'queue' ? 'created' : key, dir, queue: key === 'queue'};
}

function sortList(key) {
    const sources = listRouteKind() === 'sources';
    const defaults = sources ? SOURCE_SORTS : REVIEW_SORTS;
    const current = currentSort();
    const dir = current.key === key ? (current.dir === 'asc' ? 'desc' : 'asc') : defaults[key];
    setListParams({sort: key, dir}, {filters: false});
}

// What the last refresh changed in the rows, as a note the pager line carries.
function listChangeNote(kind) {
    const changes = lists[kind].changes;
    if (!changes) return '';
    const noun = kind === 'sources' ? ['source', 'sources'] : ['review', 'reviews'];
    const parts = [];
    if (changes.left) parts.push(`${pluralize(changes.left, noun[0], noun[1])} left this list since you opened it`);
    if (changes.added.size) parts.push(`${formatCount(changes.added.size)} new`);
    return `<span class="pager-note">${parts.join(' · ')}</span>`;
}

function renderPager(kind) {
    const list = lists[kind];
    if (list.items === undefined) return '';
    const note = listChangeNote(kind);
    const html = renderPagerCounts(kind);
    if (!note) return html;
    return html ? html.replace('</div>', ` ${note}</div>`) : `<div class="pager-count">${note}</div>`;
}

function renderPagerCounts(kind) {
    const list = lists[kind];
    const noun = kind === 'sources' ? ['source', 'sources'] : ['review', 'reviews'];
    const offset = Math.max(0, Number(listParameter('offset')) || 0);
    const total = list.total;
    const filtered = listFiltersActive(kind);
    const oldestFirst = kind === 'reviews' && currentSort().queue ? ' · oldest first' : '';
    const unfiltered = filtered && list.unfilteredTotal !== null && list.unfilteredTotal !== total
        ? ` of <strong>${formatCount(list.unfilteredTotal)}</strong>` : '';
    if (total <= PAGE_SIZE && offset === 0) {
        if (!filtered && !oldestFirst) return '';
        return `<div class="pager-count"><strong>${formatCount(total)}</strong>${unfiltered} ${noun[total === 1 ? 0 : 1]}${filtered ? ` match · <a href="#${route.name}">show all</a>` : ''}${oldestFirst}</div>`;
    }
    const pages = Math.max(1, Math.ceil(total / PAGE_SIZE));
    const first = total ? offset + 1 : 0;
    const last = Math.min(total, offset + list.items.length);
    const count = filtered
        ? `<strong>${formatCount(first)}–${formatCount(last)}</strong> of <strong>${formatCount(total)}</strong> ${noun[1]} match`
        : `<strong>${formatCount(first)}–${formatCount(last)}</strong> of <strong>${formatCount(total)}</strong> ${noun[1]}`;
    return `<div class="pager-count">${count}${oldestFirst}</div>
      <div class="pager-buttons">
        <button class="btn btn-sm" ${offset > 0 ? '' : 'disabled'} onclick="setListParams({offset: ${Math.max(0, offset - PAGE_SIZE)}}, {filters: false})" type="button">Previous</button>
        <span class="pager-page">${formatCount(Math.floor(offset / PAGE_SIZE) + 1)} / ${formatCount(pages)}</span>
        <button class="btn btn-sm" ${offset + PAGE_SIZE < total ? '' : 'disabled'} onclick="setListParams({offset: ${offset + PAGE_SIZE}}, {filters: false})" type="button">Next</button>
      </div>`;
}

// ───────────────────────────── ListingSource status ─────────────────────────────
// The conditions of a ListingSource, most severe first; only a crawl-enabled one has any. Both the
// table's one badge and the source page's status lines come from them.
function sourceConditions(source) {
    if (!source.crawl_enabled) return [];
    const conditions = [];
    if (source.last_crawl_outcome === 'NEEDS_OPERATOR') {
        const failure = describeFailure(source.last_crawl_error_kind);
        conditions.push({
            key: 'crawl_needs_operator', text: 'Crawl needs operator',
            more: `${failure.label} · ${pluralize(Number(source.crawl_failure_count) || 0, 'failed crawl', 'failed crawls')}`,
            detail: failure.cause,
            title: 'The crawl is not retried until an operator resumes it, the presentation URL changes in the admin API or crawling is disabled'
        });
    }
    if (source.last_crawl_outcome === 'FAILED') {
        const failure = describeFailure(source.last_crawl_error_kind);
        const retry = source.next_crawl_at ? ` · retry ${untilText(source.next_crawl_at)}` : '';
        conditions.push({
            key: 'crawl_failing', text: 'Crawl failing',
            more: `${failure.label} · ${pluralize(Number(source.crawl_failure_count) || 0, 'failed crawl', 'failed crawls')}${retry}`,
            detail: failureDetail(failure), title: failureDetail(failure)
        });
    }
    const streak = Number(source.scrape_failure_streak) || 0;
    if (streak > 0) {
        const paused = source.scrape_paused_until
            ? ` · retries ${Date.parse(source.scrape_paused_until) <= Date.now() ? 'now' : untilText(source.scrape_paused_until)}`
            : '';
        const failure = describeFailure(source.last_scrape_error_kind, source.last_scrape_status_code);
        conditions.push({
            key: 'failing_to_scrape', text: 'Failing to scrape',
            more: `${failure.label} · ${streak} in a row${paused}`,
            detail: failureDetail(failure), title: failureDetail(failure),
            since: source.scrape_failing_since, sinceLabel: 'Failing since'
        });
    }
    const blocked = Number(source.blocked_urls) || 0;
    if (blocked > 0) {
        const total = Number(source.product_urls) || 0;
        conditions.push({
            key: 'blocked_by_review', text: 'Blocked by review',
            more: `${blocked >= total ? `all ${formatCount(blocked)} URLs` : `${formatCount(blocked)} of ${formatCount(total)} URLs`}`,
            since: source.blocked_since, sinceLabel: 'Blocked since',
            title: 'These URLs are not scraped until the pending product-schema review is decided'
        });
    }
    const held = Number(source.held_removal_urls) || 0;
    if (held > 0 || source.mass_removal_confirmed_until) {
        conditions.push({
            key: 'removals_held', text: 'Removals held',
            more: source.mass_removal_confirmed_until
                ? `confirmed until ${formatDate(source.mass_removal_confirmed_until)}`
                : `${formatCount(held)} held`,
            since: source.mass_removal_confirmed_until ? null : source.held_since, sinceLabel: 'Last held',
            title: 'Kept by the mass-removal guard'
        });
    }
    return conditions;
}

// The one status a table shows for a ListingSource: its most severe condition, else whether it
// scrapes yet.
function primaryStatus(source) {
    if (source.source_label) {
        const applied = source.source_label;
        return {
            key: 'labelled', tone: sourceLabelTone(applied.label), text: sourceLabelName(applied.label),
            more: applied.note || '', note: true,
            title: `Labelled by ${actorWord(applied.labelled_by)}; not crawled until a human clears the label`
        };
    }
    if (!source.crawl_enabled) return {key: 'crawling_disabled', text: 'Crawling disabled', more: ''};
    const [condition] = sourceConditions(source);
    if (condition) {
        const since = condition.since ? ` · ${elapsedTime(condition.since).replace(/ ago$/, '')}` : '';
        return {key: condition.key, text: condition.text, more: `${condition.more}${since}`, title: condition.title};
    }
    const missing = Number(source.missing_value_urls) || 0;
    if (!source.last_successful_scrape) return {key: 'not_scraped_yet', text: 'Not scraped yet', more: ''};
    return {key: 'healthy', text: 'Healthy', more: missing > 0 ? `${pluralize(missing, 'page', 'pages')} missing a value` : ''};
}

function statusKeyBadge(status) {
    return `<span class="badge badge-${status.tone || TONES[status.key] || 'neutral'}"${status.title ? ` title="${escapeHtmlAttr(status.title)}"` : ''}>${escapeHtml(status.text)}</span>`;
}

function activityBadges(source) {
    return [
        source.crawling_since ? `<span class="badge badge-${TONES.crawling}" title="${escapeHtmlAttr(`Crawling since ${formatDate(source.crawling_since)}`)}"><span class="dot dot-info"></span>Crawling</span>` : '',
        source.scraping_since ? `<span class="badge badge-${TONES.scraping}" title="${escapeHtmlAttr(`Scraping since ${formatDate(source.scraping_since)}`)}"><span class="dot dot-info"></span>Scraping</span>` : ''
    ].filter(Boolean).join(' ');
}

// Why a crawl, a scrape or a page failed, in words an operator can act on. The keys are the kinds
// the crawler persists (crawl kinds, scrape-circuit kinds and per-page kinds); `profile` says what
// happens next. A test keeps the crawl and circuit kinds in step with the crawler.
const FAILURE_NEXT = {
    transient: 'Retried automatically with a growing delay; after five failures in a row it waits for an operator.',
    recoverable: 'Retried with a growing delay; after five failures in a row it waits for an operator.',
    durable: 'Retried once; after two failures in a row it waits for an operator, since the shop has to let the crawler in.',
    internal: 'Retried automatically; the delay doubles up to 24 hours.',
    circuit: 'The scraper pauses this source, then probes one page and resumes once the shop answers.',
    page: 'The page is retried after its cooldown.'
};
const FAILURES = {
    EmptyCrawl: {label: 'Empty crawl', cause: 'The crawl found no pages at all.', profile: 'transient'},
    TinyCrawl: {label: 'Too few pages', cause: 'The crawl reached only a handful of pages, too few to trust.', profile: 'transient'},
    NoAcceptedPages: {label: 'No pages accepted', cause: 'Every page the crawl fetched was rejected.', profile: 'transient'},
    RobotsBlocked: {label: 'Blocked by robots.txt', cause: "The shop's robots.txt forbids crawling its start page.", profile: 'recoverable'},
    RateLimited: {label: 'Rate limited', cause: 'The shop asked the crawler to slow down.', profile: 'transient'},
    AccessDenied: {label: 'Access denied', cause: 'The shop refuses the crawler with a 401 or 403.', profile: 'durable'},
    CloudflareChallenge: {label: 'Cloudflare challenge', cause: 'A Cloudflare check stops the crawler before it reaches the shop.', profile: 'transient'},
    BotProtection: {label: 'Bot protection', cause: "The shop's bot protection blocks the request.", profile: 'transient'},
    TlsError: {label: 'TLS error', cause: "The shop's certificate or secure connection is broken.", profile: 'recoverable'},
    ConnectError: {label: 'Connection failed', cause: 'The crawler could not connect to the shop.', profile: 'transient'},
    ServerError: {label: 'Server error', cause: 'The shop answered with a server error (5xx).', profile: 'transient'},
    RedirectProblem: {label: 'Redirect problem', cause: 'The start page redirects somewhere the crawler does not follow, such as another host.', profile: 'recoverable'},
    InvalidUrl: {label: 'Invalid URL', cause: 'The start URL cannot be crawled.', profile: 'recoverable'},
    JavascriptRequired: {label: 'JavaScript required', cause: 'The shop shows nothing without JavaScript, which the crawler does not run.', profile: 'durable'},
    InsufficientInferenceSample: {label: 'Too few pages to classify', cause: 'The crawl found too few pages to tell which URLs are products.', profile: 'recoverable'},
    NoProductUrls: {label: 'No product URLs', cause: 'The crawl found no URL that looks like a product page.', profile: 'recoverable'},
    NoUrlPattern: {label: 'No URL pattern', cause: 'No URL pattern separates the shop\'s product pages.', profile: 'recoverable'},
    PendingUrlPatternReview: {label: 'URL pattern in review', cause: 'The crawl waits for the URL-pattern review.', profile: 'recoverable'},
    spider_run_error: {label: 'Crawler error', cause: 'The crawl failed on an internal or configuration fault.', profile: 'internal'},
    TIMEOUT: {label: 'Timeouts', cause: 'Pages of the shop do not answer in time.', profile: 'circuit'},
    CONNECT: {label: 'Connection failures', cause: 'The shop does not accept connections.', profile: 'circuit'},
    DNS_TIMEOUT: {label: 'DNS timeouts', cause: "The shop's address does not resolve in time.", profile: 'circuit'},
    DNS_RESOLUTION: {label: 'DNS failure', cause: "The shop's address does not resolve.", profile: 'circuit'},
    HTTP_408: {label: 'Request timeouts (408)', cause: 'The shop answers 408: it gave up waiting for the request.', profile: 'circuit'},
    HTTP_429: {label: 'Rate limited (429)', cause: 'The shop answers 429: it asks the scraper to slow down.', profile: 'circuit'},
    HTTP_503: {label: 'Unavailable (503)', cause: 'The shop answers 503: it reports it is unavailable.', profile: 'circuit'},
    HTTP_504: {label: 'Gateway timeout (504)', cause: 'The shop answers 504: its gateway timed out.', profile: 'circuit'},
    HTTP_5XX_BURST: {label: 'Server errors (5xx)', cause: 'Several pages in a row answered with a server error.', profile: 'circuit'},
    HTTP_STATUS_BURST: {label: 'Rejected by the shop', cause: 'Several pages in a row answered with the same status, which means the shop rejects the scraper.', profile: 'circuit'},
    HttpError: {label: 'HTTP error', cause: 'The page answered with an error status.', profile: 'page'},
    RobotsDisallowed: {label: 'Disallowed by robots.txt', cause: "The shop's robots.txt forbids this page.", profile: 'page'},
    Timeout: {label: 'Timeout', cause: 'The page did not answer in time.', profile: 'page'},
    RawCaptureFailed: {label: 'Capture failed', cause: "The page's content could not be stored.", profile: 'page'},
    SourceListingIdConflict: {label: 'Identity conflict', cause: 'Two pages claim the same listing identity.', profile: 'page'},
    FreshSchemaNormalizationFailed: {label: 'New schema failed', cause: 'A freshly generated schema extracts values that fail the plausibility checks.', profile: 'page'},
    SchemaGenerationValidationFailed: {label: 'Schema generation failed', cause: 'The generated schemas did not pass the checks.', profile: 'page'}
};

// What an HTTP status says when no table entry does.
function statusCause(code) {
    if (code >= 500) return `The shop failed to answer (HTTP ${code}).`;
    if (code >= 400) return `The shop rejected the request (HTTP ${code}).`;
    if (code >= 300) return `The shop answered with an unexpected redirect (HTTP ${code}).`;
    return `The shop answered HTTP ${code}.`;
}

// The label, the cause and what happens next of a persisted failure `kind`, with the HTTP status
// the failure recorded, if any. A kind this table does not know shows as persisted.
function describeFailure(kind, statusCode = null) {
    const text = String(kind || '');
    const code = Number(statusCode) || Number((/^HTTP_(\d{3})$/.exec(text) || [])[1]) || 0;
    const entry = Object.hasOwn(FAILURES, text) ? FAILURES[text] : null;
    if (!entry) {
        if (code) return {label: `HTTP ${code}`, cause: statusCause(code), next: ''};
        return {label: text || 'Unknown failure', cause: '', next: ''};
    }
    const label = code && !entry.label.includes(String(code)) ? `${entry.label} (${code})` : entry.label;
    return {
        label,
        cause: text === 'HttpError' && code ? statusCause(code) : entry.cause,
        next: FAILURE_NEXT[entry.profile]
    };
}

function failureDetail(failure) {
    return [failure.cause, failure.next].filter(Boolean).join(' ');
}

// A source with crawling off, labelled or disabled, has no next crawl whatever its timer says.
function nextCrawlText(source) {
    const next = source.crawl_enabled ? source.next_crawl_at : null;
    if (!next) return '<span class="muted">—</span>';
    return timeSpan(next, 'due', `Next crawl: ${formatDate(next)}`);
}

// ───────────────────────────── Sources table ─────────────────────────────
function sourceColumns() {
    return [
        {label: 'Source', key: 'name'},
        {label: 'Status', key: 'attention'},
        {label: 'Pending', key: 'pending_reviews', className: 'num'},
        {label: 'Last scrape', key: 'last_successful_scrape', className: 'col-md'},
        {label: 'Next crawl', key: 'next_crawl_at', className: 'col-md'}
    ];
}

// The ListingSource rows. In the attention order, with no filter, a heading row names each
// condition the rows share, with how many sources have it.
const SOURCE_GROUP_LABELS = {
    crawl_needs_operator: 'Crawl needs operator', crawl_failing: 'Crawl failing', failing_to_scrape: 'Failing to scrape', blocked_by_review: 'Blocked by review',
    removals_held: 'Removals held', healthy: 'Healthy', not_scraped_yet: 'Not scraped yet', crawling_disabled: 'Crawling disabled', labelled: 'Labelled'
};

function sourceGroupCount(key) {
    const sources = overviewCounts?.sources;
    const counts = {
        crawl_needs_operator: sources?.crawl_needs_operator, crawl_failing: sources?.crawl_failing, failing_to_scrape: sources?.failing_to_scrape,
        blocked_by_review: sources?.blocked_by_review, removals_held: sources?.held_removals, labelled: sources?.labelled
    };
    return counts[key];
}

function renderSourceRows(list) {
    const grouped = currentSort().key === 'attention' && currentSort().dir === 'desc' && !listFiltersActive('sources');
    let group = null;
    return list.items.map(source => {
        const key = primaryStatus(source).key;
        let heading = '';
        if (grouped && key !== group) {
            const count = sourceGroupCount(key);
            heading = `<tr class="group-head"><th colspan="${visibleSourceColumns()}" scope="colgroup">${escapeHtml(SOURCE_GROUP_LABELS[key])}${count !== undefined ? ` · ${formatCount(count)}` : ''}</th></tr>`;
        }
        group = key;
        return heading + renderSourceRow(source);
    }).join('');
}

// How many of the sources table's columns show at this width: the last two hide below 1100 px.
function visibleSourceColumns() {
    return window.innerWidth < 1100 ? 3 : 5;
}

let sourceColumnsShown = visibleSourceColumns();
window.addEventListener('resize', () => {
    const shown = visibleSourceColumns();
    if (shown !== sourceColumnsShown) {
        sourceColumnsShown = shown;
        if (listRouteKind() === 'sources') renderListBody('sources');
    }
});

function rowClasses(...classes) {
    const text = classes.filter(Boolean).join(' ');
    return text ? ` class="${text}"` : '';
}

function renderSourceRow(source) {
    const status = primaryStatus(source);
    const pending = Number(source.pending_reviews) || 0;
    const blocking = REVIEW_ID_PATTERN.test(source.blocking_review_id || '') ? source.blocking_review_id : null;
    const pendingHref = `#reviews&listing_source=${encodeURIComponent(source.listing_source_id)}&status=PENDING_REVIEW`;
    const blockingLink = status.key === 'blocked_by_review' && blocking
        ? ` · <a href="${reviewHash(blocking)}">its review →</a>` : '';
    return `<tr${rowClasses(source.crawl_enabled ? '' : 'dim', listReturnId() === source.listing_source_id ? 'selected' : '', lists.sources.changes?.added.has(source.listing_source_id) ? 'changed' : '')}>
      <td><a class="row-link" href="${sourceHash(source.listing_source_id)}"><span class="cell-main">${escapeHtml(listingSourceName(source))}</span><span class="cell-sub">${escapeHtml(sourceHost(source))}</span></a></td>
      <td>${statusKeyBadge(status)}${status.more ? ` <span class="more${status.note ? ' more-note' : ''}"${status.note ? ` title="${escapeHtmlAttr(status.more)}"` : ''}>${escapeHtml(status.more)}${blockingLink}</span>` : ''}${activityBadges(source) ? ` ${activityBadges(source)}` : ''}</td>
      <td class="num">${pending ? `<a href="${escapeHtmlAttr(pendingHref)}">${formatCount(pending)}</a>` : '<span class="muted">0</span>'}</td>
      <td class="col-md">${source.last_successful_scrape ? timeBadge(source.last_successful_scrape, 'Last successful scrape') : '<span class="muted">Never</span>'}</td>
      <td class="col-md">${nextCrawlText(source)}</td>
    </tr>`;
}

// ───────────────────────────── Reviews table ─────────────────────────────
function reviewColumns() {
    return [
        {label: 'Review', key: 'source'},
        {label: 'Status', sortable: false},
        {label: 'Opened', key: 'created'},
        {label: 'Last change', key: 'updated', className: 'col-sm'}
    ];
}

function renderReviewRow(review, index) {
    const closed = review.status !== 'PENDING_REVIEW';
    const dim = closed || review.listing_source_crawl_enabled === false;
    const offset = Math.max(0, Number(listParameter('offset')) || 0);
    const queue = {key: queueKeyOfList(), index: offset + index};
    return `<tr${rowClasses(dim ? 'dim' : '', listReturnId() === review.review_id ? 'selected' : '', lists.reviews.changes?.added.has(review.review_id) ? 'changed' : '')}>
      <td><a class="row-link" href="${reviewHash(review.review_id, queue)}"><span class="cell-main">${escapeHtml(review.listing_source_name || 'Unnamed source')}</span><span class="cell-sub">${escapeHtml(`${artifactLabel(review.artifact_type)} · ${reasonWord(review.reason)}`)}</span></a></td>
      <td>${statusBadge(review.status)} ${reviewFlags(review)}</td>
      <td>${timeBadge(review.created, 'Opened')}</td>
      <td class="col-sm">${timeBadge(review.updated, 'Last change')}</td>
    </tr>`;
}

// ───────────────────────────── Source page ─────────────────────────────
// A ListingSource's own page: `detail` is its overview with its live configuration, `reviews` its
// review history. Each loads on its own, so one that fails does not hide the other.
let sourcePage = null;
const expandedMissingValueSources = new Map();
const openMassRemovalConfirmations = new Set();
// The sources whose label form is open on their page.
const openSourceLabelForms = new Set();
const MASS_REMOVAL_CONFIRMATION_HOURS = [[6, '6 hours'], [24, '24 hours'], [72, '3 days'], [168, '7 days']];

function sourcePageId() {
    return route.name === 'source' ? route.params.get('source') : null;
}

function renderSourcePage() {
    const id = sourcePageId();
    if (!sourcePage || sourcePage.id !== id) {
        sourcePage = {id, detail: undefined, detailFailure: '', notFound: false, reviews: undefined, reviewsTotal: 0, reviewsFailure: '', reviewsLoading: false};
    }
    showPage(sourcePageHtml());
}

function updateSourcePage() {
    if (route.name === 'source' && sourcePage) replaceKeepingFocus(document.getElementById('detail'), sourcePageHtml());
}

async function loadSourcePage() {
    const page = sourcePage;
    if (!page) return;
    const id = encodeURIComponent(page.id);
    const detailLoaded = api(`/api/listing-sources/${id}`).then(detail => {
        page.detail = detail;
        page.detailFailure = '';
        page.notFound = false;
    }, error => {
        if (error.status === 404) page.notFound = true;
        else page.detailFailure = apiErrorMessage(error);
    }).then(updateSourcePage);
    await Promise.all([detailLoaded, loadSourceReviews()]);
    noteRefresh('source', page.detailFailure || page.reviewsFailure || null);
}

async function loadSourceReviews() {
    const page = sourcePage;
    if (!page) return;
    const query = new URLSearchParams({limit: String(PAGE_SIZE)});
    const status = route.params.get('rstatus');
    const artifact = route.params.get('rartifact');
    if (status === 'open') query.set('status', 'PENDING_REVIEW');
    else if (status) query.set('status', status);
    if (artifact) query.set('artifact', artifact);
    const offset = Number(route.params.get('roffset')) || 0;
    if (offset > 0) query.set('offset', String(offset));
    page.reviewsLoading = true;
    try {
        const answer = await api(`/api/listing-sources/${encodeURIComponent(page.id)}/reviews?${query}`);
        page.reviews = answer.items;
        page.reviewsTotal = answer.total;
        page.reviewsFailure = '';
    } catch (error) {
        page.reviewsFailure = apiErrorMessage(error);
    }
    page.reviewsLoading = false;
    updateSourcePage();
}

function setSourceReviewParams(changes) {
    const params = new URLSearchParams(route.params);
    for (const [name, value] of Object.entries(changes)) {
        if (value === null || value === '') params.delete(name);
        else params.set(name, String(value));
    }
    if (!('roffset' in changes)) params.delete('roffset');
    route = {name: 'source', params};
    history.replaceState(null, '', routeHash('source', params, params.get('source')));
    updateSourcePage();
    loadSourceReviews();
}

function setSourceSchema(index) {
    const params = new URLSearchParams(route.params);
    if (index) params.set('schema', String(index));
    else params.delete('schema');
    route = {name: 'source', params};
    history.replaceState(null, '', routeHash('source', params, params.get('source')));
    updateSourcePage();
}

function sourcePageHtml() {
    const page = sourcePage;
    if (page.notFound) {
        return `<div class="state state-empty state-hero"><div class="state-title">Source not found</div><div class="state-actions"><a class="btn btn-sm" href="#sources">Back to sources</a></div></div>`;
    }
    const detail = page.detail;
    if (detail === undefined && !page.detailFailure) {
        return `<nav class="crumbs" aria-label="Breadcrumb"><a href="#sources">Sources</a></nav>${stateBlock('loading', 'Loading')}`;
    }
    if (detail === undefined) {
        return `<nav class="crumbs" aria-label="Breadcrumb"><a href="#sources">Sources</a></nav>${stateBlock('error', 'Source unavailable', page.detailFailure, '<button class="btn btn-sm" onclick="loadSourcePage()" type="button">Retry</button>')}`;
    }
    const status = primaryStatus(detail);
    const host = sourceHost(detail);
    const root = isHttpUrl(detail.crawl_root_url) ? `<a href="${escapeHtmlAttr(detail.crawl_root_url)}" target="_blank" rel="noopener noreferrer">${escapeHtml(host)}</a>` : escapeHtml(host);
    const meta = [root, pluralize(Number(detail.product_urls) || 0, 'product URL', 'product URLs'),
        detail.last_successful_crawl ? `last crawl ${timeBadge(detail.last_successful_crawl, 'Last successful crawl')}` : 'never crawled',
        detail.last_successful_scrape ? `last scrape ${timeBadge(detail.last_successful_scrape, 'Last successful scrape')}` : 'never scraped'];
    return `<nav class="crumbs" aria-label="Breadcrumb"><a href="#sources">Sources</a><span class="sep">/</span><span>${escapeHtml(listingSourceName(detail))}</span></nav>
      <div class="page-head"><div>
        <div class="page-title"><h1>${escapeHtml(listingSourceName(detail))}</h1>${statusKeyBadge(status)}${activityBadges(detail)}</div>
        <div class="page-meta">${meta.map(part => `<span>${part}</span>`).join('<span>·</span>')}</div>
      </div></div>
      <div class="source-grid">
        <div class="stack">${renderSourceHealth(detail)}${renderSourceReviewsCard()}</div>
        <div class="stack">${renderSourceUrlPattern(detail)}${renderSourceSchemas(detail)}${renderSourceRemovedTemplates(detail)}</div>
      </div>`;
}

function conditionLine(condition, {text = escapeHtml(condition.more), trail = ''} = {}) {
    const since = condition.since ? ` · since ${timeBadge(condition.since, condition.sinceLabel, true)}` : '';
    const detail = condition.detail ? `<div class="status-line-detail">${escapeHtml(condition.detail)}</div>` : '';
    return `<div class="status-line"><span class="badge badge-${TONES[condition.key] || 'neutral'}">${escapeHtml(condition.text)}</span><div class="status-line-text">${text}${since}${trail}${detail}</div></div>`;
}

// One line per condition of the source, with the actions that resolve it inline.
function conditionLines(source) {
    const id = escapeHtmlAttr(source.listing_source_id);
    return sourceConditions(source).map(condition => {
        if (condition.key === 'blocked_by_review') {
            const review = REVIEW_ID_PATTERN.test(source.blocking_review_id || '')
                ? `<a href="${reviewHash(source.blocking_review_id)}">the pending product-schema review</a>` : 'a pending review';
            return conditionLine(condition, {text: `${escapeHtml(condition.more)} wait on ${review}`});
        }
        if (condition.key === 'crawl_needs_operator') {
            return conditionLine(condition, {trail: ` · <button class="btn-link" onclick="resumeCrawl('${id}')" title="Crawl the source again on its schedule, with a new failure streak" type="button">Resume crawl</button>`});
        }
        if (condition.key !== 'removals_held') return conditionLine(condition);
        if (source.mass_removal_confirmed_until) {
            return conditionLine(condition, {trail: ` · by ${escapeHtml(actorWord(source.mass_removal_confirmed_by))} · <button class="btn-link" onclick="revokeMassRemovalConfirmation('${id}')" type="button">Revoke</button>`});
        }
        const open = openMassRemovalConfirmations.has(source.listing_source_id);
        return conditionLine(condition, {trail: ` · <button class="btn-link" aria-expanded="${open}" onclick="toggleMassRemovalConfirmation('${id}')" type="button">Confirm bulk removal</button>`})
            + (open ? renderMassRemovalConfirmation(source.listing_source_id) : '');
    });
}

// Up to three product pages whose last scrape failed, as examples of why the source fails: the
// path, what failed, the status and the shop's message as the page's row stores it.
function renderFailingUrls(source) {
    const urls = source.crawl_enabled ? source.failing_urls || [] : [];
    if (!urls.length) return '';
    const items = urls.map(item => {
        const failure = describeFailure(item.error_kind, item.status_code);
        const message = item.error_message ? `<span class="failing-url-message">${escapeHtml(item.error_message)}${item.error_message.length >= 200 ? '…' : ''}</span>` : '';
        const retry = item.next_retry_at ? ` · retry ${escapeHtml(untilText(item.next_retry_at))}` : '';
        const path = relativeToOrigin(item.url, urlOrigin(item.url));
        const target = isHttpUrl(item.url) ? `<a class="mono" href="${escapeHtmlAttr(item.url)}" target="_blank" rel="noopener noreferrer">${escapeHtml(path)}</a>` : `<span class="mono">${escapeHtml(path)}</span>`;
        return `<li>${target}<span class="failing-url-kind">${escapeHtml(failure.label)} · ${pluralize(Number(item.failure_count) || 0, 'failure', 'failures')}${retry}</span>${message}</li>`;
    }).join('');
    return `<div class="failing-urls"><div class="failing-urls-title">Failing pages</div><ul>${items}</ul></div>`;
}

function renderSourceHealth(source) {
    const id = escapeHtmlAttr(source.listing_source_id);
    const lines = conditionLines(source);
    const activity = [];
    if (source.crawling_since) activity.push(`<div class="status-line"><span class="badge badge-info"><span class="dot dot-info"></span>Crawling</span><div class="status-line-text">since ${timeBadge(source.crawling_since, 'Crawling since', true)}</div></div>`);
    if (source.scraping_since) activity.push(`<div class="status-line"><span class="badge badge-info"><span class="dot dot-info"></span>Scraping</span><div class="status-line-text">since ${timeBadge(source.scraping_since, 'Scraping since', true)}</div></div>`);
    const missing = Number(source.missing_value_urls) || 0;
    if (source.crawl_enabled && missing > 0) {
        const open = expandedMissingValueSources.has(source.listing_source_id);
        lines.push(`<div class="status-line"><span class="badge badge-neutral">Missing a value</span><div class="status-line-text">${pluralize(missing, 'page', 'pages')} missing a required value · <button class="btn-link" aria-expanded="${open}" onclick="toggleMissingValuePages('${id}')" type="button">${open ? 'hide' : 'show'}</button></div></div>`);
        if (open) lines.push(renderMissingValuePages(source.listing_source_id, expandedMissingValueSources.get(source.listing_source_id)));
    }
    if (source.source_label) lines.push(renderSourceLabelLine(source));
    else if (!source.crawl_enabled) lines.push('<div class="status-line"><span class="badge badge-neutral">Crawling disabled</span><div class="status-line-text">The source is neither crawled nor scraped.</div></div>');
    if (!source.source_label && openSourceLabelForms.has(source.listing_source_id)) lines.push(renderSourceLabelForm(source.listing_source_id));
    const crawl = source.last_crawl_outcome
        ? `${escapeHtml(reasonWord(source.last_crawl_outcome))}${source.last_successful_crawl ? ` ${timeBadge(source.last_successful_crawl, 'Last successful crawl')}` : ''}${Number(source.crawl_failure_count) > 0 && source.last_crawl_outcome !== 'NEEDS_OPERATOR' ? ` · ${pluralize(Number(source.crawl_failure_count), 'failed crawl', 'failed crawls')}` : ''}`
        : 'not crawled yet';
    // The status lines say what the failure streak is; this row adds only when the source last scraped.
    const scrape = source.last_successful_scrape
        ? `${(Number(source.scrape_failure_streak) || 0) > 0 ? 'last success ' : 'last scraped '}${timeBadge(source.last_successful_scrape, 'Last successful scrape')}`
        : (Number(source.scrape_failure_streak) || 0) > 0 ? 'never scraped' : 'no failure streak';
    const errors = (source.url_error_kinds || []).filter(entry => !STATUS_ERROR_KINDS.has(entry.kind));
    return `<div class="card">
      <div class="card-head"><h2>Health</h2>${source.source_label ? '' : `<button class="btn btn-sm" aria-expanded="${openSourceLabelForms.has(source.listing_source_id)}" onclick="toggleSourceLabelForm('${id}')" title="Stop crawling a site that is not an online shop or that the crawler cannot scrape" type="button">Label source…</button>`}</div>
      <div class="card-body stack">
        ${lines.length || activity.length ? `<div class="status-lines">${lines.join('')}${activity.join('')}</div>` : ''}
        ${renderFailingUrls(source)}
        <dl class="kv">
          <dt>Crawl</dt><dd>${crawl} · next ${nextCrawlText(source)}</dd>
          <dt>Scrape</dt><dd>${scrape}</dd>
          ${errors.length ? `<dt>Errors</dt><dd>${errors.map(entry => `${formatCount(entry.count)} ${escapeHtml(describeFailure(entry.kind).label)}`).join(' · ')}</dd>` : ''}
        </dl>
      </div>
    </div>`;
}

// The last error kinds of the product URLs that a status line shows already.
const STATUS_ERROR_KINDS = new Set(['PendingSchemaReview', 'MassRemovalHeld', 'MissingRequiredValue']);

function renderSourceReviewsCard() {
    const page = sourcePage;
    const status = sourceParam('rstatus');
    const artifact = sourceParam('rartifact');
    const chips = [['', '', 'All'], ['open', '', 'Open'], ['', 'PRODUCT_SCHEMA', 'Product schema'], ['', 'URL_PATTERN', 'URL pattern']];
    const pressed = ([chipStatus, chipArtifact]) => chipStatus === status && chipArtifact === artifact;
    let body;
    if (page.reviews === undefined && !page.reviewsFailure) {
        body = `<tr><td colspan="4" class="state-cell">${stateBlock('loading', 'Loading')}</td></tr>`;
    } else if (page.reviews === undefined) {
        body = `<tr><td colspan="4" class="state-cell">${stateBlock('error', 'Reviews unavailable', page.reviewsFailure, '<button class="btn btn-sm" onclick="loadSourceReviews()" type="button">Retry</button>')}</td></tr>`;
    } else if (!page.reviews.length) {
        body = `<tr><td colspan="4" class="state-cell">${stateBlock('empty', 'No review matches')}</td></tr>`;
    } else {
        body = page.reviews.map(renderSourceReviewRow).join('');
    }
    const offset = Number(route.params.get('roffset')) || 0;
    const pager = page.reviewsTotal > PAGE_SIZE ? `<div class="pager">
        <div class="pager-count"><strong>${formatCount(offset + 1)}–${formatCount(Math.min(page.reviewsTotal, offset + (page.reviews || []).length))}</strong> of <strong>${formatCount(page.reviewsTotal)}</strong> reviews</div>
        <div class="pager-buttons">
          <button class="btn btn-sm" ${offset > 0 ? '' : 'disabled'} onclick="setSourceReviewParams({roffset: ${Math.max(0, offset - PAGE_SIZE)}})" type="button">Previous</button>
          <span class="pager-page">${formatCount(Math.floor(offset / PAGE_SIZE) + 1)} / ${formatCount(Math.ceil(page.reviewsTotal / PAGE_SIZE))}</span>
          <button class="btn btn-sm" ${offset + PAGE_SIZE < page.reviewsTotal ? '' : 'disabled'} onclick="setSourceReviewParams({roffset: ${offset + PAGE_SIZE}})" type="button">Next</button>
        </div></div>` : '';
    return `<div class="card table-card">
      <div class="card-head"><h2>Reviews</h2><div class="chips">${chips.map(chip =>
          `<button class="chip" aria-pressed="${pressed(chip)}" onclick="setSourceReviewParams({rstatus: '${chip[0]}', rartifact: '${chip[1]}'})" type="button">${escapeHtml(chip[2])}</button>`).join('')}</div></div>
      <table class="data-table" aria-busy="${page.reviewsLoading && page.reviews !== undefined}">
        <thead><tr><th scope="col">Review</th><th scope="col">Status</th><th scope="col" class="col-sm">Opened</th><th scope="col" class="col-sm">Last change</th></tr></thead>
        <tbody>${body}</tbody>
      </table>${pager}
    </div>`;
}

function sourceParam(name) {
    return route.params.get(name) || '';
}

function renderSourceReviewRow(review) {
    const live = review.status === 'APPROVED' && review.candidate_is_live;
    return `<tr${review.status === 'PENDING_REVIEW' ? '' : ' class="dim"'}>
      <td><a class="row-link" href="${reviewHash(review.review_id)}"><span class="cell-main">${escapeHtml(artifactLabel(review.artifact_type))}</span><span class="cell-sub">${escapeHtml(reasonWord(review.reason))}</span></a></td>
      <td>${statusBadge(review.status)} ${reviewFlags(review, {live, revoked: review.rejection_revoked})}</td>
      <td class="col-sm">${timeBadge(review.created, 'Opened')}</td>
      <td class="col-sm">${timeBadge(review.updated, 'Last change')}${review.last_actor ? ` <span class="muted">· ${escapeHtml(actorWord(review.last_actor).replace(/^the /, ''))}</span>` : ''}</td>
    </tr>`;
}

function renderSourceUrlPattern(source) {
    const pattern = source.url_pattern;
    if (!pattern) {
        return `<div class="card"><div class="card-head"><h2>URL pattern</h2><span class="badge badge-neutral">None</span></div></div>`;
    }
    const approved = pattern.approved_at
        ? `Approved ${timeBadge(pattern.approved_at, 'Approved')}${pattern.approved_by ? ` by ${escapeHtml(actorWord(pattern.approved_by))}` : ''}${REVIEW_ID_PATTERN.test(pattern.approved_review_id || '') ? ` · <a href="${reviewHash(pattern.approved_review_id)}">open its review</a>` : ''}`
        : '';
    return `<div class="card">
      <div class="card-head"><h2>URL pattern</h2><span class="badge badge-ok">Live</span></div>
      <div class="card-body stack">
        <div class="decision"><code>${escapeHtml(pattern.value)}</code>${pattern.exclude ? `<span class="small muted">excluding</span><code>${escapeHtml(pattern.exclude)}</code>` : ''}${matchingNote(pattern.matching)}</div>
        ${approved ? `<div class="small muted">${approved}</div>` : ''}
      </div>
    </div>`;
}

function renderSourceSchemas(source) {
    const schemas = source.product_schemas || [];
    if (!schemas.length) {
        return `<div class="card"><div class="card-head"><h2>Product schemas</h2><span class="badge badge-neutral">None</span></div></div>`;
    }
    const index = Math.min(Math.max(0, Number(route.params.get('schema')) || 0), schemas.length - 1);
    const schema = schemas[index];
    const edit = REVIEW_ID_PATTERN.test(source.live_schema_review_id || '')
        ? ` · <a href="${reviewHash(source.live_schema_review_id)}">edit in its review</a>` : '';
    return `<div class="card">
      <div class="card-head"><h2>Product schemas</h2><span class="badge badge-ok">Live</span><span class="small muted">applied in this order</span></div>
      <div class="card-body stack">
        ${schemas.length > 1 ? `<div class="schema-tabs">${schemas.map((_, i) =>
            `<button class="chip" aria-pressed="${i === index}" onclick="setSourceSchema(${i})" type="button">${schemaLabel(i)}</button>`).join('')}</div>` : ''}
        <div class="rule-list">${schemaFields(schema).map(field => {
            const rule = isRawAttributeField(field) ? (schema.raw_attributes || {})[fieldKey(field)] : schema[field];
            return rule ? renderSchemaRule(field, rule) : '';
        }).join('')}</div>
        ${source.live_schemas_since ? `<div class="small muted">Live since ${timeBadge(source.live_schemas_since, 'Live since', true)}${edit}</div>` : ''}
      </div>
    </div>`;
}

function renderSchemaRule(field, rule) {
    const type = String(rule.type || 'text');
    const meta = [displayName(type).toLowerCase() + (rule.name ? ` ${rule.name}` : '') + (rule.property ? ` ${displayName(rule.property).toLowerCase()}` : ''),
        rule.cardinality === 'all' ? 'all' : '',
        (rule.additional_selectors || []).length ? `+${rule.additional_selectors.length} fallback${rule.additional_selectors.length === 1 ? '' : 's'}` : ''].filter(Boolean).join(' · ');
    return `<div class="rule"><span class="rule-field">${escapeHtml(fieldLabel(field))}</span><code>${escapeHtml(rule.selector || '')}</code><span class="rule-meta">${escapeHtml(meta)}</span></div>`;
}

function renderSourceRemovedTemplates(source) {
    const templates = source.removed_page_schemas || [];
    if (!templates.length) return '';
    return `<details class="card disclosure">
      <summary><span class="chev" aria-hidden="true">${chevronIcon()}</span><strong>Removed-page templates</strong><span class="muted">${templates.length}</span></summary>
      <div class="card-body"><div class="rule-list">${templates.map((template, index) =>
          `<div class="rule"><span class="rule-field">Template ${index + 1}</span><code>${escapeHtml(template.selector || '')}</code><span class="rule-meta">${escapeHtml(template.regex !== undefined && template.regex !== null ? `regex ${template.regex}` : `text “${template.text || ''}”`)}</span></div>`).join('')}</div></div>
    </details>`;
}

// A page whose last scrape left a required value empty opens and joins no review, such as a
// removal page that keeps an empty title element. The source page lists such pages, so a reviewer
// adds one to the product-schema review (opened for it when none is pending) and declares a
// removed-page template for it there. The scrape's outcome message names the blank values in
// backticks, such as "`title` is blank".
function missingValuesText(message) {
    const fields = [...String(message || '').matchAll(/`([^`]+)`/g)].map(match => displayName(match[1]));
    if (!fields.length) return String(message || '').trim();
    return `${fields.join(' and ')} ${fields.length === 1 ? 'is' : 'are'} empty`;
}

// The handler names a page by its position in the listed pages, never by its URL: a shop's URL
// may hold a quote, which the browser decodes from an HTML entity before the handler runs, so a
// URL in the handler's source would let the shop run script in the console.
function renderMissingValuePages(listingSourceId, pages) {
    if (pages.error) return `<div class="list"><div class="list-row muted">${escapeHtml(pages.error)}</div></div>`;
    return `<div class="list">${pages.map((page, index) => {
        const label = missingValuesText(page.missing);
        return `<div class="list-row"><span class="grow">${pageUrlLink(page.url)}${label ? ` <span class="muted">${escapeHtml(label)}</span>` : ''}</span>
        <button class="btn btn-sm" onclick="reviewMissingValuePage('${escapeHtmlAttr(listingSourceId)}', ${index})" type="button">Add to review</button></div>`;
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
    updateSourcePage();
}

async function reviewMissingValuePage(listingSourceId, index) {
    const pages = expandedMissingValueSources.get(listingSourceId);
    const url = Array.isArray(pages) ? pages[index]?.url : undefined;
    if (!url) return;
    try {
        const added = await api(
            `/api/listing-sources/${encodeURIComponent(listingSourceId)}/missing-value-pages/review`,
            {method: 'POST', body: JSON.stringify({url})}
        );
        location.hash = reviewHash(added.review_id);
    } catch (error) {
        alert(apiErrorMessage(error));
    }
}

// A source whose removals the mass-removal guard holds is confirmed as a real bulk clearance for
// a bounded time, which captures its removals, the held ones at once. The handler names the
// source by id, read from the row's own select, never from the page's HTML.
function renderMassRemovalConfirmation(listingSourceId) {
    const id = escapeHtmlAttr(listingSourceId);
    return `<div class="list-row">
      <span>Capture every removal of this source, the held ones at once, for</span>
      <select class="sm" aria-label="Confirmation duration" id="massRemovalHours-${id}">${MASS_REMOVAL_CONFIRMATION_HOURS.map(([hours, label]) =>
          `<option value="${hours}"${hours === 24 ? ' selected' : ''}>${label}</option>`).join('')}</select>
      <button class="btn btn-primary btn-sm" onclick="confirmMassRemoval('${id}')" type="button">Confirm</button>
      <button class="btn btn-quiet btn-sm" onclick="toggleMassRemovalConfirmation('${id}')" type="button">Cancel</button>
    </div>`;
}

function toggleMassRemovalConfirmation(listingSourceId) {
    if (!openMassRemovalConfirmations.delete(listingSourceId)) openMassRemovalConfirmations.add(listingSourceId);
    updateSourcePage();
}

async function confirmMassRemoval(listingSourceId) {
    const hours = Number(document.getElementById(`massRemovalHours-${listingSourceId}`)?.value);
    if (!hours) return;
    try {
        await api(`/api/listing-sources/${encodeURIComponent(listingSourceId)}/mass-removal-confirmation`,
            {method: 'POST', body: JSON.stringify({hours})});
        openMassRemovalConfirmations.delete(listingSourceId);
        await loadSourcePage();
    } catch (error) {
        alert(apiErrorMessage(error));
    }
}

async function revokeMassRemovalConfirmation(listingSourceId) {
    try {
        await api(`/api/listing-sources/${encodeURIComponent(listingSourceId)}/mass-removal-confirmation`, {method: 'DELETE'});
        await loadSourcePage();
    } catch (error) {
        alert(apiErrorMessage(error));
    }
}

// ───────────────────────────── ListingSource labels ─────────────────────────────
// What a human labels a source with when the crawler should not crawl it, in the order the form
// offers them. The keys are the labels the crawler persists.
const SOURCE_LABELS = {
    NOT_AN_ESHOP: 'NOT AN E-SHOP',
    APPROACH_NOT_APPLICABLE: 'APPROACH NOT APPLICABLE',
    BOT_PROTECTION: 'BOT PROTECTION',
    JAVASCRIPT_RENDERED: 'JAVASCRIPT RENDERED',
    LOGIN_REQUIRED: 'LOGIN REQUIRED',
    OTHER: 'OTHER',
};

// A label's colour says what kind of reason it is: blue for a site that is no shop, nothing
// broken; amber where the crawler's technique does not fit the shop; red where the shop keeps the
// crawler out; grey for any other reason.
const SOURCE_LABEL_TONES = {
    NOT_AN_ESHOP: 'info', APPROACH_NOT_APPLICABLE: 'warn', JAVASCRIPT_RENDERED: 'warn',
    BOT_PROTECTION: 'bad', LOGIN_REQUIRED: 'bad', OTHER: 'neutral'
};

function sourceLabelTone(label) {
    return SOURCE_LABEL_TONES[label] || 'neutral';
}

function sourceLabelName(label) {
    return SOURCE_LABELS[label] || displayName(label);
}

function renderSourceLabelLine(source) {
    const applied = source.source_label;
    const id = escapeHtmlAttr(source.listing_source_id);
    const note = applied.note ? `${escapeHtml(applied.note)} · ` : '';
    return `<div class="status-line"><span class="badge badge-${sourceLabelTone(applied.label)}">${escapeHtml(sourceLabelName(applied.label))}</span><div class="status-line-text">${note}not crawled · by ${escapeHtml(actorWord(applied.labelled_by))} ${timeBadge(applied.labelled_at, 'Labelled')} · <button class="btn-link" onclick="clearSourceLabel('${id}')" title="Crawl the source again once the next scope refresh enables it" type="button">Clear label</button></div></div>`;
}

// The handler names the source by id and reads the form's own fields, never the page's HTML.
function renderSourceLabelForm(listingSourceId) {
    const id = escapeHtmlAttr(listingSourceId);
    return `<div class="list-row">
      <select class="sm" aria-label="Label" id="sourceLabel-${id}">${Object.entries(SOURCE_LABELS).map(([label, name]) =>
          `<option value="${label}">${escapeHtml(name)}</option>`).join('')}</select>
      <input class="sm" aria-label="Note" id="sourceLabelNote-${id}" maxlength="500" placeholder="Note (required for OTHER)">
      <button class="btn btn-primary btn-sm" onclick="labelSource('${id}')" title="Switches crawling off until the label is cleared" type="button">Label and stop crawling</button>
      <button class="btn btn-quiet btn-sm" onclick="toggleSourceLabelForm('${id}')" type="button">Cancel</button>
    </div>`;
}

function toggleSourceLabelForm(listingSourceId) {
    if (!openSourceLabelForms.delete(listingSourceId)) openSourceLabelForms.add(listingSourceId);
    updateSourcePage();
}

async function labelSource(listingSourceId) {
    const label = document.getElementById(`sourceLabel-${listingSourceId}`)?.value;
    const note = document.getElementById(`sourceLabelNote-${listingSourceId}`)?.value.trim() || null;
    if (!label) return;
    try {
        await api(`/api/listing-sources/${encodeURIComponent(listingSourceId)}/label`,
            {method: 'PUT', body: JSON.stringify({label, note})});
        openSourceLabelForms.delete(listingSourceId);
        await loadSourcePage();
    } catch (error) {
        alert(apiErrorMessage(error));
    }
}

async function clearSourceLabel(listingSourceId) {
    try {
        await api(`/api/listing-sources/${encodeURIComponent(listingSourceId)}/label`, {method: 'DELETE'});
        await loadSourcePage();
    } catch (error) {
        alert(apiErrorMessage(error));
    }
}

// A source whose crawl waits for an operator returns to the spider's schedule once the operator
// has fixed what kept it failing.
async function resumeCrawl(listingSourceId) {
    try {
        await api(`/api/listing-sources/${encodeURIComponent(listingSourceId)}/crawl-resumption`, {method: 'POST'});
        await loadSourcePage();
    } catch (error) {
        alert(apiErrorMessage(error));
    }
}

// ───────────────────────────── Source rail of a review ─────────────────────────────
// The reviews of the selected review's ListingSource in every status, read per source: they feed
// the rail, the breadcrumb's count and the detection of a review that changed elsewhere.
let rail = {listingSourceId: null, items: undefined, total: 0, failure: '', source: null};
let railCollapsed = readRailCollapsed();

function readRailCollapsed() {
    try {
        return localStorage.getItem('crawlerReviewRailCollapsed') === 'true';
    } catch (_) {
        return false;
    }
}

function railHidden() {
    return railCollapsed || route.params.get('rail') === '0';
}

async function loadRail(listingSourceId = rail.listingSourceId) {
    if (!listingSourceId) return;
    if (rail.listingSourceId !== listingSourceId) {
        rail = {listingSourceId, items: undefined, total: 0, failure: '', source: null};
        sourceStatusWatch = null;
        sourceChange = null;
    }
    const id = encodeURIComponent(listingSourceId);
    const [reviews, source] = await Promise.allSettled([
        api(`/api/listing-sources/${id}/reviews?limit=200`),
        api(`/api/listing-sources/${id}`)
    ]);
    if (rail.listingSourceId !== listingSourceId) return;
    if (reviews.status === 'fulfilled') {
        rail = {...rail, items: reviews.value.items, total: reviews.value.total, failure: ''};
        updateSelectedReviewFreshness(reviews.value.items);
        noteRefresh('rail');
    } else {
        rail.failure = apiErrorMessage(reviews.reason);
        noteRefresh('rail', rail.failure);
    }
    if (source.status === 'fulfilled') {
        rail.source = source.value;
        watchSourceStatus(source.value);
    }
    updateRail();
}

// The ListingSource's status as the reviewer first saw it with the review open, so a source that
// starts failing meanwhile says so. Only a change into a condition is worth saying.
let sourceStatusWatch = null;
let sourceChange = null;

function watchSourceStatus(source) {
    const status = primaryStatus(source);
    if (sourceStatusWatch && sourceStatusWatch !== status.key && SOURCE_CONDITION_KEYS.has(status.key)) {
        sourceChange = {name: listingSourceName(source), text: status.text.toLowerCase(), detail: status.more};
        updateBanners();
    } else if (SOURCE_CONDITION_KEYS.has(status.key) === false && sourceStatusWatch !== status.key) {
        sourceChange = null;
        updateBanners();
    }
    sourceStatusWatch = status.key;
}

const SOURCE_CONDITION_KEYS = new Set(['crawl_needs_operator', 'crawl_failing', 'failing_to_scrape', 'blocked_by_review', 'removals_held']);

function renderSourceChange() {
    if (!sourceChange) return '';
    return notice('warn', 'Source', `${escapeHtml(sourceChange.name)} is now ${escapeHtml(sourceChange.text)}${sourceChange.detail ? ` · ${escapeHtml(sourceChange.detail)}` : ''}`,
        {role: 'status', dismiss: 'dismissSourceChange()'});
}

function dismissSourceChange() {
    sourceChange = null;
    updateBanners();
}

// The overview counts and the rail follow every refresh and every verdict.
async function refreshChrome() {
    await Promise.all([loadOverview(), loadRail()]);
}

function updateRail() {
    const container = document.getElementById('sourceRail');
    if (container) container.innerHTML = renderRailContent();
}

function renderRailItem(review, meta, badgeHtml) {
    const current = review.review_id === selectedId;
    return `<a class="rail-item" ${current ? 'aria-current="true" ' : ''}href="${reviewHash(review.review_id)}"><span class="rail-row"><span class="rail-title">${escapeHtml(artifactLabel(review.artifact_type))}</span>${badgeHtml}</span><span class="rail-meta">${meta}</span></a>`;
}

const CHEVRON_ICON = direction => `<svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="${direction === 'left' ? 'M10 4l-4 4 4 4' : 'M6 4l4 4-4 4'}"/></svg>`;

function renderRailContent() {
    if (railHidden()) {
        return `<button class="btn btn-quiet btn-icon btn-sm" onclick="setRailCollapsed(false)" aria-label="Show this source's reviews" title="Show this source's reviews" aria-expanded="false" type="button">${CHEVRON_ICON('right')}</button>`;
    }
    const name = selectedDetail?.review.listing_source_name || 'This source';
    const head = `<div class="card-head"><h3><span class="rail-source">${escapeHtml(name)}</span>${rail.source ? statusKeyBadge(primaryStatus(rail.source)) : ''}</h3>
      <button class="btn btn-quiet btn-icon btn-sm" onclick="setRailCollapsed(true)" aria-label="Hide this source's reviews" title="Hide" aria-expanded="true" type="button">${CHEVRON_ICON('left')}</button></div>`;
    if (rail.items === undefined) {
        return `${head}<div class="rail-body">${rail.failure ? stateBlock('error', 'Reviews unavailable', rail.failure) : stateBlock('loading', 'Loading')}</div>`;
    }
    const open = rail.items.filter(review => review.status === 'PENDING_REVIEW');
    const live = rail.items.filter(review => review.status === 'APPROVED' && review.candidate_is_live);
    const closed = rail.items.filter(review => !open.includes(review) && !live.includes(review));
    const shownClosed = closed.slice(0, 20);
    const group = (title, items) => items.length ? `<div class="rail-group"><div class="rail-group-title">${title}</div>${items.join('')}</div>` : '';
    const openItems = open.map(review => renderRailItem(review,
        `${escapeHtml(reasonWord(review.reason))} · ${timeBadge(review.created, 'Opened')}`,
        review.human_required ? badge('Needs a human', TONES.human_required)
            : review.repair_request ? badge('Repair', TONES.waiting_for_repair) : ''));
    const liveItems = live.map(review => renderRailItem(review, `approved ${timeBadge(review.updated, 'Approved')}`, badge('Live', TONES.live)));
    const closedItems = shownClosed.map(review => renderRailItem(review,
        `${review.rejection_revoked ? 'revoked · ' : ''}${timeBadge(review.updated, 'Closed')}`, statusBadge(review.status)));
    const more = closed.length > shownClosed.length || rail.total > rail.items.length
        ? `<div class="rail-foot"><a href="${sourceHash(rail.listingSourceId)}">All ${formatCount(rail.total)} reviews →</a></div>` : '';
    return `${head}<div class="rail-body">${group('Open', openItems)}${group('Live', liveItems)}${group('Closed', closedItems)}${more}</div>`;
}

function setRailCollapsed(collapsed) {
    railCollapsed = collapsed;
    try {
        localStorage.setItem('crawlerReviewRailCollapsed', String(collapsed));
    } catch (_) {
        // Nothing to keep.
    }
    const params = new URLSearchParams(route.params);
    if (collapsed) params.set('rail', '0');
    else params.delete('rail');
    route = {name: route.name, params};
    applyRailState();
    recordSelectionInUrl();
    document.querySelector('#sourceRail button')?.focus();
}

// The layout, the rail's width class and its content follow the collapsed state.
function applyRailState() {
    const hidden = railHidden();
    document.getElementById('reviewLayout')?.classList.toggle('rail-collapsed', hidden);
    document.getElementById('sourceRail')?.classList.toggle('rail-strip', hidden);
    updateRail();
}

function renderCrumbs(review) {
    const name = escapeHtml(review.listing_source_name || 'Source');
    return `<a href="${escapeHtmlAttr(lastReviewsHash)}">Reviews</a><span class="sep">/</span><a href="${sourceHash(review.listing_source_id)}">${name}</a><span class="sep">/</span><span>${escapeHtml(artifactLabel(review.artifact_type))}</span>`;
}

function updateCrumbs() {
    const crumbs = document.getElementById('reviewCrumbs');
    if (crumbs && selectedDetail) crumbs.innerHTML = renderCrumbs(selectedDetail.review);
}

// ───────────────────────────── Review ─────────────────────────────
// The one component for everything that is said next to the work: a tone (`bad`, `warn`, `ok`,
// `info`, `neutral`), a one- or two-word kind, the body as HTML its caller has escaped, and
// optionally actions or a dismiss button.
function notice(tone, kind, body, {role = '', actions = '', dismiss = ''} = {}) {
    return `<div class="banner banner-${tone}"${role ? ` role="${role}"` : ''}><span class="banner-kind">${escapeHtml(kind)}</span><div class="banner-body">${body}</div>${
        actions ? `<div class="banner-actions">${actions}</div>` : ''}${
        dismiss ? `<button class="btn btn-quiet btn-icon btn-sm banner-dismiss" onclick="${dismiss}" title="Dismiss" aria-label="Dismiss" type="button">${closeIcon()}</button>` : ''}</div>`;
}

// The review's banners, only those that apply, in the order of what to do first: changed elsewhere,
// the candidate error, a human is needed, the repair note, what is not evaluated, then the answer
// to the reviewer's last request.
function renderBanners(detail, matrix) {
    return [
        renderRefreshBanner(),
        renderSourceChange(),
        renderVerdictNotice(detail),
        renderClosedNotice(detail),
        renderCandidateError(detail),
        renderHumanRequired(detail),
        renderRepairRequest(detail),
        matrix ? renderMatrixNotice(detail, matrix) : '',
        renderAutoSchemaEvaluation(detail.review),
        renderKeptEdits(),
        renderEvaluatedNotice(),
        renderEditMessages()
    ].join('');
}

// What a closed review says about how it closed: by whom, when and why, and whether its approval is
// the live one.
function renderClosedNotice(detail) {
    const review = detail.review;
    if (review.status === 'PENDING_REVIEW' || verdictNotices.has(review.review_id)) return '';
    const events = detail.events || [];
    const closing = events.slice().reverse().find(event => ['APPROVED', 'REJECTED'].includes(event.action));
    const when = closing ? ` ${timeBadge(closing.created, 'Closed')} by ${escapeHtml(actorWord(closing.actor))}` : ` ${timeBadge(review.updated, 'Closed')}`;
    const notes = closing?.notes ? ` · “${escapeHtml(closing.notes)}”` : '';
    if (review.status === 'SUPERSEDED') return notice('neutral', 'Superseded', `Replaced by a later review ·${when}`);
    if (review.status === 'REJECTED') {
        const undo = review.artifact_type === 'URL_PATTERN' ? ' A rejected URL pattern cannot be undone; the next crawl opens a new review.' : '';
        return notice('neutral', 'Rejected', `${when.trim()}${notes}.${undo}`);
    }
    const live = review.artifact_type === 'PRODUCT_SCHEMA' ? Boolean(detail.candidate_is_live) : Boolean(railEntry(review.review_id)?.candidate_is_live);
    return notice('neutral', 'Approved', `${when.trim()}${notes}${live ? '' : ' · superseded by a later approval'}.`);
}

function railEntry(reviewId) {
    return (rail.items || []).find(review => review.review_id === reviewId);
}

function updateBanners() {
    const banners = document.getElementById('banners');
    if (banners && selectedDetail) banners.innerHTML = renderBanners(selectedDetail, selectedMatrix);
}

function lastChangeText(detail) {
    const events = detail.events || [];
    const latest = events[events.length - 1];
    return latest
        ? `last change ${timeBadge(latest.created, 'Last change')} by ${escapeHtml(actorWord(latest.actor))}`
        : `last change ${timeBadge(detail.review.updated, 'Last change')}`;
}

// ───────────────────────────── After a verdict ─────────────────────────────
// What the reviewer's verdict did and how to take it back or go on: the banner that replaces the
// plain "Approved" with Undo, and with the next review of the queue the review was opened from.
// The reviewer's verdicts of this session by review, so Undo stays offered when they come back.
const verdictNotices = new Map();

const VERDICT_WORDS = {
    approve: 'Approved', reject: 'Rejected', 'needs-repair': 'Repair requested',
    restore: 'Restored', 'revoke-rejection': 'Rejection revoked'
};

function renderVerdictNotice(detail) {
    const verdictNotice = verdictNotices.get(detail.review.review_id);
    if (!verdictNotice) return '';
    const review = detail.review;
    const name = escapeHtml(review.listing_source_name || 'the source');
    const actions = [];
    let body = '';
    if (verdictNotice.verdict === 'approve') {
        body = `Live for ${name}`;
        actions.push('<button class="btn btn-quiet btn-sm" onclick="restoreApproval()" title="Bring back what this approval replaced" type="button">Undo (restore previous)</button>');
    } else if (verdictNotice.verdict === 'reject') {
        if (rejectionRevocable(detail)) {
            body = `Rejected ${name}'s review`;
            actions.push('<button class="btn btn-quiet btn-sm" onclick="revokeRejection()" title="Lift the non-product verdict of this rejection" type="button">Undo (revoke rejection)</button>');
        } else {
            body = review.artifact_type === 'URL_PATTERN'
                ? 'Rejecting a URL pattern cannot be undone; the next crawl opens a new review.'
                : `Rejected ${name}'s review`;
        }
    } else {
        body = review.listing_source_name ? `${name}` : '';
    }
    const next = queue?.next;
    if (next) actions.push(`<a class="btn btn-primary btn-sm" href="${escapeHtmlAttr(queueItemHref(next))}">Next: ${escapeHtml(next.name)} →</a>`);
    return notice('ok', VERDICT_WORDS[verdictNotice.verdict] || 'Done', body, {role: 'status', actions: actions.join(''), dismiss: 'dismissVerdictNotice()'});
}

function dismissVerdictNotice() {
    verdictNotices.delete(selectedId);
    updateBanners();
}

// ───────────────────────────── Queue ─────────────────────────────
// A review opened from a list belongs to that list's queue: the fragment carries the list's query
// and the review's place in it, and the neighbours are read from the same list endpoint, so a list
// that shrinks under the reviewer still yields the right next review.
let queue = null;

function queueFromRoute() {
    const key = route.params.get('queue') || '';
    const index = Number(route.params.get('i'));
    if (!/^reviews(&|$)/.test(key) || !Number.isInteger(index) || index < 0) return null;
    return {key, index};
}

function queueLabel(params) {
    if (params.get('waiting') === 'human') return 'waiting for a human';
    if (params.get('waiting') === 'repair') return 'waiting for repair';
    if (params.get('status') === 'PENDING_REVIEW') return 'pending';
    return 'in this list';
}

async function loadQueue() {
    const place = queueFromRoute();
    if (!place) {
        queue = null;
        updateQueueBar();
        return;
    }
    const params = new URLSearchParams(place.key);
    const query = listApiQuery('reviews', params);
    const offset = Math.max(0, place.index - 1);
    query.set('limit', '3');
    query.set('offset', String(offset));
    try {
        const answer = await api(`/api/reviews?${query}`);
        const at = answer.items.findIndex(review => review.review_id === selectedId);
        const neighbour = (review, index) => review && {review_id: review.review_id, name: review.listing_source_name || 'Unnamed source', index};
        let position;
        let previous;
        let next;
        if (at >= 0) {
            position = offset + at;
            previous = neighbour(answer.items[at - 1], position - 1);
            next = neighbour(answer.items[at + 1], position + 1);
        } else {
            // The review left the list, so the one after it took its place.
            position = place.index;
            previous = place.index > 0 ? neighbour(answer.items[0], place.index - 1) : null;
            next = neighbour(answer.items[place.index > 0 ? 1 : 0], place.index);
        }
        queue = {key: place.key, label: queueLabel(params), position, total: answer.total, present: at >= 0, previous: previous || null, next: next || null};
    } catch (_) {
        // The queue is a convenience: without it the review is still worked as a single one.
        queue = null;
    }
    updateQueueBar();
    updateBanners();
}

function queueItemHref(item) {
    return reviewHash(item.review_id, {key: queue.key, index: item.index});
}

function queueBarHtml() {
    if (!queue) return '';
    const place = queue.present ? `<strong>${formatCount(queue.position + 1)} of ${formatCount(queue.total)}</strong>` : `<strong>${formatCount(queue.total)} left</strong>`;
    const offset = Math.floor(queue.position / PAGE_SIZE) * PAGE_SIZE;
    const back = `#${queue.key}${offset > 0 ? `&offset=${offset}` : ''}`;
    const previous = queue.previous
        ? `<a class="btn btn-sm" href="${escapeHtmlAttr(queueItemHref(queue.previous))}" title="Previous (K)">← Previous <span class="kbd">K</span></a>`
        : '<button class="btn btn-sm" disabled type="button">← Previous <span class="kbd">K</span></button>';
    const next = queue.next
        ? `<a class="btn btn-sm" href="${escapeHtmlAttr(queueItemHref(queue.next))}" title="Next (J)">Next: ${escapeHtml(queue.next.name)} → <span class="kbd">J</span></a>`
        : '<button class="btn btn-sm" disabled type="button">Next → <span class="kbd">J</span></button>';
    return `<div class="queue-bar" role="navigation" aria-label="Review queue"><span>${place} ${escapeHtml(queue.label)}</span><span class="muted">·</span><a href="${escapeHtmlAttr(back)}">back to the list</a>
      <span class="spacer"></span>
      <span class="kbd-hint"><span class="kbd">A</span> approve · <span class="kbd">R</span> reject · <span class="kbd">N</span> needs repair · <span class="kbd">E</span> evaluate</span>
      ${previous}${next}</div>`;
}

function updateQueueBar() {
    const bar = document.getElementById('queueBar');
    if (bar) bar.innerHTML = queueBarHtml();
}

function goQueue(step) {
    const item = step > 0 ? queue?.next : queue?.previous;
    if (item) location.hash = queueItemHref(item);
}

// ───────────────────────────── Keyboard ─────────────────────────────
// Shortcuts for working a queue with the keyboard. They are ignored while a field has the focus or
// a modifier is held, and a button or link keeps its own Enter.
let chordTimer = null;
let chord = '';

function typingTarget(target) {
    return target instanceof Element && target.closest('input, textarea, select, [contenteditable]');
}

function reviewPending() {
    return selectedDetail?.review.status === 'PENDING_REVIEW' && !pendingAction;
}

document.addEventListener('keydown', event => {
    if (event.defaultPrevented || event.ctrlKey || event.metaKey || event.altKey || typingTarget(event.target)) return;
    const key = event.key.toLowerCase();
    if (chord === 'g') {
        chord = '';
        const target = {o: '#overview', s: '#sources', r: '#reviews'}[key];
        if (target) {
            event.preventDefault();
            location.hash = key === 'r' ? lastReviewsHash : target;
        }
        return;
    }
    if (key === 'g') {
        chord = 'g';
        clearTimeout(chordTimer);
        chordTimer = setTimeout(() => { chord = ''; }, 1200);
        return;
    }
    if (key === '/' && listRouteKind()) {
        const search = document.querySelector('#listFilters input[type=search]');
        if (search) {
            event.preventDefault();
            search.focus();
        }
        return;
    }
    if (route.name !== 'review' || !selectedDetail) return;
    if (key === 'enter' && verdictNotices.has(selectedId) && queue?.next && !event.target.closest?.('button, a')) {
        event.preventDefault();
        goQueue(1);
    } else if (key === 'j' || key === 'k') {
        goQueue(key === 'j' ? 1 : -1);
    } else if (reviewPending() && key === 'a') {
        event.preventDefault();
        approveReview();
    } else if (reviewPending() && key === 'r') {
        event.preventDefault();
        rejectReview();
    } else if (reviewPending() && key === 'n') {
        event.preventDefault();
        needsRepair();
    } else if (key === 'e' && selectedMatrix && !evaluating) {
        event.preventDefault();
        reevaluateMatrix();
    }
});

// ───────────────────────────── Evaluating ─────────────────────────────
// A live evaluation fetches every review page of the shop at its crawl delay, which takes a while:
// the banner says it runs, the button stays off, and the coverage it replaces is dimmed, until the
// matrix lands and the result is said once.
let evaluating = null;
let evaluatedNotice = null;

function renderEvaluating() {
    const pages = evaluating.pages;
    return notice('info', 'Evaluating', `<span class="spinner" aria-hidden="true"></span>Fetching ${pages ? pluralize(pages, 'page', 'pages') : 'the pages'} from the shop at its crawl delay …`,
        {role: 'status', actions: '<button class="btn btn-sm" disabled type="button">Evaluating…</button>'});
}

function renderEvaluatedNotice() {
    if (evaluatedNotice?.reviewId !== selectedId) return '';
    return notice('info', 'Evaluated', escapeHtml(evaluatedNotice.text), {role: 'status', dismiss: 'dismissEvaluatedNotice()'});
}

function dismissEvaluatedNotice() {
    evaluatedNotice = null;
    updateBanners();
}

// The outcome of an evaluation, said when its matrix has been loaded.
function finishEvaluating(detail, matrix, failure) {
    if (evaluating?.reviewId !== detail.review.review_id) return;
    evaluating = null;
    if (failure || !matrix?.candidates?.length) return;
    const coverage = reviewPageCoverage(detail, matrix);
    evaluatedNotice = {
        reviewId: detail.review.review_id,
        text: `${pluralize(coverage.total, 'page', 'pages')} · covers ${coverage.covered} of ${coverage.total}`
    };
}

// ───────────────────────────── Unsaved edits across a conflict ─────────────────────────────
// When the candidate changed elsewhere, refreshing keeps what the reviewer typed: the fields they
// changed are applied to the new candidate as unsaved edits, and what could not be applied is said.
let candidateBaseline = null;
let keptEdits = null;

function sameJson(a, b) {
    return JSON.stringify(a ?? null) === JSON.stringify(b ?? null);
}

function cloneJson(value) {
    return value === undefined ? undefined : JSON.parse(JSON.stringify(value));
}

// The candidate `fresh` with the changes `draft` made to `base` applied: per schema field when the
// schema lists have the same length, as a whole otherwise. Returns the merged candidate and what
// could not be applied.
function mergeDraftCandidate(artifactType, base, draft, fresh) {
    const merged = cloneJson(fresh);
    const notes = [];
    if (!base) return {merged: cloneJson(draft), notes};
    if (artifactType === 'URL_PATTERN') {
        if (!sameJson(draft.decision, base.decision)) merged.decision = cloneJson(draft.decision);
        return {merged, notes};
    }
    if (!sameJson(draft.removed_page_schemas, base.removed_page_schemas)) {
        merged.removed_page_schemas = cloneJson(draft.removed_page_schemas || []);
    }
    const baseSchemas = base.schemas || [];
    const draftSchemas = draft.schemas || [];
    if (baseSchemas.length !== draftSchemas.length) {
        merged.schemas = cloneJson(draftSchemas);
        notes.push('You added or removed a schema, so your schema list replaced the new one.');
        return {merged, notes};
    }
    draftSchemas.forEach((schema, index) => {
        const before = baseSchemas[index] || {};
        const keys = new Set([...Object.keys(schema), ...Object.keys(before)].filter(key => key !== 'raw_attributes'));
        const changed = [...keys].filter(key => !sameJson(schema[key], before[key]));
        const rawKeys = new Set([...Object.keys(schema.raw_attributes || {}), ...Object.keys(before.raw_attributes || {})]);
        const rawChanged = [...rawKeys].filter(key => !sameJson((schema.raw_attributes || {})[key], (before.raw_attributes || {})[key]));
        if (!changed.length && !rawChanged.length) return;
        if (!merged.schemas?.[index]) {
            notes.push(`${schemaLabel(index)} no longer exists; your edits to it were dropped.`);
            return;
        }
        for (const key of changed) {
            if (schema[key] === undefined) delete merged.schemas[index][key];
            else merged.schemas[index][key] = cloneJson(schema[key]);
        }
        for (const key of rawChanged) {
            merged.schemas[index].raw_attributes = merged.schemas[index].raw_attributes || {};
            const value = (schema.raw_attributes || {})[key];
            if (value === undefined) delete merged.schemas[index].raw_attributes[key];
            else merged.schemas[index].raw_attributes[key] = cloneJson(value);
        }
    });
    return {merged, notes};
}

function renderKeptEdits() {
    if (keptEdits?.reviewId !== selectedId) return '';
    const notes = keptEdits.notes.map(note => ` ${escapeHtml(note)}`).join('');
    return notice('warn', 'Reloaded', `Your unsaved edits were kept; check them against the new candidate before saving.${notes}`,
        {role: 'status', dismiss: 'dismissKeptEdits()'});
}

function dismissKeptEdits() {
    keptEdits = null;
    updateBanners();
}

async function refreshKeepingEdits() {
    const review = selectedDetail.review;
    const draft = cloneJson(review.candidate_payload);
    const base = cloneJson(candidateBaseline);
    const labelDraft = sampleLabelDraft;
    const loaded = await submitEdit(async () => {
        await loadSelectedReview(false);
        await refreshChrome();
    });
    if (!loaded) return;
    const {merged, notes} = mergeDraftCandidate(review.artifact_type, base, draft, selectedDetail.review.candidate_payload);
    candidateBaseline = cloneJson(selectedDetail.review.candidate_payload);
    selectedDetail.review.candidate_payload = merged;
    sampleLabelDraft = labelDraft;
    dirty = true;
    keptEdits = {reviewId: review.review_id, notes};
    selectedReviewNeedsRefresh = false;
    selectedReviewRefreshReason = '';
    rerenderWorkbench();
}

function renderDetail(detail, matrix) {
    const review = detail.review;
    const urls = detail.urls || [];
    const live = review.status === 'APPROVED' && review.artifact_type === 'PRODUCT_SCHEMA' && Boolean(detail.candidate_is_live);
    const frameKept = writeDetail(`
      <nav class="crumbs" id="reviewCrumbs" aria-label="Breadcrumb">${renderCrumbs(review)}</nav>
      <div id="queueBar">${queueBarHtml()}</div>
      <div class="review-layout${railHidden() ? ' rail-collapsed' : ''}" id="reviewLayout">
        <aside class="card rail${railHidden() ? ' rail-strip' : ''}" id="sourceRail" aria-label="This source">${renderRailContent()}</aside>
        <div class="review">
          <div class="page-head">
            <div>
              <div class="page-title">
                <h1>${escapeHtml(artifactLabel(review.artifact_type))}</h1>
                ${statusBadge(review.status)}
                ${reviewFlags(review, {live, revoked: rejectionRevoked(detail)})}
              </div>
              <div class="page-meta">
                <span>${escapeHtml(reasonWord(review.reason))}</span><span>·</span>
                <span>opened ${timeBadge(review.created, 'Opened')}</span><span>·</span>
                <span>${lastChangeText(detail)}</span>
              </div>
            </div>
            <div class="page-actions">${renderApprovalPanel(detail)}</div>
          </div>
          <div class="banners" id="banners">${renderBanners(detail, matrix)}</div>
          <div id="actionPanel">${renderActionPanel()}</div>
          ${matrix ? renderSchemaWorkbench(detail, matrix) : renderNonSchemaReview(review, urls, detail)}
        </div>
      </div>`);
    if (matrix) highlightPreview(frameKept);
    syncPageActions();
    recordSelectionInUrl();
    updateReloadState();
}

const MATRIX_POLL_INTERVAL_MS = 3000;

// A refresh fetches every review page at the shop's robots.txt Crawl-delay, so the server
// answers 202 while it runs in the background; poll until the matrix or its error arrives. A read
// without refresh never fetches: without a stored matrix it is refused with `NO_MATRIX`, and the
// console asks for a refresh only when the reviewer does. `refreshed` tells whether a refresh,
// the console's own or one it joined, stored the matrix meanwhile.
async function loadSchemaMatrix(reviewId, refresh) {
    const matrix = await api(`/api/reviews/${reviewId}/matrix?${refresh ? 'refresh=true' : 'probe=true'}`);
    if (matrix?.status === 'none') return {matrix: null, refreshed: false, missing: true};
    const refreshed = refresh || matrix?.status === 'refreshing';
    return {matrix: await pollSchemaMatrix(reviewId, matrix), refreshed};
}

async function pollSchemaMatrix(reviewId, matrix) {
    if (matrix?.status !== 'refreshing') return matrix;
    if (selectedId !== reviewId) return null;
    if (selectedDetail?.review.review_id !== reviewId) {
        showPage(stateBlock('loading', matrix.message || 'Refreshing schema matrix…'));
    }
    await new Promise((resolve) => setTimeout(resolve, MATRIX_POLL_INTERVAL_MS));
    return pollSchemaMatrix(reviewId, await api(`/api/reviews/${reviewId}/matrix?probe=true`));
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
                const loaded = await loadSchemaMatrix(reviewId, refreshMatrix);
                if (loaded.missing) {
                    matrix = {review_id: reviewId, candidates: [], removed_page_matches: []};
                    matrixMissing = true;
                } else {
                    ({matrix, refreshed} = loaded);
                }
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
    loadRail(detail.review.listing_source_id);
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
    candidateBaseline = cloneJson(detail.review.candidate_payload);
    finishEvaluating(detail, matrix, matrixFailure);
    normalizeSchemaSelection();
    renderDetail(detail, matrix);
    loadQueue();
}

// Loading the preview fetches the shop page live, sends the shop another request and loses the
// preview's scroll and picked element, so a render keeps the frame while it shows the same
// page and replaces only the markup around it. Detaching the frame would reload it too, so it
// stays in the document while its ancestors take the new attributes and siblings, unless the
// reviewer asked to load the preview again. Answers whether the frame was kept.
let previewReloadRequested = false;

function writeDetail(html) {
    const container = document.getElementById('detail');
    container.dataset.route = 'review';
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
    return notice('bad', 'Cannot approve', `The candidate fails validation: ${escapeHtml(detail.candidate_error)}`, {role: 'alert'});
}

function renderApprovalPanel(detail) {
    const review = detail.review;
    if (review.status !== 'PENDING_REVIEW') {
        if (rejectionRevocable(detail)) {
            return '<button onclick="revokeRejection()" title="Lift this rejection\'s non-product verdict, so the crawler scrapes its seed and triggering pages again" type="button">Revoke rejection</button>';
        }
        if (rejectionRevoked(detail)) {
            return `<span class="badge badge-neutral" title="The review stays rejected, but its rejection no longer classifies its seed and triggering pages as non-product pages">Revoked</span>`;
        }
        if (review.status !== 'APPROVED') return '';
        const live = review.artifact_type === 'PRODUCT_SCHEMA' && !detail.candidate_is_live
            ? '<span class="badge badge-neutral" title="The live schemas changed since this review was approved">Read-only</span>'
            : '';
        const restore = approvalRestorable(detail)
            ? '<button onclick="restoreApproval()" title="Bring back the live configuration this approval replaced" type="button">Restore previous</button>'
            : '';
        return `${live}${restore}`;
    }
    return `<button class="btn" onclick="needsRepair()" title="Needs repair (N)" type="button">Needs repair</button>
      <button class="btn btn-danger" onclick="rejectReview()" title="Reject (R)" type="button">Reject</button>
      <button class="btn btn-primary" onclick="approveReview()" title="Approve (A)" type="button">Approve</button>`;
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
        return notice('neutral', 'Read-only', 'Closed review.');
    }
    return detail.candidate_is_live
        ? notice('neutral', 'Live schemas', 'Saving a field or the schema list changes the live schemas at once.')
        : notice('neutral', 'Read-only', 'The live schemas changed since this approval; edit the review whose candidate is live.');
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
    const label = `${pageRoleLabel(page.role)} · ${urlPath(page.url)}`;
    return page.not_product_reason ? `${label} · no product` : label;
}

// The agent's hand-over since a human last acted on the review, which makes it wait for a human
// whatever its rounds.
function handOverSinceHuman(detail) {
    const events = detail?.events || [];
    const lastHuman = events.map(event => event.actor).lastIndexOf('HUMAN');
    return events.slice(lastHuman + 1).reverse().find(event => event.action === 'HANDED_TO_HUMAN');
}

function renderHumanRequired(detail) {
    const review = detail?.review;
    if (!review?.human_required) return '';
    const rounds = Number(review.agent_rounds) || 0;
    const handOver = handOverSinceHuman(detail);
    const why = handOver
        ? `${escapeHtml(actorWord(handOver.actor))} handed this review to a human${handOver.notes ? `: “${escapeHtml(handOver.notes)}”` : ''}.`
        : `The agents gave ${escapeHtml(rounds)} needs-repair verdict${rounds === 1 ? '' : 's'} since a human last touched this review and stopped.`;
    return notice('bad', 'Needs a human', `${why} Any edit, page or label change, or a needs-repair note hands it back to them.`, {role: 'alert'});
}

function renderRepairRequest(detail) {
    const review = detail?.review;
    const request = review?.repair_request;
    if (!request || review.status !== 'PENDING_REVIEW') return '';
    const requested = (detail.events || []).slice().reverse().find(event => event.action === 'REPAIR_REQUESTED');
    return notice('warn', 'Repair note',
        `“${escapeHtml(request.note)}” ${timeBadge(request.requested, 'Repair requested')}${requested ? ` · ${escapeHtml(actorWord(requested.actor).replace(/^the /, ''))}` : ''}`);
}

const REVIEW_EVENT_LABELS = {
    CANDIDATE_EDITED: 'Candidate edited',
    SCHEMA_FIELD_EDITED: 'Field edited',
    PAGE_ADDED: 'Page added',
    PAGE_REMOVED: 'Page removed',
    PAGE_MARKED_NOT_PRODUCT: 'Page marked as no product',
    PAGE_NOT_PRODUCT_CLEARED: 'No-product mark cleared',
    NO_SOLD_PAGE_RECORDED: 'No sold page found',
    REPAIR_REQUESTED: 'Repair requested',
    HANDED_TO_HUMAN: 'Handed to a human',
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

// The review's event trail, newest first: each decision and edit with the actor that made it. A
// URL-pattern review keeps it under its body, closed; a product-schema review has it as a tab.
function renderReviewEvents(detail) {
    const events = detail?.events || [];
    if (!events.length) return '';
    const open = reviewEventsOpenReviewId === detail.review.review_id;
    return `<details class="card disclosure" ${open ? 'open' : ''} ontoggle="reviewEventsToggled(this.open)">
      <summary><span class="chev" aria-hidden="true">${chevronIcon()}</span><strong>History</strong><span class="muted">${events.length}</span></summary>
      ${renderEventList(events)}
    </details>`;
}

function renderHistoryPanel(detail) {
    const events = detail?.events || [];
    return `<div class="tab-panel">${events.length ? renderEventList(events) : stateBlock('empty', 'No history yet')}</div>`;
}

function renderEventList(events) {
    return `<ol class="events">${events.slice().reverse().map(renderReviewEvent).join('')}</ol>`;
}

function renderReviewEvent(event) {
    const summary = reviewEventSummary(event);
    const label = REVIEW_EVENT_LABELS[event.action] || displayName(event.action);
    return `<li class="event">
      ${actorBadge(event.actor)}
      <span><strong>${escapeHtml(label)}</strong>${summary ? ` <span class="muted">${escapeHtml(summary)}</span>` : ''}${event.notes ? `<br><span class="notes">${escapeHtml(event.notes)}</span>` : ''}</span>
      ${timeBadge(event.created, label)}
    </li>`
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
        case 'PAGE_NOT_PRODUCT_CLEARED':
            return details.url || '';
        case 'PAGE_MARKED_NOT_PRODUCT':
            return [details.url, details.reason].filter(Boolean).join(': ');
        case 'NO_SOLD_PAGE_RECORDED': {
            const checked = (details.checked_urls || []).length;
            return `${checked} page${checked === 1 ? '' : 's'} checked`;
        }
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
    return badge(displayName(actor), TONES[actor]);
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
    return notice(outcome.tone === 'ok' ? 'ok' : outcome.tone === 'bad' ? 'bad' : 'info', 'LLM evaluation',
        `<strong>${escapeHtml(outcome.text)}</strong> · ${escapeHtml(evaluation.summary || 'No summary provided.')}
        <div class="small muted">Model: ${escapeHtml(displayName(evaluation.decision || 'unknown').toLowerCase())} · confidence ${escapeHtml(String(evaluation.confidence || 'unknown').toLowerCase())} · deterministic gate ${escapeHtml(deterministicGateText(review))}${risks.length ? ` · risks: ${risks.map(risk => escapeHtml(risk)).join('; ')}` : ''}</div>`);
}

// A URL-pattern review: the decision, the sample URLs and the sample labels. They cover every
// edit the API accepts for this artifact.
function renderNonSchemaReview(review, urls, detail) {
    return `<div class="card"><div class="card-body stack">
      ${renderUrlPatternReview(review, urls)}
      ${renderSampleLabels(review)}
    </div></div>
    ${renderReviewEvents(detail)}`;
}

function urlPatternDecision(review) {
    return review?.candidate_payload?.decision || {type: 'UNDECIDED'};
}

function decisionPattern(decision) {
    return decision?.type === 'PATTERN' ? decision.value : '';
}

// The exclusion of a PATTERN decision, a regex for the non-items that share the product URL shape;
// the console shows it and keeps it across a save of the pattern, and the agent or the API sets it.
function decisionExclude(decision) {
    return decision?.type === 'PATTERN' ? (decision.exclude || '') : '';
}

// What a PATTERN decision is matched against: PATH_AND_QUERY_FULL_MATCH, the whole path and query,
// or FULL_URL_SEARCH, anywhere in the URL, which a decision without one has. The console keeps it
// across a save of the pattern, and the agent or the API sets it.
function decisionMatching(decision) {
    return decision?.type === 'PATTERN' && decision.matching ? decision.matching : 'FULL_URL_SEARCH';
}

function matchingNote(matching) {
    return matching === 'PATH_AND_QUERY_FULL_MATCH' ? ' <span class="small muted">whole path and query</span>' : '';
}

// The browser's regex of the decision's exclusion; none without one or when the browser cannot
// compile it.
function decisionExcludeRegex(decision) {
    const exclude = decisionExclude(decision);
    if (!exclude) return null;
    try {
        return new RegExp(exclude);
    } catch (_) {
        return null;
    }
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
let urlFilter = {reviewId: null, kind: 'all', text: '', page: 0};

const URL_FILTER_KINDS = [
    ['all', 'All'],
    ['product', 'Taken'],
    ['not_product', 'Not taken'],
    ['changed', 'Changed']
];

// The sample table shows 100 URLs at a time; the page starts over with each filter.
const URL_SAMPLE_PAGE_SIZE = 100;

// The decision and the sample URLs of a URL-pattern review. The counts and the table follow the
// pattern input while the reviewer types, as a browser preview, and show the server's
// classification of the saved candidate otherwise.
function renderUrlPatternReview(review, urls) {
    if (urlFilter.reviewId !== review.review_id) urlFilter = {reviewId: review.review_id, kind: 'all', text: '', page: 0};
    const view = urlPatternView(review, urls, urlPatternInputValue(review).trim());
    return `${renderUrlPatternDecisionSection(review, view)}
      ${urls.length ? `<div class="section">
        <div class="section-head"><h3>Sample URLs</h3><span class="small muted" id="urlPatternStats">${escapeHtml(urlPatternStatsText(view.counts))}</span></div>
        <div class="url-filter-bar">
          <div class="chips" id="urlFilterChips">${renderUrlFilterChips(view)}</div>
          <input id="urlFilterText" class="sm" type="search" value="${escapeHtmlAttr(urlFilter.text)}" placeholder="Filter paths" aria-label="Filter paths" style="max-width: 240px" oninput="setUrlFilterText(this.value)">
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
        const candidateMatch = preview.active
            ? regexTakesUrl(preview.regex, u.url, decisionExcludeRegex(urlPatternEditOf(review).stored), decisionMatching(urlPatternEditOf(review).stored))
            : u.candidate_pattern_match;
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
    return `${formatCount(counts.product)} of ${formatCount(counts.all)} taken by the candidate · ${formatCount(counts.changed)} change`;
}

function renderUrlPatternDecisionSection(review, view) {
    const decision = urlPatternEditOf(review).stored;
    const currentPattern = review.candidate_payload?.current_pattern;
    const currentExclude = review.candidate_payload?.current_exclude;
    const currentMatching = review.candidate_payload?.current_matching;
    const editable = urlPatternEditable(review);
    const live = currentPattern
        ? `Live: <code>${escapeHtml(currentPattern)}</code>${currentExclude ? ` excluding <code>${escapeHtml(currentExclude)}</code>` : ''}${matchingNote(currentMatching)}`
        : 'No live pattern yet.';
    return `<div class="section">
      <div class="section-head"><h3>Decision</h3>${renderUrlPatternDecision(decision)}</div>
      <div class="small muted">${live}</div>
      ${editable ? `<label class="field">Product URL regex
          <input id="urlPatternInput" class="mono" value="${escapeHtmlAttr(urlPatternInputValue(review))}" placeholder="e.g. /product/" oninput="markDirty(); previewUrlPattern()">
        </label>
        <div class="actions">
          <button class="btn" onclick="saveUrlPattern()" type="button">Save pattern</button>
          <button class="btn" onclick="saveNoUrlPattern()" type="button">Save as no pattern</button>
          <span class="small muted" id="urlPatternPreview">${escapeHtml(urlPatternPreviewText(view.preview))}</span>
        </div>
        ${handBackHint()}
        ${decision.type === 'UNDECIDED' ? notice('warn', 'Undecided', 'Enter a product URL regex or save no pattern before approving.') : ''}` : ''}
    </div>`;
}

function renderUrlPatternDecision(decision) {
    if (decision.type === 'PATTERN') return `<span class="decision">${badge('Pattern', 'ok')} <code>${escapeHtml(decision.value)}</code>${decision.exclude ? ` <span class="small muted">excluding</span> <code>${escapeHtml(decision.exclude)}</code>` : ''}${matchingNote(decisionMatching(decision))}</span>`;
    if (decision.type === 'NO_PATTERN') return badge('No pattern');
    return badge('Undecided', 'warn');
}

// Said only while the typed pattern differs from the saved one or does not compile.
function urlPatternPreviewText(preview) {
    if (preview.error) return `Invalid pattern: ${preview.error}`;
    return preview.active ? 'Preview of the unsaved pattern; saving validates it.' : '';
}

function renderUrlFilterChips(view) {
    return URL_FILTER_KINDS.map(([kind, label]) => `<button class="chip" aria-pressed="${urlFilter.kind === kind}" onclick="setUrlFilterKind('${kind}')" type="button">${escapeHtml(label)}</button>`).join('');
}

function urlRowShown(row) {
    if (urlFilter.kind === 'product' && !row.candidateProduct) return false;
    if (urlFilter.kind === 'not_product' && row.candidateProduct) return false;
    if (urlFilter.kind === 'changed' && !row.changed) return false;
    const text = urlFilter.text.trim().toLowerCase();
    return !text || row.path.toLowerCase().includes(text);
}

// Whether `regex` takes the page of `url` as the crawler matches a URL pattern: the URL as stored,
// or its path with or without the trailing slash (docs/crawler/networking.md#stored-urls), and the
// exclusion, when there is one, matches none of those spellings. Under PATH_AND_QUERY_FULL_MATCH
// the regex matches the whole path and query of a spelling and the exclusion anywhere in it.
function regexTakesUrl(regex, url, excludeRegex = null, matching = 'FULL_URL_SEARCH') {
    let spellings = pageSpellings(url);
    let include = regex;
    if (matching === 'PATH_AND_QUERY_FULL_MATCH') {
        spellings = spellings.map(pathAndQuery).filter(target => target !== null);
        include = new RegExp(`^(?:${regex.source})$`, regex.flags);
    }
    return spellings.some(spelling => include.test(spelling))
        && !(excludeRegex && spellings.some(spelling => excludeRegex.test(spelling)));
}

// The path and `?query` of `url`, without scheme, host, port or fragment; null for no URL.
function pathAndQuery(url) {
    try {
        const parsed = new URL(url);
        return `${parsed.pathname || '/'}${parsed.search.length > 1 ? parsed.search : ''}`;
    } catch (_) {
        return null;
    }
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
    urlFilter.page = 0;
    rerenderUrlPatternView();
}

function setUrlFilterText(text) {
    urlFilter.text = text || '';
    urlFilter.page = 0;
    rerenderUrlPatternView();
}

function setUrlSamplePage(page) {
    urlFilter.page = Math.max(0, page);
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
        sampleLabelStats: () => sampleLabelStatsHtml(currentSampleLabels(review)),
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
    const payload = selectedDetail.review?.candidate_payload;
    await saveCandidatePayload({
        decision,
        current_pattern: payload?.current_pattern ?? null,
        ...(payload?.current_exclude ? {current_exclude: payload.current_exclude} : {}),
        ...(payload?.current_matching ? {current_matching: payload.current_matching} : {})
    });
}

async function saveUrlPattern() {
    const value = document.getElementById('urlPatternInput').value.trim();
    if (!value) return;
    const stored = urlPatternEditOf(selectedDetail.review).stored;
    const exclude = decisionExclude(stored);
    await saveUrlPatternDecision({type: 'PATTERN', value, ...(exclude ? {exclude} : {}), matching: decisionMatching(stored)});
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
    const pages = Math.max(1, Math.ceil(rows.length / URL_SAMPLE_PAGE_SIZE));
    urlFilter.page = Math.min(urlFilter.page, pages - 1);
    const shown = rows.slice(urlFilter.page * URL_SAMPLE_PAGE_SIZE, (urlFilter.page + 1) * URL_SAMPLE_PAGE_SIZE);
    // The preview shows what the typed pattern takes; the label is the decision either way.
    const candidateHead = view.preview.active ? 'Candidate (preview)' : 'Candidate';
    const pager = rows.length > URL_SAMPLE_PAGE_SIZE ? `<div class="pager">
        <div class="pager-count"><strong>${formatCount(urlFilter.page * URL_SAMPLE_PAGE_SIZE + 1)}–${formatCount(urlFilter.page * URL_SAMPLE_PAGE_SIZE + shown.length)}</strong> of <strong>${formatCount(rows.length)}</strong></div>
        <div class="pager-buttons">
          <button class="btn btn-sm" ${urlFilter.page > 0 ? '' : 'disabled'} onclick="setUrlSamplePage(${urlFilter.page - 1})" type="button">Previous</button>
          <span class="pager-page">${urlFilter.page + 1} / ${pages}</span>
          <button class="btn btn-sm" ${urlFilter.page < pages - 1 ? '' : 'disabled'} onclick="setUrlSamplePage(${urlFilter.page + 1})" type="button">Next</button>
        </div></div>` : '';
    return `<div class="table-wrap"><table class="data-table compact url-table"><thead><tr><th>Path</th><th class="shrink">Now</th><th class="shrink">${candidateHead}</th><th class="shrink">Label</th></tr></thead><tbody>${
        shown.map(row => {
            const label = labels.find(l => l.url === row.url)?.label || '';
            const cell = editable
                ? renderSampleLabelSelect(label, `setSampleLabelOfSampleUrl(${row.index}, this.value)`, true)
                : (label ? badge(displayName(label)) : '');
            const path = isHttpUrl(row.url)
                ? `<a class="url" href="${escapeHtmlAttr(row.url)}" target="_blank" rel="noopener noreferrer" title="${escapeHtmlAttr(row.url)}">${escapeHtml(row.path)}</a>`
                : escapeHtml(row.path);
            return `<tr${row.changed ? ' class="changed"' : ''}>
              <td>${path}</td>
              <td>${patternMatchIcon(row.currentMatch)}</td>
              <td>${patternMatchIcon(row.candidateMatch)}</td>
              <td>${cell}</td>
            </tr>`;
        }).join('') || '<tr><td colspan="4" class="state-cell"><div class="state state-empty"><div class="state-title">No sample URL matches the filter</div></div></td></tr>'
    }</tbody></table></div>${pager}`;
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
    return `<select class="sm" onchange="${onchange}">${options.map(value =>
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
        return regexTakesUrl(new RegExp(pattern), url, decisionExcludeRegex(urlPatternEditOf(review).stored), decisionMatching(urlPatternEditOf(review).stored));
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

// The labels' counts and how many of them the decision in the pattern input contradicts.
function sampleLabelStatsHtml(labels) {
    const counts = labelCounts(labels);
    const inconsistent = labels.filter(label => {
        const takes = decisionPreviewTakes(label.url);
        return takes !== null && takes !== (label.label === 'PRODUCT');
    }).length;
    return `${counts.product} product · ${counts.not_product} not product${inconsistent ? ` · ${badge(`${inconsistent} inconsistent`, 'bad')}` : ''}`;
}

// A URL as its path with the host as the tooltip, as the sample table shows it.
function labelUrlText(url) {
    if (!isHttpUrl(url)) return escapeHtml(url);
    const origin = urlOrigin(url);
    return `<span class="url" title="${escapeHtmlAttr(url)}">${escapeHtml(relativeToOrigin(url, origin) || url)}</span>`;
}

// The author agent's or a human's labels of URLs on the review's crawl host, its sample's among
// them: which are product pages. An agent approval of the pattern rests on them alone; the
// reviewer agent cannot change them.
function renderSampleLabels(review) {
    if (review.artifact_type !== 'URL_PATTERN') return '';
    const labels = currentSampleLabels(review);
    const editable = sampleLabelsEditable(review);
    const host = urlOrigin(selectedDetail?.urls?.[0]?.url || labels[0]?.url || '').replace(/^https?:\/\//, '');
    const rows = labels.map((label, index) => `<tr>
          <td>${labelUrlText(label.url)}</td>
          <td>${editable ? renderSampleLabelSelect(label.label, `setSampleLabelKind(${index}, this.value)`, false) : badge(displayName(label.label))}</td>
          <td>${editable
            ? `<input class="sm" value="${escapeHtmlAttr(label.reason || '')}" placeholder="Why" aria-label="Reason" oninput="setSampleLabelReason(${index}, this.value)">`
            : escapeHtml(label.reason || '')}</td>
          <td id="sampleLabelTakes${index}">${renderSampleLabelTakes(label)}</td>
          <td>${editable ? `<button class="btn btn-quiet btn-danger btn-sm" onclick="removeSampleLabel(${index})" type="button">Remove</button>` : ''}</td>
        </tr>`).join('');
    const adder = editable ? `<div class="actions">
        <input id="sampleLabelUrl" class="sm" placeholder="Another URL${host ? ` on ${escapeHtmlAttr(host)}` : ''}" aria-label="URL to label" style="flex: 1 1 240px">
        ${renderSampleLabelSelect('PRODUCT', '', false).replace('<select', '<select id="sampleLabelKind" aria-label="Label"')}
        <input id="sampleLabelReason" class="sm" placeholder="Why" aria-label="Reason" style="flex: 1 1 160px">
        <button class="btn btn-sm" onclick="addSampleLabel()" type="button">Add label</button>
        <span class="spacer"></span>
        <button class="btn" onclick="saveSampleLabels()" type="button">Save labels</button>
      </div>${handBackHint()}` : '';
    return `<div class="section">
      <div class="section-head"><h3>Labels</h3><span class="small muted" id="sampleLabelStats">${sampleLabelStatsHtml(labels)}</span></div>
      ${labels.length ? `<div class="table-wrap"><table class="data-table compact"><thead><tr><th>URL</th><th class="shrink">Label</th><th>Reason</th><th class="shrink">Decision takes it</th><th class="shrink"></th></tr></thead><tbody>${rows}</tbody></table></div>` : ''}
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
        await refreshChrome();
    });
}

function renderSchemaWorkbench(detail, matrix) {
    const pages = detail.pages || [];
    const schemas = currentSchemas();
    const page = pages.find(p => p.review_page_id === selectedPageId) || pages[0];
    if (page && !selectedPageId) selectedPageId = page.review_page_id;
    const previewUrl = previewUrlOverride || (page ? page.url : '');
    const templates = currentRemovedPageTemplates().length;
    const dataIssues = groupFindingsByDescription(selectedCandidate(matrix)).size;
    return `
    <div class="workbench">
      <div class="card preview">
        <div class="preview-bar">
          <div class="preview-row">
            <label for="reviewPageSelect">Page</label>
            <select id="reviewPageSelect" aria-label="Review page" onchange="selectedPageId=this.value; previewUrlOverride=''; rerenderWorkbench()">
              ${pages.map(p => `<option value="${escapeHtmlAttr(p.review_page_id)}" ${p.review_page_id === selectedPageId ? 'selected' : ''} title="${escapeHtmlAttr(p.url)}">${escapeHtml(reviewPageLabel(p))}</option>`).join('')}
            </select>
            <button class="btn btn-quiet btn-danger" ${pageRemovable(page) ? '' : 'disabled'} onclick="removeReviewPage()" title="Removes a page a reviewer added or a page waiting on the review; seed pages stay" type="button">Remove page</button>
          </div>
          <div class="preview-row">
            <input id="livePreviewUrl" class="mono" value="${escapeHtmlAttr(previewUrl)}" placeholder="https://shop.example/product" aria-label="Preview URL" onkeydown="if(event.key==='Enter') loadPreviewUrl()">
            <button class="btn btn-icon" onclick="openPreviewUrl()" title="Open in a new tab" aria-label="Open in a new tab" type="button">${externalIcon()}</button>
            ${previewUrlOverride ? '<button class="btn btn-quiet" onclick="resetPreviewUrl()" type="button">Back to review page</button>' : ''}
            <button ${reviewPagesEditable(detail.review) ? '' : 'disabled'} onclick="addReviewPage()" title="Fetches this URL of the source and adds it to the review pages" type="button">Add to review</button>
          </div>
        </div>
        ${page?.not_product_reason ? `<div class="small muted">Marked as no product page: ${escapeHtml(page.not_product_reason)}</div>` : ''}
        ${renderPreviewFrame(page, previewUrl)}
      </div>
      <div class="card inspector">
        <div class="tabs" role="tablist">
          ${workbenchTab('selector', 'Selector')}
          ${workbenchTab('data', `Data${dataIssues ? ` <span class="badge badge-bad badge-count">${dataIssues}</span>` : ''}`)}
          ${workbenchTab('diff', 'Diff')}
          ${workbenchTab('removed', `Removed pages${templates ? ` <span class="count">${templates}</span>` : ''}`)}
          ${workbenchTab('history', `History${(detail.events || []).length ? ` <span class="count">${detail.events.length}</span>` : ''}`)}
        </div>
        ${renderWorkbenchPanel(detail, matrix, schemas)}
      </div>
    </div>`;
}

function renderPreviewFrame(page, previewUrl) {
    if (!previewUrl) {
        return '<div class="frame-placeholder">No review page yet — paste a product URL above and Add to review; the schema is tested on it.</div>';
    }
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
    const dim = key === 'data' && evaluating?.reviewId === selectedId;
    return `<button class="tab ${active ? 'active' : ''}${dim ? ' dimmed' : ''}" role="tab" aria-selected="${active}" onclick="setWorkbenchPanel('${key}')" type="button">${label}</button>`;
}

function renderWorkbenchPanel(detail, matrix, schemas) {
    if (activeWorkbenchPanel === 'data') return renderDataPanel(matrix);
    if (activeWorkbenchPanel === 'diff') return renderDiffPanel(schemas);
    if (activeWorkbenchPanel === 'removed') return renderRemovedPagePanel(detail, matrix);
    if (activeWorkbenchPanel === 'history') return renderHistoryPanel(detail);
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
        <label class="field">Field
          <select id="selectedFieldSelect" onchange="selectedField=this.value; rerenderWorkbench()">
            ${schemaFields(currentSchema()).map(field => `<option value="${escapeHtmlAttr(field)}" ${field === selectedField ? 'selected' : ''}>${escapeHtml(fieldLabel(field))}${fieldStateSuffix(matrixPage, field)}</option>`).join('')}
          </select>
        </label>
        <label class="field">Selector
          <span class="selector-row">
            <input id="selectedSelector" class="mono" ${disabled} value="${escapeHtmlAttr(currentSelector())}" oninput="setCurrentSelector(this.value); postHighlight(this.value)">
            <button class="btn btn-icon" onclick="postHighlight(currentSelector())" title="Highlight in the preview" aria-label="Highlight in the preview" type="button">${eyeIcon()}</button>
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
          <button class="btn" ${disabled} onclick="saveCurrentField()" type="button">Save field</button>
          <button class="btn btn-quiet btn-danger" ${disabled || (fieldDeletable(selectedField) ? '' : 'disabled')} onclick="deleteCurrentRule()" type="button">Delete rule</button>
          <span class="spacer"></span>
          ${dirty ? '<button class="btn btn-quiet" onclick="discardEdits()" type="button">Discard edits</button>' : ''}
        </div>
        ${handBackHint()}
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
      <div class="selector-row">
        <select id="newRawAttributeKey" aria-label="Raw attribute to add" title="A registry key the schema does not configure yet; adding it selects it as the field to edit" ${disabled}>
          <option value="">Add a raw attribute…</option>
          ${groups.map(group => `<optgroup label="${escapeHtmlAttr(displayName(group))}">${
            available.filter(entry => entry.group === group).map(entry =>
                `<option value="${escapeHtmlAttr(entry.key)}" title="${escapeHtmlAttr(entry.description)}">${escapeHtml(entry.key)}</option>`
            ).join('')}</optgroup>`).join('')}
        </select>
        <button class="btn" ${disabled} onclick="addRawAttribute()" title="Starts from the picked element when one is picked; save the field to store it" type="button">Add</button>
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
    const coverage = selectedMatrix ? reviewPageCoverage(selectedDetail, selectedMatrix) : null;
    const coverageBadge = coverage && coverage.total
        ? `<span class="badge badge-${coverageComplete(coverage) ? 'ok' : coverage.covered === 0 ? 'bad' : 'warn'}${evaluating?.reviewId === selectedId ? ' dimmed' : ''}" title="${escapeHtmlAttr(coverageText(coverage))}">Covers ${coverage.covered} of ${coverage.total} page${coverage.total === 1 ? '' : 's'}</span>` : '';
    return `<div class="section">
      <div class="section-head"><h3>Schemas</h3>${coverageBadge}</div>
      <div class="chips" title="Schemas are applied in this order">
        ${schemas.map((_, i) => `<button class="chip ${i === selectedSchemaIndex ? 'active' : ''}" aria-pressed="${i === selectedSchemaIndex}" onclick="selectedSchemaIndex=${i}; normalizeSchemaSelection(); rerenderWorkbench()" type="button">${schemaLabel(i)}</button>`).join('')}
        <button class="chip add" ${disabled} onclick="addNewSchema()" title="Adds a copy of the selected schema to rewrite for another template" type="button">+ Add</button>
      </div>
      <div class="toolbar">
        <button class="btn btn-quiet" ${disabled || (selectedSchemaIndex === 0 ? 'disabled' : '')} onclick="moveCurrentSchema(-1)" type="button">Move up</button>
        <button class="btn btn-quiet" ${disabled || (last ? 'disabled' : '')} onclick="moveCurrentSchema(1)" type="button">Move down</button>
        <button class="btn btn-quiet btn-danger" ${disabled || (schemas.length > 1 ? '' : 'disabled')} onclick="deleteCurrentSchema()" type="button">Delete</button>
        <span class="spacer"></span>
        <button class="btn btn-quiet" onclick="postHighlightSchema()" title="Highlights every rule of the selected schema in the preview" type="button">Highlight all</button>
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
        <button class="btn btn-icon" onclick="postHighlight(additionalSelectorAt(${index}))" title="Highlight in the preview" aria-label="Highlight in the preview" type="button">${eyeIcon()}</button>
        <button class="btn btn-icon" ${disabled} onclick="removeAdditionalSelector(${index})" title="Remove this fallback" aria-label="Remove this fallback" type="button">${closeIcon()}</button>
      </div>`).join('')}
      <div class="actions"><button class="btn btn-quiet" ${disabled} onclick="addAdditionalSelector()" type="button">+ Add fallback</button></div>
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
        <div class="section-head"><h3 title="A template declares a page removed: a selector bound to the notice, with text it contains or a regex. Approval replaces the stored templates, so removing one here retires it.">Removed-page templates</h3><span class="small muted">${templates.length}</span></div>
        ${editable ? '' : `<div class="small muted">${detail.review.status === 'APPROVED' ? 'Stored when approved; edit on a pending review.' : 'Closed; the stored templates are unchanged.'}</div>`}
        ${templates.map((template, index) => renderRemovedPageTemplate(template, index, disabled)).join('')
          || '<div class="small muted">No removed-page template.</div>'}
        <div class="actions">
          <button ${disabled} onclick="addRemovedPageTemplate()" title="Starts from the picked element when one is picked" type="button">+ Add template</button>
          <button class="btn" ${disabled} onclick="saveRemovedPageTemplates()" type="button">Save templates</button>
          <span class="spacer"></span>
          ${dirty && editable ? '<button class="btn btn-quiet" onclick="discardEdits()" type="button">Discard edits</button>' : ''}
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
          <button class="btn btn-icon" onclick="postHighlight(currentRemovedPageTemplates()[${index}].selector, 'Removed notice', '#dc2626')" title="Highlight in the preview" aria-label="Highlight in the preview" type="button">${eyeIcon()}</button>
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
        <button class="btn btn-quiet" ${disabled} onclick="usePickedForRemovedPageTemplate(${index})" type="button">Use picked element</button>
        <button class="btn btn-quiet btn-danger" ${disabled} onclick="removeRemovedPageTemplate(${index})" type="button">Remove</button>
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
    return `<div class="table-wrap"><table class="data-table compact">
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
        // A conflict is said once, by the "Changed elsewhere" banner, not again as a refusal that
        // repeats it with the server's sentence.
        const currentVersion = changedCandidateVersion(error);
        const coverage = refusedCoverage(error);
        editRefusal = currentVersion !== null || coverage ? null : {reviewId, message: apiErrorMessage(error)};
        if (currentVersion !== null && selectedId === reviewId) {
            selectedReviewNeedsRefresh = true;
            selectedReviewRefreshReason = 'The candidate changed elsewhere; your edits are not saved. Refresh keeps them for you to check against the new candidate.';
        }
        if (coverage && selectedId === reviewId) {
            selectedReviewNeedsRefresh = true;
            selectedReviewRefreshReason = `Not approved: the review's coverage is now ${coverageText({...coverage, notEvaluated: coverage.not_evaluated})}. Refresh to confirm its current coverage.`;
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
    showPage(stateBlock('error', 'The review could not be loaded', editRefusal?.message || '', '<button class="btn btn-sm" onclick="selectReview(selectedId)" type="button">Retry</button>'));
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
    updateReloadState();
    if (selectedDetail && Number.isInteger(saved?.candidate_version)) {
        selectedDetail.review.candidate_version = saved.candidate_version;
    }
}

function renderMatrixNotice(detail, matrix) {
    // A review without pages has nothing to evaluate, and a closed one is evaluated no more unless its
    // approval holds what is live now.
    if (!(detail.pages || []).length) return '';
    if (detail.review.status !== 'PENDING_REVIEW' && !(detail.review.status === 'APPROVED' && detail.candidate_is_live)) return '';
    if (evaluating?.reviewId === selectedId) return renderEvaluating();
    if (selectedMatrixMissing) {
        return notice('warn', 'Not evaluated', 'The candidate has not been evaluated on the live review pages.',
            {actions: '<button onclick="reevaluateMatrix()" type="button">Evaluate on live pages</button>'});
    }
    if (selectedMatrixFailure) {
        return notice('bad', 'Evaluation failed', escapeHtml(selectedMatrixFailure),
            {actions: '<button onclick="reevaluateMatrix()" type="button">Evaluate again</button>'});
    }
    const unevaluated = unevaluatedReviewPages(detail, matrix);
    if (!unevaluated.length) return '';
    return notice('warn', 'Not evaluated', `${unevaluated.length} page${unevaluated.length === 1 ? '' : 's'} joined after the last evaluation (${unevaluated.map(page => escapeHtml(reviewPageLabel(page))).join(', ')}).`,
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
    updateBanners();
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
        const body = JSON.parse(message);
        text = body.error || message;
        if (Number.isInteger(body.retry_after_secs)) text = `${text} Retry in ${body.retry_after_secs} s.`;
    } catch (_) {}
    if (error?.network) return `Request failed, the console server did not answer: ${text}`;
    if (Number.isInteger(error?.status)) {
        const status = `HTTP ${error.status}${error.statusText ? ` ${error.statusText}` : ''}`;
        return text ? `${status}: ${text}` : status;
    }
    return text;
}

// The review pages the candidate resolves, out of the review's product pages (those not marked as
// no product page, which the matrix leaves out), counted as the server
// counts them on approval: a page the removed-page templates capture as removed, or else one at
// least one schema applies to. A review page the matrix did not evaluate is not covered until the
// next evaluation, and without a matrix no page is evaluated.
function reviewPageCoverage(detail, matrix) {
    const evaluatedPages = matrix?.candidates?.[0]?.pages || [];
    const coverage = {covered: 0, removed: 0, total: 0, notEvaluated: 0};
    for (const page of (detail?.pages || []).filter(page => !page.not_product_reason)) {
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
    AVAILABILITY_TEXT: 'is an availability label, not a price',
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
    AVAILABILITY_TEXT: 'Availability text',
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
    return (detail.pages || []).filter(page => !page.not_product_reason && !evaluated.has(`persisted:${page.review_page_id}`));
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
    return `<div>${
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
    await refreshChrome();
}

// Re-evaluating reloads the review's candidate, so it asks before discarding unsaved edits,
// and with them a refusal of those edits. The matrix, or why there is none, shows its outcome.
async function reevaluateMatrix() {
    if (evaluating) return;
    await confirmDiscardingEdits('Re-evaluating', async () => {
        editNotice = null;
        evaluatedNotice = null;
        evaluating = {
            reviewId: selectedId,
            pages: (selectedDetail.pages || []).filter(page => !page.not_product_reason).length
        };
        rerenderWorkbench();
        try {
            return await reloadDiscardingEdits(reloadMatrix);
        } finally {
            if (evaluating) {
                evaluating = null;
                rerenderWorkbench();
            }
        }
    });
}

// Loading the current review discards the local edits, and with them a refusal of those edits.
async function refreshSelectedReview() {
    editNotice = null;
    pendingAction = null;
    if (dirty || sampleLabelDraft) return refreshKeepingEdits();
    return reloadDiscardingEdits(async () => {
        await loadSelectedReview(false);
        await refreshChrome();
    });
}

function schemasPayload() {
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
            ? (shownCoverage.total === 0
                ? 'This review has no pages, so nothing was tested. Approve anyway?'
                : `Coverage is incomplete: ${coverageText(shownCoverage)}. Approving confirms these counts.`)
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
            return submitVerdict('approve', verdict);
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
        run: notes => submitVerdict('restore', {notes})
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
        run: notes => submitVerdict('revoke-rejection', {notes})
    });
}

function rejectReview() {
    openActionPanel({
        title: 'Reject?',
        notes: 'optional',
        confirmLabel: 'Reject',
        danger: true,
        run: notes => submitVerdict('reject', {notes})
    });
}

function needsRepair() {
    openActionPanel({
        title: 'Needs repair',
        notes: 'required',
        notesLabel: 'What needs repair?',
        confirmLabel: 'Request repair',
        run: notes => submitVerdict('needs-repair', {notes})
    });
}

// A refused verdict leaves the review on screen as it was and shows why; only an accepted one
// reloads the review and the queue.
async function submitVerdict(verdict, body) {
    const reviewId = selectedId;
    return submitMutation(async () => {
        await api(`/api/reviews/${reviewId}/${verdict}`, {method: 'POST', body: JSON.stringify(body)});
        verdictNotices.set(reviewId, {verdict});
        evaluatedNotice = null;
        keptEdits = null;
    }, '', async () => {
        await refreshChrome();
        await loadSelectedReview(false);
        await loadQueue();
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
          <button id="actionPanelConfirm" class="btn ${action.danger ? 'btn-danger' : 'btn-primary'}" ${busy || blocked} onclick="confirmActionPanel()" type="button">${escapeHtml(action.busy ? 'Sending…' : action.confirmLabel)}</button>
          <button class="btn btn-quiet" ${busy} onclick="cancelActionPanel()" type="button">Cancel</button>
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
    await refreshChrome();
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
// Saving a change to a review the agents wait on hands it back to them; the hint under a Save
// button says so while there is something to save.
function handBackHint() {
    const review = selectedDetail?.review;
    if (review?.status !== 'PENDING_REVIEW' || !(review.human_required || review.repair_request)) return '';
    return `<div class="small muted hand-back"${dirty ? '' : ' hidden'}>Saving hands this review back to the agents.</div>`;
}

function updateReloadState() {
    document.querySelectorAll('.hand-back').forEach(hint => {
        hint.hidden = !dirty;
    });
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

// The rail's reviews of the selected review's ListingSource tell whether it changed elsewhere.
function updateSelectedReviewFreshness(reviews) {
    if (!selectedId || !selectedDetail) {
        selectedReviewNeedsRefresh = false;
        selectedReviewRefreshReason = '';
        updateReloadState();
        return;
    }
    const current = reviews.find(review => review.review_id === selectedId);
    if (current && selectedReviewStatus && current.status !== selectedReviewStatus) {
        selectedReviewNeedsRefresh = true;
        selectedReviewRefreshReason = `Status changed from ${displayName(selectedReviewStatus).toLowerCase()} to ${displayName(current.status).toLowerCase()}.`;
    } else if (current && !matrixRefreshesInFlight.has(selectedId) && isNewerTimestamp(current.updated, selectedReviewUpdatedAt)) {
        selectedReviewNeedsRefresh = true;
        selectedReviewRefreshReason = 'The review was updated elsewhere.';
    } else if (current) {
        selectedReviewNeedsRefresh = false;
        selectedReviewRefreshReason = '';
    }
    updateBanners();
    updateReloadState();
}

function renderRefreshBanner() {
    if (!selectedReviewNeedsRefresh) return '';
    return notice('warn', 'Changed elsewhere', escapeHtml(selectedReviewRefreshReason || 'The review changed elsewhere.'),
        {role: 'status', actions: `<button class="btn btn-sm" onclick="refreshSelectedReview()" type="button">${dirty || sampleLabelDraft ? 'Refresh · keep my edits' : 'Refresh review'}</button>`});
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


// A badge with the text `text` in the tone `tone`, one of `ok`, `warn`, `bad`, `info`; neutral
// without one.
function badge(text, tone) {
    const known = ['ok', 'warn', 'bad', 'info'].includes(tone) ? tone : 'neutral';
    return `<span class="badge badge-${known}">${escapeHtml(text)}</span>`;
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
    return timeSpan(value, bare ? 'bare' : 'ago', `${label}: ${formatDate(value)}`);
}

// The text of a moment relative to now, in one of the console's modes: `ago` ("5m ago"), `bare`
// ("5m", for a sentence that says "since" itself), `until` ("in 5m") and `due` (`until`, "due"
// once past).
function timeText(value, mode) {
    if (mode === 'bare') return elapsedTime(value).replace(/ ago$/, '');
    if (mode === 'until') return untilText(value);
    if (mode === 'due') return Date.parse(value) <= Date.now() ? 'due' : untilText(value);
    return elapsedTime(value);
}

// A relative time that keeps its moment and mode, so `retickTimes` brings it up to date between
// refreshes.
function timeSpan(value, mode, title) {
    if (!value) return '';
    return `<span class="time" data-time="${escapeHtmlAttr(value)}" data-mode="${mode}" title="${escapeHtmlAttr(title)}">${escapeHtml(timeText(value, mode))}</span>`;
}

function retickTimes() {
    for (const element of document.querySelectorAll('.time[data-time]')) {
        const text = timeText(element.dataset.time, element.dataset.mode);
        if (element.textContent !== text) element.textContent = text;
    }
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
    if (seconds < 45) return 'just now';
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

// ───────────────────────────── Navigation ─────────────────────────────
// A review's fragment names the review and, for a schema review, its page and schema, so reload,
// browser back and a shared link restore them: `#review=<id>&page=<id>&schema=<index>`, with the
// 0-based schema index that `schemaLabel` shows from 1, and `rail=0` while the source rail is
// hidden.
function selectionUrlHash() {
    if (!selectedId) return location.hash;
    const params = new URLSearchParams({review: selectedId});
    for (const name of ['queue', 'i']) {
        if (route.params.get(name)) params.set(name, route.params.get(name));
    }
    if (selectedMatrix && selectedDetail?.review.review_id === selectedId) {
        if (selectedPageId) params.set('page', selectedPageId);
        params.set('schema', String(selectedSchemaIndex));
    }
    if (railHidden()) params.set('rail', '0');
    return `#${params}`;
}

// A page or schema change within the review only updates the current history entry.
function recordSelectionInUrl() {
    if (selectedId && selectionUrlHash() !== location.hash) replaceSelectionInUrl();
}

function replaceSelectionInUrl() {
    history.replaceState(null, '', selectionUrlHash() || `${location.pathname}${location.search}`);
}

// `requested` names the page and schema a URL fragment asks for; without it the review opens on
// its first page and schema.
async function selectReview(id, requested = null) {
    selectedId = id;
    requestedSelection = requested;
    pickedSelector = '';
    pickedSample = '';
    dirty = false;
    sampleLabelDraft = null;
    editNotice = null;
    editRefusal = null;
    pendingAction = null;
    selectedDetail = null;
    selectedMatrix = null;
    selectedReviewNeedsRefresh = false;
    selectedReviewRefreshReason = '';
    evaluating = null;
    evaluatedNotice = null;
    keptEdits = null;
    queue = null;
    activeWorkbenchPanel = 'selector';
    showPage(stateBlock('loading', 'Loading review'));
    updateReloadState();
    await submitEdit(() => loadSelectedReview(true));
}

function clearSelectedReview() {
    queue = null;
    selectedId = null;
    selectedDetail = null;
    selectedMatrix = null;
    dirty = false;
    editNotice = null;
    editRefusal = null;
    pendingAction = null;
    selectedReviewNeedsRefresh = false;
    selectedReviewRefreshReason = '';
    rail = {listingSourceId: null, items: undefined, total: 0, failure: '', source: null};
    sourceStatusWatch = null;
    sourceChange = null;
    updateReloadState();
}

// Shows the route the fragment names. The two list routes read their current window, the source
// page its own reads, and a review its detail.
function openRoute(next) {
    rememberListReturn(next);
    route = next;
    listReturn = listRouteKind() && leftList?.hash === routeHash(route.name, route.params) ? leftList : null;
    if (next.name === 'reviews') lastReviewsHash = location.hash;
    if (next.name === 'review') {
        updateNav();
        loadOverview();
        return selectReview(next.params.get('review'), {
            pageId: next.params.get('page') || null,
            schemaIndex: /^\d+$/.test(next.params.get('schema') || '') ? Number(next.params.get('schema')) : null
        });
    }
    clearSelectedReview();
    updateNav();
    if (!listReturn) window.scrollTo(0, 0);
    if (next.name === 'source') {
        renderSourcePage();
        loadOverview();
        return loadSourcePage();
    }
    renderListPage();
    return Promise.all([loadOverview(), loadList(), next.name === 'overview' ? loadActivity() : null]);
}

function applyRouteFromUrl() {
    const requested = parseRoute();
    if (requested.name === 'review' && selectedId && requested.params.get('review') === selectedId) {
        route = requested;
        applyRequestedPageAndSchema({
            pageId: requested.params.get('page') || null,
            schemaIndex: /^\d+$/.test(requested.params.get('schema') || '') ? Number(requested.params.get('schema')) : null
        });
        updateCrumbs();
        applyRailState();
        return;
    }
    if (!dirty || !selectedId) return openRoute(requested);
    const requestedUrl = location.href;
    replaceSelectionInUrl();
    confirmDiscardingEdits(requested.name === 'review' ? 'Opening another review' : 'Leaving the review', () => {
        history.replaceState(null, '', requestedUrl);
        return openRoute(requested);
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

window.addEventListener('hashchange', applyRouteFromUrl);

// The header counts and the list on screen refresh calmly, and not at all while the tab is hidden;
// they refresh at once when the tab is shown again. A refresh never reloads the selected review.
const REFRESH_INTERVAL_MS = 15000;
let refreshTimer = null;

async function refreshView() {
    clearTimeout(refreshTimer);
    refreshTimer = null;
    try {
        const work = [loadOverview()];
        if (route.name === 'overview') work.push(loadActivity());
        if (listRouteKind()) work.push(loadList());
        else if (route.name === 'source') work.push(loadSourcePage());
        else if (route.name === 'review') work.push(loadRail(), loadQueue());
        await Promise.all(work);
        noteRefresh('view');
    } catch (error) {
        noteRefresh('view', apiErrorMessage(error));
    }
    scheduleRefresh();
}

function scheduleRefresh() {
    clearTimeout(refreshTimer);
    refreshTimer = document.hidden ? null : setTimeout(refreshView, REFRESH_INTERVAL_MS);
}

document.addEventListener('visibilitychange', () => {
    if (document.hidden) {
        clearTimeout(refreshTimer);
        refreshTimer = null;
    } else {
        refreshView();
    }
});

scheduleRefresh();
updateSignOutButton();
updateRefreshState();
applyRouteFromUrl();
