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
let collapsedListingSourceGroups = new Set(JSON.parse(
    localStorage.getItem('crawlerReviewCollapsedListingSourceGroups')
    || localStorage.getItem('crawlerReviewCollapsedShopGroups')
    || '[]'
));
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

applyTheme(theme);

function schemaLabel(index) {
    return `Schema ${index}`;
}

function applyTheme(nextTheme) {
    theme = nextTheme === 'dark' ? 'dark' : 'light';
    document.documentElement.dataset.theme = theme;
    localStorage.setItem('crawlerReviewTheme', theme);
    const toggle = document.getElementById('themeToggle');
    if (toggle) {
        toggle.textContent = theme === 'dark' ? 'Light mode' : 'Dark mode';
        toggle.setAttribute('aria-pressed', String(theme === 'dark'));
        toggle.title = theme === 'dark' ? 'Switch to light mode' : 'Switch to dark mode';
    }
}

function toggleTheme() {
    applyTheme(theme === 'dark' ? 'light' : 'dark');
}

function setReviewAuthToken(token) {
    reviewAuthToken = token || '';
    reviewSessionEstablished = false;
    if (reviewAuthToken) {
        sessionStorage.setItem('crawlerReviewAuthToken', reviewAuthToken);
    } else {
        sessionStorage.removeItem('crawlerReviewAuthToken');
    }
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
    document.getElementById('reloadState').textContent = 'Review token and browser session cleared';
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

async function loadReviews() {
    // The crawl state only adds to the queue, so its failure must not hide the reviews.
    const [reviews, listingSources] = await Promise.all([
        api('/api/reviews'),
        api('/api/listing_sources').catch(() => null)
    ]);
    showQueue(renderDomainsNeedingOperator(listingSources)
        + renderMissingValueSources(listingSources)
        + renderReviewGroups(reviews));
    updateSelectedReviewFreshness(reviews);
    return reviews;
}

// A refresh that changes nothing leaves the queue's elements alone, and one that does keeps the
// reviewer's scroll position and keyboard focus in it, such as on a group just toggled or a
// review just opened from the keyboard.
let shownQueueHtml = '';

function showQueue(html) {
    const list = document.getElementById('reviewList');
    if (html === shownQueueHtml && list.childElementCount) return;
    const scrollTop = list.scrollTop;
    const focused = list.contains(document.activeElement) ? queueControlKey(document.activeElement) : null;
    list.innerHTML = html;
    list.scrollTop = scrollTop;
    shownQueueHtml = html;
    if (focused) {
        Array.from(list.querySelectorAll('a, button'))
            .find(control => queueControlKey(control) === focused)
            ?.focus({preventScroll: true});
    }
}

// A queue control is named by what it opens or toggles, which a render does not change.
function queueControlKey(control) {
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
        return `<div class="empty-list">No reviews match the current search.</div>`;
    }

    return groups.map(group => {
        const pendingCount = group.reviews.filter(review => review.status === 'PENDING_REVIEW').length;
        const collapsed = collapsedListingSourceGroups.has(group.listingSourceId);
        const listId = `queueGroup-${escapeHtmlAttr(group.listingSourceId)}`;
        return `<div class="listing-source-group">
          <button type="button" class="listing-source-group-head" aria-expanded="${!collapsed}" aria-controls="${listId}" onclick="toggleListingSourceGroup('${escapeHtmlAttr(group.listingSourceId)}')">
            <span class="disclosure-icon" aria-hidden="true">${chevronIcon()}</span>
            <span class="group-text">
              <span class="group-title-row">
                <span class="queue-title">${escapeHtml(group.listingSourceName)}</span>
                ${pendingCount ? badge(`${pendingCount} pending`, 'pending') : '<span class="queue-meta">Reviewed</span>'}
              </span>
              <span class="queue-meta">${escapeHtml(reviewCountsByArtifactType(group.reviews))}${group.crawlEnabled ? '' : ' · crawling disabled'}</span>
            </span>
          </button>
          <div class="listing-source-review-list ${collapsed ? 'collapsed' : ''}" id="${listId}">
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

// A domain whose deterministic crawl failures kept repeating opens no review, so the queue
// lists it here until an operator fixes its configuration or disables crawling.
function renderDomainsNeedingOperator(listingSources) {
    if (listingSources === null) {
        return `<div class="empty-list">Domain crawl state is unavailable.</div>`;
    }
    const query = reviewSearchTerm.trim().toLowerCase();
    const domains = listingSources
        .filter(source => source.crawl_enabled)
        .flatMap(source => (source.domains || [])
            .filter(domain => domain.last_crawl_outcome === 'NEEDS_OPERATOR')
            .map(domain => ({...domain, listingSourceName: source.listing_source_name || source.listing_source_id})))
        .filter(domain => !query || [domain.domain, domain.listingSourceName, domain.last_crawl_error_kind]
            .some(value => String(value || '').toLowerCase().includes(query)));
    if (!domains.length) return '';
    return `<div class="listing-source-group">
      <div class="listing-source-group-head">
        <div>
          <div class="queue-title">Domains waiting for an operator</div>
          <div class="queue-meta">Not crawled again until re-registered or disabled</div>
        </div>
        <div class="listing-source-group-actions">${badge(`${domains.length} failed`, 'rejected')}</div>
      </div>
      <div class="listing-source-review-list">
        ${domains.map(domain => `<div class="item">
          <div class="queue-row">
            <span class="queue-artifact">${escapeHtml(domain.domain)}</span>
            ${badge('Needs operator')}
          </div>
          <div class="queue-meta">${escapeHtml(domain.listingSourceName)}</div>
          <div class="queue-meta">${escapeHtml(domain.last_crawl_error_kind || 'Unknown failure')} / ${escapeHtml(domain.crawl_failure_count)} consecutive failures</div>
        </div>`).join('')}
      </div>
    </div>`;
}

// A page whose last scrape left a required value empty opens and joins no review, such as a
// removal page that keeps an empty title element. The queue lists the ListingSources with such
// pages, so a reviewer adds one to the product-schema review (opened for it when none is
// pending) and declares a removed-page template for it there.
const expandedMissingValueSources = new Map();

function renderMissingValueSources(listingSources) {
    if (!listingSources) return '';
    const query = reviewSearchTerm.trim().toLowerCase();
    const sources = listingSources
        .filter(source => source.crawl_enabled && source.missing_value_urls > 0)
        .filter(source => !query || String(source.listing_source_name || source.listing_source_id).toLowerCase().includes(query));
    if (!sources.length) return '';
    return `<div class="listing-source-group">
      <div class="listing-source-group-head">
        <div>
          <div class="queue-title">Pages missing a required value</div>
          <div class="queue-meta">Open no review on their own; add one to a review to declare a removed-page template</div>
        </div>
      </div>
      <div class="listing-source-review-list">
        ${sources.map(source => {
            const id = escapeHtmlAttr(source.listing_source_id);
            const pages = expandedMissingValueSources.get(source.listing_source_id);
            return `<div class="item" onclick="toggleMissingValuePages('${id}')">
              <div class="queue-row">
                <span class="queue-artifact">${escapeHtml(source.listing_source_name || source.listing_source_id)}</span>
                ${badge(`${source.missing_value_urls} missing`, 'missing')}
              </div>
              ${pages ? renderMissingValuePages(source.listing_source_id, pages) : ''}
            </div>`;
        }).join('')}
      </div>
    </div>`;
}

// The handler names a page by its position in the listed pages, never by its URL: a shop's URL
// may hold a quote, which the browser decodes from an HTML entity before the handler runs, so a
// URL in the handler's source would let the shop run script in the console.
function renderMissingValuePages(listingSourceId, pages) {
    if (pages.error) return `<div class="queue-meta">${escapeHtml(pages.error)}</div>`;
    return pages.map((page, index) => `<div class="queue-meta missing-value-page" onclick="event.stopPropagation()">
        ${productPageLink(page.url)} ${escapeHtml(page.missing || '')}
        <button onclick="reviewMissingValuePage('${escapeHtmlAttr(listingSourceId)}', ${index})">Add to review</button>
      </div>`).join('');
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
    loadReviews().catch(err => document.getElementById('reviewList').textContent = err);
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
    loadReviews().catch(err => document.getElementById('reviewList').textContent = err);
}

function toggleListingSourceGroup(listingSourceId) {
    if (collapsedListingSourceGroups.has(listingSourceId)) {
        collapsedListingSourceGroups.delete(listingSourceId);
    } else {
        collapsedListingSourceGroups.add(listingSourceId);
    }
    localStorage.setItem('crawlerReviewCollapsedListingSourceGroups', JSON.stringify([...collapsedListingSourceGroups]));
    loadReviews().catch(err => document.getElementById('reviewList').textContent = err);
}

function renderReviewQueueItem(r) {
    const evaluation = autoSchemaEvaluation(r);
    const active = r.review_id === selectedId;
    // A link to the review's fragment: following it goes through the same hashchange path as
    // browser back, so it asks before discarding unsaved edits, and it opens in a new tab too.
    return `<a class="item ${active ? 'active' : ''}" href="#${new URLSearchParams({review: r.review_id})}" ${active ? 'aria-current="true"' : ''}>
      <span class="queue-row">
        <span class="queue-artifact">${escapeHtml(displayName(r.artifact_type))}</span>
        <span class="queue-badges">${statusBadge(r.status)}${repairBadge(r)}</span>
      </span>
      <span class="queue-meta">${escapeHtml(displayName(r.reason))} · ${timeBadge(r.created, 'Created')}</span>
      ${evaluation ? `<span class="queue-meta">${autoSchemaOutcomeBadge(evaluation, 'LLM evaluation: ')}</span>` : ''}
    </a>`;
}

// `requested` names the page and schema a URL fragment asks for; without it the review opens on
// its first page and schema.
async function selectReview(id, requested = null) {
    selectedId = id;
    requestedSelection = requested;
    pickedSelector = '';
    pickedSample = '';
    if (selectionFromUrl().reviewId !== id) recordSelectionInUrl();
    dirty = false;
    sampleLabelDraft = null;
    editNotice = null;
    pendingAction = null;
    activeWorkbenchPanel = 'selector';
    await loadReviews().catch(reportQueueFailure);
    await submitEdit(() => loadSelectedReview(true));
}

function reportQueueFailure(error) {
    document.getElementById('reloadState').textContent = `Queue refresh failed: ${apiErrorMessage(error)}`;
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

function renderDetail(detail, matrix) {
    const review = detail.review;
    const urls = detail.urls || [];
    const frameKept = writeDetail(`
    <div class="review-page">
      <div id="reviewRefreshBanner" class="refresh-banner" hidden></div>
      <div class="review-topbar">
        <div>
          <div class="review-title">
            <strong>${escapeHtml(review.listing_source_name || review.listing_source_id)}</strong>
            ${statusBadge(review.status)}
            ${repairBadge(review)}
            ${statusBadge(review.artifact_type)}
            ${review.listing_source_crawl_enabled === false ? statusBadge('Source disabled') : ''}
          </div>
          <div class="review-meta">
            <span>${escapeHtml(displayName(review.reason))}</span>
            <span>${escapeHtml(review.review_id)}</span>
            ${timeBadge(review.updated, 'Updated')}
          </div>
        </div>
        ${renderApprovalPanel(detail)}
      </div>
      <div id="actionPanel">${renderActionPanel()}</div>
      <div id="editMessages">${renderEditMessages()}</div>
      ${renderCandidateError(detail)}
      ${renderHumanRequired(review)}
      ${renderRepairRequest(review)}
      ${renderReviewEvents(detail)}
      ${matrix ? renderMatrixNotice(detail, matrix) : ''}
      ${renderAutoSchemaEvaluation(review)}
      ${matrix ? renderSchemaWorkbench(detail, matrix) : renderNonSchemaReview(review, urls)}
    </div>`);
    if (matrix) highlightPreview(frameKept);
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
    return `<div class="hint-card finding-hint" role="alert">
      <span>The candidate fails validation and cannot be approved until it is repaired: ${escapeHtml(detail.candidate_error)}</span>
    </div>`;
}

function renderApprovalPanel(detail) {
    const review = detail.review;
    if (review.status !== 'PENDING_REVIEW') {
        if (rejectionRevocable(detail)) {
            return '<div class="approval-actions"><button onclick="revokeRejection()" title="Lift this rejection\'s non-product verdict, so the crawler scrapes its seed and triggering pages again">Revoke rejection</button></div>';
        }
        if (rejectionRevoked(detail)) {
            return '<div class="approval-actions"><span class="badge" title="The review stays rejected, but its rejection no longer classifies its seed and triggering pages as non-product pages">Rejection revoked</span></div>';
        }
        if (review.status !== 'APPROVED') return '';
        const live = review.artifact_type === 'PRODUCT_SCHEMA'
            ? (detail.candidate_is_live
                ? statusBadge('Live schema editable')
                : '<span class="badge" title="The live schemas changed since this review was approved">Read-only: not the live schemas</span>')
            : '';
        const restore = approvalRestorable(detail)
            ? '<button onclick="restoreApproval()" title="Bring back the live configuration this approval replaced">Restore previous</button>'
            : '';
        return live || restore ? `<div class="approval-actions">${live}${restore}</div>` : '';
    }
    return `<div class="approval-actions">
      <button class="primary" onclick="approveReview()">Approve</button>
      <button class="danger" onclick="rejectReview()">Reject</button>
      <button onclick="needsRepair()">Needs repair</button>
    </div>`;
}

// Edits go to a pending review's candidate, or to the live schema list through the one approved
// review whose candidate is that list; the server refuses every other edit.
function candidateEditable(detail) {
    const review = detail?.review;
    if (!review) return false;
    if (review.status === 'PENDING_REVIEW') return true;
    return review.status === 'APPROVED' && review.artifact_type === 'PRODUCT_SCHEMA' && Boolean(detail.candidate_is_live);
}

function renderCandidateEditHint(detail) {
    const review = detail.review;
    if (review.status === 'PENDING_REVIEW') return '';
    if (review.status !== 'APPROVED') {
        return '<div class="hint-card">This review is closed; its candidate is read-only.</div>';
    }
    return detail.candidate_is_live
        ? '<div class="hint-card">This approved review holds the live schema list: saving a field or the schema list changes the live schemas at once.</div>'
        : '<div class="hint-card">The live schemas changed since this review was approved, so it is read-only. Edit the review whose candidate is live.</div>';
}

// The server takes reviewer pages on pending and approved product-schema reviews.
function reviewPagesEditable(review) {
    return review?.artifact_type === 'PRODUCT_SCHEMA' && ['PENDING_REVIEW', 'APPROVED'].includes(review.status);
}

function pageRemovable(page) {
    return Boolean(page) && REMOVABLE_PAGE_ROLES.has(page.role) && reviewPagesEditable(selectedDetail?.review);
}

function reviewPageLabel(page) {
    let path = page.url;
    try {
        path = new URL(page.url).pathname;
    } catch (_) {}
    return `${displayName(page.role)}: ${path}`;
}

function isWaitingForRepair(review) {
    return review?.status === 'PENDING_REVIEW' && Boolean(review.repair_request);
}

// A review whose agent needs-repair rounds reached the cap waits for a human, which outranks
// waiting for the repair the agents asked each other for.
function repairBadge(review) {
    if (review?.human_required) return badge('Needs a human', 'human');
    return isWaitingForRepair(review) ? badge('Waiting for repair', 'repair') : '';
}

function renderHumanRequired(review) {
    if (!review?.human_required) return '';
    const rounds = Number(review.agent_rounds) || 0;
    return `<div class="review-card human-required-card">
      <div class="panel-head">
        <div class="panel-title">
          <strong>Needs a human</strong>
          <span class="muted">The review agents gave ${escapeHtml(rounds)} needs-repair verdict${rounds === 1 ? '' : 's'} since a human last changed or judged this review and stopped working it. Edit it, change its pages or labels, or give needs-repair with what to do to hand it back to them; or approve or reject it.</span>
        </div>
      </div>
    </div>`;
}

function renderRepairRequest(review) {
    const request = review?.repair_request;
    if (!request) return '';
    const title = isWaitingForRepair(review) ? 'Waiting for repair' : 'Repair requested';
    return `<div class="review-card repair-request-card">
      <div class="panel-head">
        <div class="panel-title">
          <strong>${title}</strong>
          <span class="muted">${escapeHtml(request.note)}</span>
        </div>
        ${timeBadge(request.requested, 'Repair requested')}
      </div>
    </div>`;
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
    return `<details class="review-card review-events-card" ${open ? 'open' : ''} ontoggle="reviewEventsToggled(this.open)">
      <summary><span class="disclosure-icon" aria-hidden="true">${chevronIcon()}</span><strong>History</strong><span class="muted">${events.length} event${events.length === 1 ? '' : 's'}, latest ${escapeHtml(REVIEW_EVENT_LABELS[events[events.length - 1].action] || displayName(events[events.length - 1].action)).toLowerCase()} ${timeBadge(events[events.length - 1].created, 'Latest change')}</span></summary>
      <ol class="review-events">${events.slice().reverse().map(renderReviewEvent).join('')}</ol>
    </details>`;
}

function renderReviewEvent(event) {
    const summary = reviewEventSummary(event);
    return `<li class="review-event">
      ${actorBadge(event.actor)}
      <strong>${escapeHtml(REVIEW_EVENT_LABELS[event.action] || displayName(event.action))}</strong>
      ${summary ? `<span class="muted">${escapeHtml(summary)}</span>` : ''}
      ${event.notes ? `<span class="review-event-notes">${escapeHtml(event.notes)}</span>` : ''}
      <span class="muted">version ${escapeHtml(event.candidate_version)}</span>
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

function actorBadge(actor) {
    const kind = {AGENT_AUTHOR: 'agent', AGENT_REVIEWER: 'agent', SYSTEM: 'system'}[actor] || 'human';
    return `<span class="badge badge-actor-${kind}">${escapeHtml(displayName(actor))}</span>`;
}

function autoSchemaEvaluation(review) {
    return (((review || {}).validation_summary || {}).auto_schema_evaluation) || null;
}

// The one outcome of the crawler's LLM evaluation of a generated schema: auto-approved when the
// crawler approved it itself (`approved_by_llm`: in `auto` mode, on the model's high-confidence
// approval and a passed deterministic gate), rejected when the model rejected it, otherwise left
// to a human. The model's own decision and confidence and the gate's result are shown apart from
// it, labelled, so they never read as a second outcome.
function autoSchemaOutcome(evaluation) {
    if (evaluation.approved_by_llm) return {text: 'Auto-approved', tone: 'approved'};
    if (evaluation.decision === 'REJECT') return {text: 'Rejected by the model', tone: 'rejected'};
    return {text: 'Needs human review', tone: 'pending'};
}

// The queue item labels the outcome as the LLM evaluation's, so a closed review's status badge
// beside it never reads as a second outcome of the review.
function autoSchemaOutcomeBadge(evaluation, prefix = '') {
    const outcome = autoSchemaOutcome(evaluation);
    return badge(`${prefix}${outcome.text}`, outcome.tone);
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
    return `<div class="review-card llm-evaluation-card">
      <div class="panel-head">
        <div class="panel-title">
          <strong>LLM Schema Evaluation</strong>
          <span class="muted">${escapeHtml(evaluation.summary || 'No summary provided.')}</span>
          <span class="llm-evaluation-inputs">
            <span>Model decision: ${escapeHtml(displayName(evaluation.decision || 'unknown'))}</span>
            <span>Model confidence: ${escapeHtml(displayName(evaluation.confidence || 'unknown'))}</span>
            <span>Deterministic gate: ${escapeHtml(deterministicGateText(review))}</span>
          </span>
        </div>
        <div class="llm-evaluation-badges">${autoSchemaOutcomeBadge(evaluation)}</div>
      </div>
      ${risks.length ? `<div class="llm-risks">${risks.map(risk => `<span>${escapeHtml(risk)}</span>`).join('')}</div>` : ''}
    </div>`;
}

// The review whose raw payload the reviewer opened, so a re-render, such as the one a refused
// full-JSON save or a sample label change makes, keeps it open.
let rawPayloadOpenReviewId = null;

function rawPayloadToggled(open) {
    rawPayloadOpenReviewId = open ? selectedId : null;
}

function renderNonSchemaReview(review, urls) {
    const urlPattern = review.artifact_type === 'URL_PATTERN';
    const open = !urlPattern || rawPayloadOpenReviewId === review.review_id;
    return `<div class="review-card">
      <div class="tab-panel">
        ${urlPattern || urls.length ? renderUrlPatternReview(review, urls) : ''}
        ${renderSampleLabels(review)}
        <details class="raw-payload" ${open ? 'open' : ''} ontoggle="rawPayloadToggled(this.open)">
          <summary>Raw payload</summary>
          <h3>Candidate Payload</h3>
          <textarea id="candidatePayload" oninput="markDirty()">${escapeHtml(JSON.stringify(review.candidate_payload, null, 2))}</textarea>
          <div class="actions"><button onclick="saveCandidate()">Save full JSON</button></div>
          <h3>Validation</h3>
          <pre>${escapeHtml(JSON.stringify(review.validation_summary, null, 2))}</pre>
        </details>
      </div>
    </div>`;
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

// The summary, the decision and the sample URLs of a URL-pattern review. The summary and the table
// follow the pattern input while the reviewer types, as a browser preview, and show the server's
// classification of the saved candidate otherwise.
function renderUrlPatternReview(review, urls) {
    if (urlFilter.reviewId !== review.review_id) urlFilter = {reviewId: review.review_id, kind: 'all', text: ''};
    const view = urlPatternView(review, urls, urlPatternInputValue(review).trim());
    return `<div id="urlPatternSummary">${renderUrlPatternSummary(review, view)}</div>
      ${renderUrlPatternEditor(review)}
      ${urls.length ? `<h3>Sample URLs</h3>
        <div class="url-filter-bar">
          <div class="url-filter-chips" id="urlFilterChips">${renderUrlFilterChips(view)}</div>
          <input id="urlFilterText" type="search" value="${escapeHtmlAttr(urlFilter.text)}" placeholder="Filter paths" oninput="setUrlFilterText(this.value)">
        </div>
        <div id="urlPatternTable">${renderUrls(review, view)}</div>` : ''}`;
}

// How each sample URL classifies under the domain's current pattern and under the candidate: the
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

function renderUrlPatternSummary(review, view) {
    const {counts, preview} = view;
    const currentPattern = review.candidate_payload?.current_pattern;
    const candidateLabel = preview.active ? 'Product URLs (preview)' : 'Product URLs (candidate)';
    return `<div class="url-pattern-summary">
      <div class="url-pattern-decision">
        <span class="muted">Decision</span>
        ${renderUrlPatternDecision(urlPatternEditOf(review).stored)}
        <span class="muted">Current pattern</span>
        ${currentPattern ? `<code>${escapeHtml(currentPattern)}</code>` : '<span class="muted">none</span>'}
      </div>
      <div class="url-pattern-stats">
        ${urlPatternStat('Sample URLs', counts.all)}
        ${urlPatternStat('Product URLs (current)', counts.current_product)}
        ${urlPatternStat(candidateLabel, counts.product)}
        ${urlPatternStat('Changed URLs', counts.changed)}
      </div>
      <div class="muted" id="urlPatternPreview">${escapeHtml(urlPatternPreviewText(preview))}</div>
    </div>`;
}

function urlPatternStat(label, value) {
    return `<div class="url-pattern-stat"><strong>${escapeHtml(value)}</strong><span>${escapeHtml(label)}</span></div>`;
}

// One element, so the badge and the pattern share the summary grid's cell.
function renderUrlPatternDecision(decision) {
    if (decision.type === 'PATTERN') return `<span>${badge('Pattern')} <code>${escapeHtml(decision.value)}</code></span>`;
    if (decision.type === 'NO_PATTERN') return badge('No pattern');
    return badge(displayName(decision.type), 'pending');
}

function urlPatternPreviewText(preview) {
    if (preview.error) return `Invalid pattern: ${preview.error}. The counts and the table show the saved candidate.`;
    if (!preview.active) return 'Server classification of the saved candidate.';
    return 'Browser preview of the unsaved pattern; the server validates and classifies it on save.';
}

function renderUrlPatternEditor(review) {
    if (!urlPatternEditable(review)) return '';
    const decision = urlPatternEditOf(review).stored;
    return `<div class="url-pattern-editor">
      <div class="hint-card">${decision.type === 'UNDECIDED'
        ? 'Enter a product URL regex or save NO_PATTERN before approving.'
        : 'Save a new decision to change it.'}</div>
      <input id="urlPatternInput" value="${escapeHtmlAttr(urlPatternInputValue(review))}" placeholder="Product URL regex, e.g. /product/" oninput="markDirty(); previewUrlPattern()">
      <div class="actions">
        <button onclick="saveUrlPattern()">Save pattern</button>
        <button onclick="saveNoUrlPattern()">Save NO_PATTERN</button>
      </div>
    </div>`;
}

function renderUrlFilterChips(view) {
    return URL_FILTER_KINDS.map(([kind, label]) => `<button class="schema-chip ${urlFilter.kind === kind ? 'active' : ''}" onclick="setUrlFilterKind('${kind}')">${escapeHtml(label)} ${view.counts[kind]}</button>`).join('');
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

// Renders the summary, the filter counts, the table and the sample labels' "Decision takes it"
// cells again, leaving the inputs alone so they keep their focus and what was typed into them.
function rerenderUrlPatternView() {
    if (!selectedDetail) return;
    const review = selectedDetail.review;
    const view = urlPatternView(review, selectedDetail.urls || [], urlPatternInputValue(review).trim());
    const parts = {
        urlPatternSummary: () => renderUrlPatternSummary(review, view),
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
    return `<div class="data-grid-wrap"><table class="url-table"><thead><tr><th>Path</th><th>Current</th><th>${candidateHead}</th><th>${classHead}</th><th>Label</th></tr></thead><tbody>${
        rows.map(row => {
            const label = labels.find(l => l.url === row.url)?.label || '';
            const cell = editable
                ? renderSampleLabelSelect(label, `setSampleLabelOfSampleUrl(${row.index}, this.value)`, true)
                : (label ? statusBadge(label) : '');
            const path = isHttpUrl(row.url)
                ? `<a class="product-url" href="${escapeHtmlAttr(row.url)}" target="_blank" rel="noopener noreferrer" title="${escapeHtmlAttr(row.url)}">${escapeHtml(row.path)}</a>`
                : escapeHtml(row.path);
            return `<tr class="${row.changed ? 'url-row-changed' : ''}">
              <td>${path}</td>
              <td>${patternMatchIcon(row.currentMatch)}</td>
              <td>${patternMatchIcon(row.candidateMatch)}</td>
              <td>${row.candidateClass ? badge(displayName(row.candidateClass), row.candidateClass === 'product' ? 'approved' : undefined) : ''}</td>
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
    return `${counts?.product ?? 0} PRODUCT, ${counts?.not_product ?? 0} NOT_PRODUCT`;
}

function renderSampleLabelSelect(selected, onchange, allowNone) {
    const options = (allowNone ? [''] : []).concat(SAMPLE_URL_LABELS);
    return `<select onchange="${onchange}">${options.map(value =>
        `<option value="${escapeHtmlAttr(value)}" ${value === selected ? 'selected' : ''}>${escapeHtml(value || '-')}</option>`
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
    const takesText = takes === null ? '' : (takes ? 'yes' : 'no');
    const consistent = takes === null || takes === (label.label === 'PRODUCT');
    return consistent ? escapeHtml(takesText) : badge(displayName(`${takesText}: inconsistent`));
}

// The author agent's or a human's labels of URLs of the review's domain, its sample's among
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
          <td>${editable ? renderSampleLabelSelect(label.label, `setSampleLabelKind(${index}, this.value)`, false) : statusBadge(label.label)}</td>
          <td>${editable
            ? `<input value="${escapeHtmlAttr(label.reason || '')}" placeholder="Why" oninput="setSampleLabelReason(${index}, this.value)">`
            : escapeHtml(label.reason || '')}</td>
          <td id="sampleLabelTakes${index}">${renderSampleLabelTakes(label)}</td>
          <td>${editable ? `<button onclick="removeSampleLabel(${index})">Remove</button>` : ''}</td>
        </tr>`).join('');
    const adder = editable ? `<div class="actions">
        <input id="sampleLabelUrl" placeholder="Another URL of the review's domain">
        ${renderSampleLabelSelect('PRODUCT', '', false).replace('<select', '<select id="sampleLabelKind"')}
        <input id="sampleLabelReason" placeholder="Why">
        <button onclick="addSampleLabel()">Add label</button>
      </div>
      <div class="actions"><button class="primary" onclick="saveSampleLabels()">Save labels</button></div>` : '';
    return `<h3>Sample Labels</h3>
      <div class="hint-card">${escapeHtml(sampleLabelCountsText(counts))} (${status}). The reviewer agent's approval needs enough labels of each and a decision that takes every PRODUCT URL and no NOT_PRODUCT URL; schema.org Product markup decides nothing.</div>
      ${labels.length ? `<div class="data-grid-wrap"><table><thead><tr><th>URL</th><th>Label</th><th>Reason</th><th>Decision takes it</th><th></th></tr></thead><tbody>${rows}</tbody></table></div>` : ''}
      ${adder}`;
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
    }, 'Sample labels saved.', async () => {
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
    return `
    <div class="schema-workbench">
      <div class="preview-panel">
        <div class="panel-head preview-head">
          <div class="preview-head-row">
            <strong>Product View</strong>
            <div class="preview-page-picker">
              <select aria-label="Review page" onchange="selectedPageId=this.value; previewUrlOverride=''; rerenderWorkbench()">
                ${pages.map(p => `<option value="${escapeHtmlAttr(p.review_page_id)}" ${p.review_page_id === selectedPageId ? 'selected' : ''} title="${escapeHtmlAttr(p.url)}">${escapeHtml(reviewPageLabel(p))}</option>`).join('')}
              </select>
              <button ${pageRemovable(page) ? '' : 'disabled'} onclick="removeReviewPage()" title="Removes a page a reviewer added or a page waiting on the review; seed pages stay">Remove page</button>
            </div>
          </div>
          <div class="preview-url-row muted">${previewUrl
            ? `${productPageLink(previewUrl)}<span class="preview-url-kind">(${previewUrlOverride ? 'custom URL' : `${escapeHtml(page.role)}, live HTML`})</span>`
            : 'No live page available'}</div>
        </div>
        <div class="live-url-bar">
          <input id="livePreviewUrl" value="${escapeHtmlAttr(previewUrl)}" placeholder="https://example.com/product" onkeydown="if(event.key==='Enter') loadPreviewUrl()">
          <button onclick="loadPreviewUrl()">Open URL</button>
          <button ${page ? '' : 'disabled'} onclick="resetPreviewUrl()">Use review URL</button>
          <button ${reviewPagesEditable(detail.review) ? '' : 'disabled'} onclick="addReviewPage()" title="Fetches this URL of the ListingSource and adds it to the review pages, so the matrix evaluates every schema on it">Add to review pages</button>
        </div>
        ${renderPreviewFrame(page, previewUrl)}
      </div>
      <div class="right-panel">
        <div class="tabs">
          ${workbenchTab('selector', 'Selector')}
          ${workbenchTab('data', 'Data')}
          ${workbenchTab('diff', 'Schema diff')}
          ${workbenchTab('removed', `Removed page${currentRemovedPageTemplates().length ? ` (${currentRemovedPageTemplates().length})` : ''}`)}
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

// The reviewer asks for the preview with these buttons, so they load it again even when it
// already shows that URL, for example after the shop refused the first fetch.
function loadPreviewUrl() {
    const input = document.getElementById('livePreviewUrl');
    previewUrlOverride = (input && input.value || '').trim();
    previewReloadRequested = true;
    rerenderWorkbench();
}

function resetPreviewUrl() {
    previewUrlOverride = '';
    previewReloadRequested = true;
    rerenderWorkbench();
}

function workbenchTab(key, label) {
    return `<button class="tab ${activeWorkbenchPanel === key ? 'active' : ''}" onclick="setWorkbenchPanel('${key}')">${label}</button>`;
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
    const matrixPage = currentMatrixPage(matrix);
    const disabled = candidateEditable(selectedDetail) ? '' : 'disabled';
    return `<div class="tab-panel">
      ${renderCandidateEditHint(selectedDetail)}
      ${renderSchemaOrderControls(schemas, disabled)}
      <section class="panel-section">
        <h3>Field</h3>
        <label>Field
          <select id="selectedFieldSelect" onchange="selectedField=this.value; rerenderWorkbench()">
            ${schemaFields(currentSchema()).map(field => `<option value="${escapeHtmlAttr(field)}" ${field === selectedField ? 'selected' : ''}>${escapeHtml(fieldLabel(field))}${fieldFindings(matrixPage, field).length ? ' (failed check)' : ''}</option>`).join('')}
          </select>
        </label>
        ${renderRawAttributeAdder(disabled)}
        <label>Selector
          <span class="selector-box">
            <input id="selectedSelector" value="${escapeHtmlAttr(currentSelector())}" oninput="setCurrentSelector(this.value); postHighlight(this.value)">
            <button class="primary" ${disabled} onclick="saveCurrentField()">Save field</button>
            <button class="danger" ${disabled || (fieldDeletable(selectedField) ? '' : 'disabled')} onclick="deleteCurrentRule()">Delete rule</button>
          </span>
        </label>
        ${renderSelectedFieldFindings(matrixPage)}
        <div class="actions">
          <button onclick="applyPickedSelector()">Use clicked element</button>
          <button onclick="postHighlight(currentSelector())">Highlight field</button>
        </div>
        ${renderPickedElement()}
        ${renderRuleOptions(rule)}
        ${renderAdditionalSelectors(rule)}
      </section>
      <section class="panel-section">
        <h3>Highlight legend</h3>
        ${renderSchemaLegend(currentSchema())}
      </section>
    </div>`;
}

// Raw attribute keys come from the crawler's registry: an edit may add only those, while a key
// the candidate already holds stays editable and removable.
function renderRawAttributeAdder(disabled) {
    const configured = new Set(Object.keys(currentSchema().raw_attributes || {}));
    const available = (rawAttributeKeys || []).filter(entry => !configured.has(entry.key));
    if (!available.length) return '';
    const groups = [...new Set(available.map(entry => entry.group))];
    return `<div class="raw-attribute-adder">
      <label>Raw attribute
        <select id="newRawAttributeKey" ${disabled}>
          ${groups.map(group => `<optgroup label="${escapeHtmlAttr(displayName(group))}">${
            available.filter(entry => entry.group === group).map(entry =>
                `<option value="${escapeHtmlAttr(entry.key)}" title="${escapeHtmlAttr(entry.description)}">${escapeHtml(entry.key)}</option>`
            ).join('')}</optgroup>`).join('')}
        </select>
      </label>
      <button ${disabled} onclick="addRawAttribute()" title="Starts from the clicked element when one is picked; save the field to store it">Add raw attribute</button>
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

// The element last clicked in the preview, kept across side-panel renders.
function renderPickedElement() {
    return `<div class="picked" id="pickedElement">${pickedSelector
        ? `<code>${escapeHtml(pickedSelector)}</code>${pickedSample ? `<span class="muted">${escapeHtml(pickedSample)}</span>` : ''}`
        : '<span class="muted">Click an element in the preview to pick it.</span>'}</div>`;
}

function renderRuleOptions(rule) {
    const effectiveRule = rule || defaultRuleFor(selectedField);
    const cardinality = effectiveRule.cardinality || 'first';
    const type = effectiveRule.type || 'text';
    return `<div class="rule-options-card" title="Applies to the primary and additional selectors for this field">
      <div class="field-grid">
        <label>Cardinality
          <select onchange="setCurrentRuleCardinality(this.value)">
            <option value="first" ${cardinality === 'first' ? 'selected' : ''}>First match</option>
            <option value="all" ${cardinality === 'all' ? 'selected' : ''}>All matches</option>
          </select>
        </label>
        <label>Extract
          <select onchange="setCurrentRuleType(this.value)">
            <option value="text" ${type === 'text' ? 'selected' : ''}>Text</option>
            <option value="attribute" ${type === 'attribute' ? 'selected' : ''}>Attribute</option>
            <option value="image_url" ${type === 'image_url' ? 'selected' : ''} title="The best full-size image URL candidate of each matched element">Image URL</option>
          </select>
        </label>
      </div>
      ${type === 'attribute' ? `<label>Attribute name
        <input value="${escapeHtmlAttr(effectiveRule.name || '')}" oninput="setCurrentRuleAttributeName(this.value)">
      </label>` : ''}
    </div>`;
}

function renderSchemaOrderControls(schemas, disabled) {
    if (!schemas.length) return '<div class="hint-card">No schemas available for ordering.</div>';
    return `<section class="panel-section schema-order-card">
      <h3 title="Approval writes schemas in this order for the ListingSource">Schemas</h3>
      <div class="schema-order-list">
        ${schemas.map((_, i) => `<button class="schema-chip ${i === selectedSchemaIndex ? 'active' : ''}" onclick="selectedSchemaIndex=${i}; normalizeSchemaSelection(); rerenderWorkbench()">${schemaLabel(i)}</button>`).join('')}
      </div>
      <div class="schema-order-actions">
        <button ${disabled || (selectedSchemaIndex === 0 ? 'disabled' : '')} onclick="moveCurrentSchema(-1)">Move up</button>
        <button ${disabled || (selectedSchemaIndex >= schemas.length - 1 ? 'disabled' : '')} onclick="moveCurrentSchema(1)">Move down</button>
        <button ${disabled} onclick="addNewSchema()" title="Adds a copy of the selected schema to rewrite for another template">Add schema</button>
        <button class="danger" ${disabled || (schemas.length <= 1 ? 'disabled' : '')} onclick="deleteCurrentSchema()">Delete schema</button>
      </div>
      <div class="actions">
        <button onclick="postHighlightSchema()">Highlight schema</button>
        <button onclick="reevaluateMatrix()">Re-evaluate</button>
        <button onclick="discardEdits()">Discard edits</button>
      </div>
    </section>`;
}

// The handlers let postHighlight name the selected field: a raw attribute key comes from the
// candidate, and HTML escaping does not make it safe inside an inline handler's JS string.
function renderAdditionalSelectors(rule) {
    const additional = (rule && Array.isArray(rule.additional_selectors)) ? rule.additional_selectors : [];
    return `<div class="additional-selector-card">
      <h4 title="Fallback or extra selectors for the selected field">Additional selectors</h4>
      <div class="additional-selector-list">
        ${additional.length ? additional.map((selector, index) => `
          <div class="additional-selector-row">
            <input value="${escapeHtmlAttr(selector)}" oninput="setAdditionalSelector(${index}, this.value); postHighlight(this.value)">
            <button onclick="usePickedForAdditionalSelector(${index})">Use clicked</button>
            <button onclick="postHighlight(additionalSelectorAt(${index}))">Highlight</button>
            <button class="danger" onclick="removeAdditionalSelector(${index})">Delete</button>
          </div>`).join('') : ''}
      </div>
      <div class="actions">
        <button onclick="addAdditionalSelector()">Add additional selector</button>
        <button onclick="addPickedAdditionalSelector()">Add clicked selector</button>
      </div>
    </div>`;
}

function renderDataPanel(matrix) {
    return `<div class="tab-panel">
      ${renderCoverageSummary(matrix)}
      ${renderCurrentExtractedData(matrix)}
    </div>`;
}

function renderDiffPanel(schemas) {
    if (schemas.length < 2) {
        return `<div class="tab-panel"><div class="diff-empty">No second schema to compare.</div></div>`;
    }
    normalizeSchemaSelection();
    return `<div class="tab-panel">
      <div class="field-grid">
        <label>Current schema
          <select onchange="selectedSchemaIndex=Number(this.value); normalizeSchemaSelection(); rerenderWorkbench()">
            ${schemas.map((_, i) => `<option value="${i}" ${i === selectedSchemaIndex ? 'selected' : ''}>${schemaLabel(i)}</option>`).join('')}
          </select>
        </label>
        <label>Compare with
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
      <div class="panel-title">
        <strong>Removed-page templates</strong>
        <span class="muted">${editable
            ? 'Declare a page removed: a selector bound to the removal notice, with text it contains or a regex. A regex that matches any text is rejected. The list starts from the stored templates; approval replaces them with it, so removing a template here retires it.'
            : detail.review.status === 'APPROVED'
                ? 'Stored when the review was approved; edit or retire templates on a pending review.'
                : 'This review is closed without approval; it did not change the stored templates.'}</span>
      </div>
      ${templates.map((template, index) => renderRemovedPageTemplate(template, index, disabled)).join('')
        || '<div class="muted">No removed-page template.</div>'}
      <div class="actions">
        <button ${disabled} onclick="addRemovedPageTemplate()" title="Starts from the clicked element when one is picked">Add template</button>
        <button ${disabled} onclick="saveRemovedPageTemplates()">Save templates</button>
        <button ${disabled} onclick="discardEdits()">Discard edits</button>
      </div>
      ${renderPickedElement()}
      <h3>Matched review pages</h3>
      ${renderRemovedPageMatches(matrix, templates)}
    </div>`;
}

function renderRemovedPageTemplate(template, index, disabled) {
    const mode = template.regex !== undefined && template.regex !== null ? 'regex' : 'text';
    const value = mode === 'regex' ? template.regex : (template.text || '');
    return `<div class="field-grid removed-page-template">
      <label>Template ${index + 1} selector
        <input ${disabled} value="${escapeHtmlAttr(template.selector || '')}" oninput="setRemovedPageTemplate(${index}, {selector: this.value})">
      </label>
      <label>Match
        <select ${disabled} onchange="setRemovedPageTemplateMode(${index}, this.value)">
          <option value="text" ${mode === 'text' ? 'selected' : ''}>Text</option>
          <option value="regex" ${mode === 'regex' ? 'selected' : ''}>Regex</option>
        </select>
      </label>
      <label>${mode === 'regex' ? 'Regex' : 'Notice text'}
        <input ${disabled} value="${escapeHtmlAttr(value)}" oninput="setRemovedPageTemplateValue(${index}, this.value)">
      </label>
      <div class="actions">
        <button onclick="postHighlight(currentRemovedPageTemplates()[${index}].selector, 'Removed notice', '#dc2626')">Highlight</button>
        <button ${disabled} onclick="usePickedForRemovedPageTemplate(${index})">Use clicked element</button>
        <button ${disabled} onclick="removeRemovedPageTemplate(${index})">Remove</button>
      </div>
    </div>`;
}

// Mirrors `removed_page_matches` in the schema matrix: the review pages the templates match,
// and whether the scraper would capture each as removed or keep scraping it as a product
// because a candidate schema still extracts it. A template that takes a plausibly extracted
// title for its notice asks for a breadth check: it would withdraw that page, which is right only
// when the page is a removal notice that keeps the product template.
function renderRemovedPageMatches(matrix, templates) {
    if (!templates.length) return '<div class="muted">Declare a template to see the pages it matches.</div>';
    if (dirty) return '<div class="muted">Save the templates to evaluate them on the review pages.</div>';
    const matches = (matrix && matrix.removed_page_matches) || [];
    if (!matches.length) return '<div class="muted">No review page matches the templates.</div>';
    return `<table>
      <thead><tr><th>Page</th><th>Role</th><th>Outcome</th><th>Matched notices</th></tr></thead>
      <tbody>${matches.map(match => `<tr>
        <td>${productPageLink(match.url)}</td>
        <td>${escapeHtml(match.role)}</td>
        <td>${match.outcome === 'REMOVED'
            ? statusBadge('Captured as removed')
            : statusBadge('Scraped as a product: a candidate schema extracts it')}</td>
        <td>${(match.templates || []).map(t => `Template ${t.template_index + 1}: ${escapeHtml((t.notices || []).join(' | '))}${t.product_title
            ? `<br>${statusBadge('Check breadth')} takes the title “${escapeHtml(t.product_title)}” a candidate schema extracts for its notice: too broad unless this page is a removal notice`
            : ''}`).join('<br>')}</td>
      </tr>`).join('')}</tbody>
    </table>`;
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

// The selected review could not be loaded, so no detail of it is shown to keep the message with.
function renderReviewLoadFailure() {
    selectedDetail = null;
    selectedMatrix = null;
    document.getElementById('detail').innerHTML = `<div class="hint-card finding-hint" role="alert">
      <span>The review could not be loaded: ${escapeHtml(editRefusal?.message || '')}</span>
      <button onclick="selectReview(selectedId)">Retry</button>
    </div>`;
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
        return `<div class="hint-card">
          <span>No schema matrix yet: the current candidate has not been evaluated on the live review pages. Evaluating fetches every review page from the shop at its crawl delay.</span>
          <button onclick="reevaluateMatrix()">Evaluate on live pages</button>
        </div>`;
    }
    if (selectedMatrixFailure) {
        return `<div class="hint-card finding-hint">
          <span>No schema matrix: ${escapeHtml(selectedMatrixFailure)}</span>
          <button onclick="reevaluateMatrix()">Re-evaluate</button>
        </div>`;
    }
    const unevaluated = unevaluatedReviewPages(detail, matrix);
    if (!unevaluated.length) return '';
    return `<div class="hint-card">
      <span>Not evaluated: ${unevaluated.length === 1 ? 'a page was' : `${unevaluated.length} pages were`} added to the review while this matrix was evaluated (${unevaluated.map(page => escapeHtml(reviewPageLabel(page))).join(', ')}).</span>
      <button onclick="reevaluateMatrix()">Re-evaluate</button>
    </div>`;
}

function renderEditMessages() {
    const refusal = editRefusal && editRefusal.reviewId === selectedId
        ? `<div class="hint-card finding-hint edit-message" role="alert">
            <span>${escapeHtml(editRefusal.message)}</span>
            <button class="message-dismiss" onclick="dismissEditMessage('refusal')" title="Dismiss" aria-label="Dismiss">&times;</button>
          </div>`
        : '';
    const notice = editNotice && editNotice.reviewId === selectedId
        ? `<div class="hint-card success-hint edit-message" role="status">
            <span>${escapeHtml(editNotice.message)}</span>
            <button class="message-dismiss" onclick="dismissEditMessage('notice')" title="Dismiss" aria-label="Dismiss">&times;</button>
          </div>`
        : '';
    return refusal + notice;
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
      <div class="panel-title">
        <strong>Full Candidate JSON</strong>
        <span class="muted">Use field controls for normal edits; this is the raw payload.</span>
      </div>
      <textarea id="candidatePayload" oninput="markDirty()">${escapeHtml(JSON.stringify(detail.review.candidate_payload, null, 2))}</textarea>
      <div class="actions">
        <button ${candidateEditable(detail) ? '' : 'disabled'} onclick="saveCandidate()">Save full JSON</button>
        <button onclick="discardEdits()">Discard edits</button>
      </div>
      <h3>Validation</h3>
      <pre>${escapeHtml(JSON.stringify(detail.review.validation_summary, null, 2))}</pre>
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

function renderCoverageSummary(matrix) {
    if (!matrix || !matrix.candidates || !matrix.candidates.length) {
        return '<div class="muted">No schema coverage data available.</div>';
    }
    const coverage = reviewPageCoverage(selectedDetail, matrix);
    const scope = coverage.notEvaluated ? 'evaluated' : 'live';
    const failures = currentCandidateFailures(matrix);
    return `<div class="hint-card">
      <strong>Coverage</strong>
      <div>${coverageText(coverage)}.</div>
      ${failures.length ? `<div class="muted">Selected schema failed on ${failures.length} page${failures.length === 1 ? '' : 's'}: ${failures.map(page => escapeHtml(displayName(page.role))).join(', ')}</div>` : `<div class="muted">Selected schema applied to every ${scope} page.</div>`}
      ${renderSchemasApplyingToNoPage(matrix, scope)}
      ${renderOptionalMissSummary(currentCandidatePagesWithOptionalMisses(matrix))}
      ${renderSkippedElementSummary(currentCandidatePagesWithSkippedElements(matrix))}
      ${renderPlausibilitySummary(matrix)}
    </div>`;
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

function renderFindingBadges(findings) {
    return findings.map(finding =>
        `<span class="badge badge-rejected finding-badge" title="${escapeHtmlAttr(findingDescription(finding))}">${escapeHtml(finding.check)}</span>`
    ).join(' ');
}

// Cross-page findings name the values another product page repeats; for a list such as
// the images they point out the default share image among the product photos, which lists
// like the field's own images, by its resolved first candidate.
function renderSharedValues(findings, key) {
    const values = findings.flatMap(finding => finding.values || []);
    if (!values.length) return '';
    return `<div class="muted">Also extracted from another product page: ${key === 'images'
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
    return `<div class="muted">Contradicting identities: ${values.join(', ')}</div>`;
}

function renderPlausibilitySummary(matrix) {
    const failingSchemas = matrix.candidates.filter(candidate =>
        (candidate.pages || []).some(page => pageFindings(page).length > 0)
    );
    if (!failingSchemas.length) {
        return '<div class="muted">Every schema passed the plausibility checks on every live page.</div>';
    }
    const candidate = selectedCandidate(matrix);
    const failingPages = (candidate?.pages || []).filter(page => pageFindings(page).length > 0);
    const perPage = failingPages.map(page => `<li><strong>${escapeHtml(displayName(page.role))}</strong><span>${
        pageFindings(page).map(finding => escapeHtml(findingDescription(finding))).join('; ')
    }</span></li>`).join('');
    const pagePlural = failingPages.length === 1 ? '' : 's';
    return `<div class="plausibility-summary">
      <strong>Plausibility</strong>
      <div>${failingSchemas.map(c => escapeHtml(schemaLabel(c.schema_index))).join(', ')} failed plausibility checks, which keeps a generated schema out of auto-approval.</div>
      ${failingPages.length
        ? `<div class="muted">Selected schema failed them on ${failingPages.length} page${pagePlural}:</div><ul class="plausibility-pages">${perPage}</ul>`
        : '<div class="muted">Selected schema passed them on every live page.</div>'}
    </div>`;
}

function renderSelectedFieldFindings(page) {
    const findings = fieldFindings(page, selectedField);
    if (!findings.length) return '';
    return `<div class="hint-card finding-hint">
      ${renderFindingBadges(findings)}
      <span>${findings.map(finding => escapeHtml(findingDescription(finding))).join('; ')} on the selected page.</span>
    </div>`;
}

function renderOptionalMissSummary(pagesWithMisses) {
    if (!pagesWithMisses.length) return '';
    const pageWord = pagesWithMisses.length === 1 ? 'page' : 'pages';
    const perPage = pagesWithMisses.map(page => {
        const fields = optionalFieldMisses(page).map(field =>
            `${escapeHtml(displayName(field.field))}${matchedOnlyEmptyElements(field) ? ' (empty element)' : ''}`
        ).join(', ');
        return `${escapeHtml(displayName(page.role))} (${fields})`;
    }).join('; ');
    return `<div class="muted">Optional rules yielded no value (field left empty, page still applied) on ${pagesWithMisses.length} ${pageWord}: ${perPage}</div>`;
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

function currentCandidatePagesWithOptionalMisses(matrix) {
    const candidate = selectedCandidate(matrix);
    if (!candidate) return [];
    return (candidate.pages || []).filter(page => optionalFieldMisses(page).length > 0);
}

// Fields whose rule skipped matched elements without an image URL (no image URL candidate, or
// no attribute of an attribute rule) on the page (`skipped_element_count`, set for the `images`
// rule only). Next to extracted images, skips show a loose selector.
function fieldsWithSkippedElements(page) {
    return (page?.fields || []).filter(field => field.skipped_element_count > 0);
}

function skippedElementCount(page) {
    return fieldsWithSkippedElements(page).reduce((sum, field) => sum + field.skipped_element_count, 0);
}

function matchedElementCount(field) {
    return [field.selector_match_count || 0, ...(field.additional_selector_match_counts || [])]
        .reduce((sum, count) => sum + count, 0);
}

function currentCandidatePagesWithSkippedElements(matrix) {
    const candidate = selectedCandidate(matrix);
    if (!candidate) return [];
    return (candidate.pages || []).filter(page => skippedElementCount(page) > 0);
}

// A rule that skipped every element it matched yields no image: the page shows an empty
// gallery of placeholders, which is no sign of a loose selector.
function isEmptyGallery(page) {
    return fieldsWithSkippedElements(page).every(field => field.skipped_element_count === matchedElementCount(field));
}

function renderSkippedElementSummary(pagesWithSkips) {
    const summary = (pages, description) => {
        if (!pages.length) return '';
        const perPage = pages.map(page =>
            `${escapeHtml(displayName(page.role))} (${skippedElementCount(page)})`
        ).join('; ');
        return `<div class="muted">${description} on ${pages.length} ${pages.length === 1 ? 'page' : 'pages'}: ${perPage}</div>`;
    };
    return summary(
        pagesWithSkips.filter(page => !isEmptyGallery(page)),
        'Selected schema skipped matched elements without an image URL next to extracted images, a sign of a loose selector,'
    ) + summary(
        pagesWithSkips.filter(isEmptyGallery),
        'Selected schema matched only elements without an image URL, an empty gallery,'
    );
}

function renderSkippedElements(fields) {
    if (!fields.length) return '';
    return `<div class="muted">Matched elements without an image URL were skipped: ${
        fields.map(field => `${escapeHtml(displayName(field.field))} <code>${escapeHtml(field.selector)}</code> skipped ${field.skipped_element_count} of ${matchedElementCount(field)}`).join(', ')
    }</div>`;
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

// Schemas that apply to no live review page. The reviewer retires a schema whose template the
// shop no longer uses by deleting it from the candidate; another page of its template, added to
// the review, shows whether it is still needed.
function renderSchemasApplyingToNoPage(matrix, scope) {
    const unused = matrix.candidates
        .filter(candidate => (candidate.pages || []).length && candidate.pages.every(page => !page.apply_ok))
        .map(candidate => escapeHtml(schemaLabel(candidate.schema_index)));
    if (!unused.length) return '';
    return `<div class="muted">Applies to no ${scope} page: ${unused.join(', ')}. Add a page of its template, or delete the schema once the shop no longer uses that template.</div>`;
}

function currentCandidateFailures(matrix) {
    const candidate = selectedCandidate(matrix);
    if (!candidate) return [];
    return (candidate.pages || []).filter(page => !page.apply_ok);
}

function renderCurrentExtractedData(matrix) {
    const candidate = selectedCandidate(matrix);
    if (!candidate) return '<div class="muted">No extracted data for this schema.</div>';
    const page = selectedMatrixPage(candidate);
    if (!page) return '<div class="muted">No extracted data for the selected page.</div>';
    const misses = optionalFieldMisses(page);
    const skips = fieldsWithSkippedElements(page);
    // A rejecting plausibility finding fails the page but keeps its extraction, so the
    // reviewer sees the failing check next to the field it concerns.
    return `<section class="extracted-page ${page.apply_ok ? '' : 'failed'}">
      <div class="extracted-page-head">
        <strong>${escapeHtml(displayName(page.role))}</strong>
        <span class="extracted-page-status">${renderPageStatusBadges(page, misses, skips)}</span>
      </div>
      ${page.apply_ok ? '' : `<div class="page-error">${expandableValue(page.error || 'Schema did not apply.')}</div>`}
      ${page.extracted ? rawDataCell(page.extracted, page) : ''}
      ${page.apply_ok ? renderOptionalFieldMisses(misses) : ''}
      ${renderSkippedElements(skips)}
    </section>`;
}

function renderPageStatusBadges(page, misses, skips) {
    const findings = pageFindings(page);
    const checkPlural = findings.length === 1 ? '' : 's';
    return `<span class="${page.apply_ok ? 'status-ok badge' : 'status-failed badge'}">${page.apply_ok ? 'OK' : 'Failed'}</span>`
        + (misses.length ? ` <span class="badge badge-missing">${misses.length} empty</span>` : '')
        + (skips.length ? ` <span class="badge badge-pending" title="Matched elements without an image URL">${skippedElementCount(page)} skipped</span>` : '')
        + (findings.length ? ` <span class="badge badge-rejected">${findings.length} failed check${checkPlural}</span>` : '');
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

function renderOptionalFieldMisses(misses) {
    const fieldList = fields => fields
        .map(field => `${escapeHtml(displayName(field.field))} <code>${escapeHtml(field.selector)}</code>`)
        .join(', ');
    const unmatched = misses.filter(field => !matchedOnlyEmptyElements(field));
    const empty = misses.filter(matchedOnlyEmptyElements);
    return (unmatched.length ? `<div class="muted">Rule matched nothing, field left empty: ${fieldList(unmatched)}</div>` : '')
        + (empty.length ? `<div class="muted">Rule matched only empty elements, field left empty: ${fieldList(empty)}</div>` : '');
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
    if (!raw) return '<span class="muted">No extracted data.</span>';
    const fields = Object.entries(raw).filter(([key, value]) =>
        hasRawValue(value) || fieldFindings(page, key).length > 0
    );
    if (!fields.length) return '<span class="muted">No extracted data.</span>';
    return `<dl class="raw-data">${
        fields.map(([key, value]) => {
            const findings = fieldFindings(page, key);
            return `
      <div class="${findings.length ? 'implausible-field' : ''}">
        <dt>${escapeHtml(displayName(key))}</dt>
        <dd>${findings.length ? `<div class="finding-badges">${renderFindingBadges(findings)}</div>` : ''}${hasRawValue(value) ? (key === 'images' && Array.isArray(value) ? renderImageCandidateGroups(value) : rawValue(value)) : '<span class="muted">blank</span>'}${Array.isArray(value) ? renderSharedValues(findings, key) : renderContradictingIdentities(findings)}</dd>
      </div>`;
        }).join('')
    }</dl>`;
}

function renderSchemaLegend(schema) {
    const rules = schemaHighlightRules(schema);
    if (!rules.length) return '<div class="hint-card">No configured selectors for this schema.</div>';
    return `<div class="schema-legend">${
        rules.map(rule => `<div class="legend-item"><span class="legend-swatch" style="background:${escapeHtmlAttr(rule.color)}"></span>${escapeHtml(displayName(rule.field))}</div>`).join('')
    }</div>`;
}

function renderSchemaDiff(current, compare) {
    const rawAttributeFields = Array.from(new Set(Object.keys(current?.raw_attributes || {}).concat(Object.keys(compare?.raw_attributes || {}))))
        .map(field => `raw_attributes.${field}`);
    const fields = [...selectorFields, ...rawAttributeFields];
    return `<div class="schema-diff">${
        fields.map(field => {
            const left = normalizeRuleValue(schemaFieldValue(current, field));
            const right = normalizeRuleValue(schemaFieldValue(compare, field));
            const status = diffStatus(left, right);
            return `<div class="diff-row">
              <div class="diff-field">${escapeHtml(fieldLabel(field))}</div>
              ${statusBadge(status)}
              <div class="diff-values">
                <div class="diff-value"><span class="diff-label">Selected</span>${expandableValue(left || 'not configured')}</div>
                <div class="diff-value"><span class="diff-label">Compare</span>${expandableValue(right || 'not configured')}</div>
              </div>
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
        ? `<details class="expandable-text"><summary>${rest.length} more image${rest.length === 1 ? '' : 's'}</summary><ol class="image-list" start="${IMAGES_SHOWN_BEFORE_COLLAPSE + 1}">${rest.join('')}</ol></details>`
        : ''}`;
}

function renderImageCandidateGroup(group) {
    const candidates = imageCandidates(group);
    const [first = ''] = candidates;
    const address = resolvedPageAddress(first);
    const fallbacks = candidates.length - 1;
    const tooltip = escapeHtmlAttr(candidates.map(resolvedPageAddress).join('\n'));
    const label = escapeHtml(address);
    return `<li title="${tooltip}"><div>${/^https?:\/\//i.test(address)
        ? `<a href="${escapeHtmlAttr(address)}" target="_blank" rel="noopener noreferrer">${label}</a>`
        : `<span>${label}</span>`}${fallbacks > 0 ? `<span class="image-fallbacks">+${fallbacks} fallback${fallbacks === 1 ? '' : 's'}</span>` : ''}</div></li>`;
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
    return `<details class="expandable-text">
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

function productPageLink(url) {
    return `<a class="product-url" href="${escapeHtmlAttr(url)}" target="_blank" rel="noopener noreferrer" title="${escapeHtmlAttr(url)}">${escapeHtml(url)}</a>`;
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

function setCurrentRuleType(type) {
    const rule = ensureCurrentRule();
    rule.type = type;
    if (type === 'attribute') {
        rule.name = rule.name || (selectedField === 'images' ? 'src' : 'href');
    } else {
        delete rule.name;
    }
    markDirty();
    rerenderWorkbench();
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

function usePickedForAdditionalSelector(index) {
    if (!pickedSelector) return;
    setAdditionalSelector(index, pickedSelector);
    rerenderWorkbench();
    postHighlight(pickedSelector);
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
    }, rule ? 'Field saved.' : 'Rule deleted.', reloadSavedCandidate);
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
    }, 'Page added to the review.', reloadMatrix));
}

function removeReviewPage() {
    const page = (selectedDetail.pages || []).find(p => p.review_page_id === selectedPageId);
    if (!pageRemovable(page)) return;
    openActionPanel({
        title: 'Remove review page',
        message: `Remove ${page.url} from the review pages?${dirty ? ` Removing it ${DISCARDS_EDITS}` : ''}`,
        confirmLabel: 'Remove page',
        danger: true,
        run: () => submitMutation(async () => {
            await api(`/api/review-pages/${page.review_page_id}`, {method: 'DELETE'});
            selectedPageId = null;
        }, 'Page removed from the review.', reloadMatrix)
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

// The review an action panel acts on, as the reviewer finds it in the queue.
function selectedReviewName() {
    const review = selectedDetail?.review || {};
    return `${review.listing_source_name || review.listing_source_id || 'review'}, ${displayName(review.artifact_type)}`;
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
        title: `Approve ${selectedReviewName()}`,
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
            return submitVerdict('approve', verdict, 'Approved.');
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
        title: `Restore previous: ${selectedReviewName()}`,
        message: `Restore ${what} this approval replaced? Pages waiting on a review are retried with it.`,
        notes: 'optional',
        notesLabel: 'Why restore? (optional)',
        confirmLabel: 'Restore previous',
        run: notes => submitVerdict('restore', {notes}, 'Previous configuration restored.')
    });
}

// Lets the crawler scrape this review's seed and triggering pages again.
function revokeRejection() {
    openActionPanel({
        title: `Revoke rejection: ${selectedReviewName()}`,
        message: 'Revoke this rejection? Its seed and triggering pages are classified again by their URL pattern and scraped again; a page no cached schema extracts opens a new review.',
        notes: 'optional',
        notesLabel: 'Why revoke? (optional)',
        confirmLabel: 'Revoke rejection',
        run: notes => submitVerdict('revoke-rejection', {notes}, 'Rejection revoked.')
    });
}

function rejectReview() {
    openActionPanel({
        title: `Reject ${selectedReviewName()}`,
        notes: 'optional',
        confirmLabel: 'Reject',
        danger: true,
        run: notes => submitVerdict('reject', {notes}, 'Rejected.')
    });
}

function needsRepair() {
    openActionPanel({
        title: `Needs repair: ${selectedReviewName()}`,
        notes: 'required',
        notesLabel: 'What needs repair?',
        confirmLabel: 'Request repair',
        run: notes => submitVerdict('needs-repair', {notes}, 'Repair requested.')
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
    return `<div class="action-panel ${action.danger ? 'action-panel-danger' : ''}" role="dialog" aria-label="${escapeHtmlAttr(action.title)}" onkeydown="actionPanelKeydown(event)">
      <strong>${escapeHtml(action.title)}</strong>
      ${action.message ? `<div class="action-panel-message">${escapeHtml(action.message)}</div>` : ''}
      ${action.notes ? `<label>${escapeHtml(action.notesLabel)}
        <textarea id="actionPanelNotes" rows="3" ${busy} oninput="setActionPanelNotes(this.value)">${escapeHtml(action.notesText)}</textarea>
      </label>` : ''}
      <div class="actions">
        <button id="actionPanelConfirm" class="${action.danger ? 'danger' : 'primary'}" ${busy || blocked} onclick="confirmActionPanel()">${escapeHtml(action.busy ? 'Sending…' : action.confirmLabel)}</button>
        <button ${busy} onclick="cancelActionPanel()">Cancel</button>
      </div>
    </div>`;
}

// Updates the panel alone, so unsaved edits in the workbench's inputs stay as typed.
function updateActionPanel() {
    const panel = document.getElementById('actionPanel');
    if (panel) panel.innerHTML = renderActionPanel();
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
    }, 'Candidate saved.', reloadSavedCandidate);
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

// The state is a live region that every render updates, so it changes its text only when the
// state changes: rewriting the same text would have a screen reader announce it again.
function updateReloadState() {
    const state = document.getElementById('reloadState');
    const [text, className] = dirty
        ? ['Review has local edits', 'system-status dirty']
        : selectedReviewNeedsRefresh
            ? ['Selected review changed elsewhere - refresh to update', 'system-status stale']
            : ['Queue auto refresh active', 'system-status'];
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
        toggle.setAttribute('aria-label', collapsed ? 'Expand reviews' : 'Collapse reviews');
        toggle.setAttribute('title', collapsed ? 'Expand reviews' : 'Collapse reviews');
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
    banner.innerHTML = `
    <span>${escapeHtml(selectedReviewRefreshReason || 'Selected review changed elsewhere.')}</span>
    <button onclick="refreshSelectedReview()">Refresh review</button>
  `;
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

// The tone of each status the console knows by its exact value: review statuses and the schema
// diff's field statuses. Any other text renders neutral unless its caller names a tone, so a
// negative text such as "Not approved" can never take the approved style by a substring.
const STATUS_TONES = {
    PENDING_REVIEW: 'pending',
    APPROVED: 'approved',
    REJECTED: 'rejected',
    SUPERSEDED: 'superseded',
    'same': 'approved',
    'changed': 'changed',
    'missing current': 'missing',
    'missing compare': 'missing'
};

const BADGE_TONES = new Set(['pending', 'approved', 'rejected', 'changed', 'missing', 'human', 'repair', 'superseded']);

function statusBadge(status) {
    return badge(displayName(status), STATUS_TONES[status]);
}

// A badge with the text `text` in the tone `tone`, one of `BADGE_TONES`; neutral without one.
function badge(text, tone) {
    const cls = BADGE_TONES.has(tone) ? `badge badge-${tone}` : 'badge';
    return `<span class="${cls}">${escapeHtml(text)}</span>`;
}

function displayName(value) {
    return String(value ?? '')
        .replace(/[_-]+/g, ' ')
        .replace(/\s+/g, ' ')
        .trim()
        .toLowerCase()
        .replace(/\b\w/g, ch => ch.toUpperCase());
}

function formatDate(value) {
    if (!value) return '';
    const date = new Date(value);
    if (Number.isNaN(date.getTime())) return String(value);
    return date.toLocaleString();
}

function timeBadge(value, label) {
    if (!value) return '';
    return `<span class="time-badge" title="${escapeHtmlAttr(`${label}: ${formatDate(value)}`)}">${escapeHtml(elapsedTime(value))}</span>`;
}

// The one disclosure glyph of the console: it points down while its section is open and right
// while it is closed (the CSS turns it); the sidebar turns it to point where the sidebar moves.
function chevronIcon() {
    return '<svg viewBox="0 0 16 16" width="16" height="16" focusable="false"><path d="M4 6l4 4 4-4" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>';
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
    pickedSample = `${data.tag || ''} ${data.text || data.src || data.href || ''}`.trim();
    const picked = document.getElementById('pickedElement');
    if (picked) picked.outerHTML = renderPickedElement();
});

// The URL fragment names the selected review and, for a schema review, its page and schema, so
// reload, browser back and a shared link restore them: `#review=<id>&page=<id>&schema=<index>`.
// A link is untrusted and the review id becomes part of API paths, so only an id of the review
// id's characters is taken; anything else, such as `/` or `?`, selects no review.
const REVIEW_ID_PATTERN = /^[A-Za-z0-9_-]+$/;

function selectionFromUrl() {
    const params = new URLSearchParams(location.hash.slice(1));
    const review = params.get('review');
    const schema = params.get('schema');
    return {
        reviewId: REVIEW_ID_PATTERN.test(review || '') ? review : null,
        pageId: params.get('page') || null,
        schemaIndex: /^\d+$/.test(schema || '') ? Number(schema) : null
    };
}

function selectionUrlHash() {
    if (!selectedId) return '';
    const params = new URLSearchParams({review: selectedId});
    if (selectedMatrix && selectedDetail?.review.review_id === selectedId) {
        if (selectedPageId) params.set('page', selectedPageId);
        params.set('schema', String(selectedSchemaIndex));
    }
    return `#${params}`;
}

// Selecting another review adds a history entry, so browser back returns to the previous one;
// a page or schema change within the review only updates the current entry.
function recordSelectionInUrl() {
    if (selectionUrlHash() === location.hash) return;
    if (selectionFromUrl().reviewId === selectedId) {
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

// Following the URL to another review discards unsaved edits, so it asks first; meanwhile the
// URL keeps naming the review on screen. Confirming puts the requested URL back into the
// history entry the browser moved to, so it adds no entry and back and forward keep their order.
function applySelectionFromUrl() {
    const requested = selectionFromUrl();
    if (requested.reviewId === selectedId) {
        applyRequestedPageAndSchema(requested);
        return;
    }
    const follow = () => requested.reviewId
        ? selectReview(requested.reviewId, requested)
        : clearSelectedReview();
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
    const detail = document.getElementById('detail');
    detail.textContent = 'Select a review.';
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

loadReviews().catch(err => document.getElementById('reviewList').textContent = err);
setSidebarCollapsed(sidebarCollapsed);
if (selectionFromUrl().reviewId) applySelectionFromUrl();
