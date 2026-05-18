let selectedId = null;
let selectedDetail = null;
let selectedMatrix = null;
let selectedSchemaIndex = 0;
let compareSchemaIndex = 1;
let selectedField = 'title';
let selectedPageId = null;
let activeWorkbenchPanel = 'selector';
let pickedSelector = '';
let dirty = false;
let selectedReviewUpdatedAt = null;
let selectedReviewStatus = null;
let selectedReviewNeedsRefresh = false;
let selectedReviewRefreshReason = '';
let sidebarCollapsed = localStorage.getItem('crawlerReviewSidebarCollapsed') === 'true';
let reviewSearchTerm = '';
let collapsedShopGroups = new Set(JSON.parse(localStorage.getItem('crawlerReviewCollapsedShopGroups') || '[]'));

const selectorFields = [
    'shops_product_id', 'title', 'description', 'price', 'price_estimate_min',
    'price_estimate_max', 'state', 'images', 'auction_start', 'auction_end'
];
const optionalFields = new Set([
    'description', 'price', 'price_estimate_min', 'price_estimate_max',
    'auction_start', 'auction_end'
]);
const schemaHighlightColors = {
    shops_product_id: '#2563eb',
    title: '#7c3aed',
    description: '#0891b2',
    price: '#16a34a',
    price_estimate_min: '#65a30d',
    price_estimate_max: '#0d9488',
    state: '#f97316',
    images: '#db2777',
    auction_start: '#9333ea',
    auction_end: '#dc2626'
};

async function api(path, options = {}) {
    const res = await fetch(path, {headers: {'content-type': 'application/json'}, ...options});
    if (!res.ok) throw new Error(await res.text());
    return res.json();
}

async function loadReviews() {
    const reviews = await api('/api/reviews');
    const list = document.getElementById('reviewList');
    list.innerHTML = renderReviewGroups(reviews);
    updateSelectedReviewFreshness(reviews);
    return reviews;
}

function renderReviewGroups(reviews) {
    const query = reviewSearchTerm.trim().toLowerCase();
    const groups = [];
    const byShop = new Map();
    for (const review of reviews) {
        if (query && !reviewMatchesSearch(review, query)) continue;
        const shopKey = review.shop_id || 'unknown-shop';
        if (!byShop.has(shopKey)) {
            const group = {
                shopId: shopKey,
                shopName: review.shop_name || review.shop_id || 'Unknown shop',
                reviews: []
            };
            byShop.set(shopKey, group);
            groups.push(group);
        }
        byShop.get(shopKey).reviews.push(review);
    }

    if (!groups.length) {
        return `<div class="empty-list">No reviews match the current search.</div>`;
    }

    return groups.map(group => {
        const pendingCount = group.reviews.filter(review => review.status === 'PENDING_REVIEW').length;
        const productSchemaCount = group.reviews.filter(review => review.artifact_type === 'PRODUCT_SCHEMA').length;
        const collapsed = collapsedShopGroups.has(group.shopId);
        return `<div class="shop-group">
          <div class="shop-group-head" onclick="toggleShopGroup('${escapeHtmlAttr(group.shopId)}')">
            <div>
              <div class="queue-title">${escapeHtml(group.shopName)}</div>
              <div class="queue-meta">${productSchemaCount} schema review${productSchemaCount === 1 ? '' : 's'} / ${group.reviews.length} total</div>
            </div>
            <div class="shop-group-actions">
              ${statusBadge(`${pendingCount} pending`)}
              <span class="collapse-indicator">${collapsed ? '+' : '-'}</span>
            </div>
          </div>
          <div class="shop-review-list ${collapsed ? 'collapsed' : ''}">
            ${group.reviews.map(renderReviewQueueItem).join('')}
          </div>
        </div>`;
    }).join('');
}

function reviewMatchesSearch(review, query) {
    return [
        review.shop_name,
        review.shop_id,
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

function toggleShopGroup(shopId) {
    if (collapsedShopGroups.has(shopId)) {
        collapsedShopGroups.delete(shopId);
    } else {
        collapsedShopGroups.add(shopId);
    }
    localStorage.setItem('crawlerReviewCollapsedShopGroups', JSON.stringify([...collapsedShopGroups]));
    loadReviews().catch(err => document.getElementById('reviewList').textContent = err);
}

function renderReviewQueueItem(r) {
    return `<div class="item ${r.review_id === selectedId ? 'active' : ''}" onclick="selectReview('${r.review_id}')">
      <div class="queue-row">
        <span class="queue-artifact">${escapeHtml(displayName(r.artifact_type))}</span>
        ${statusBadge(r.status)}
      </div>
      <div class="queue-meta">${escapeHtml(displayName(r.reason))}</div>
      <div class="queue-meta">${timeBadge(r.created, 'Created')}</div>
    </div>`;
}

async function selectReview(id) {
    selectedId = id;
    dirty = false;
    activeWorkbenchPanel = 'selector';
    await loadReviews();
    await loadSelectedReview(true);
}

async function loadSelectedReview(resetSelection) {
    if (!selectedId) return;
    const detail = await api(`/api/reviews/${selectedId}`);
    const matrix = detail.review.artifact_type === 'PRODUCT_SCHEMA'
        ? await api(`/api/reviews/${selectedId}/matrix`)
        : null;
    selectedDetail = detail;
    selectedMatrix = matrix;
    selectedReviewUpdatedAt = detail.review.updated;
    selectedReviewStatus = detail.review.status;
    selectedReviewNeedsRefresh = false;
    selectedReviewRefreshReason = '';
    if (resetSelection) {
        selectedSchemaIndex = 0;
        compareSchemaIndex = 1;
        selectedField = 'title';
        selectedPageId = (detail.pages && detail.pages[0] && detail.pages[0].review_page_id) || null;
    }
    normalizeSchemaSelection();
    renderDetail(detail, matrix);
}

function renderDetail(detail, matrix) {
    const review = detail.review;
    const urls = detail.urls || [];
    document.getElementById('detail').innerHTML = `
    <div class="review-page">
      <div id="reviewRefreshBanner" class="refresh-banner" hidden></div>
      <div class="review-topbar">
        <div>
          <div class="review-title">
            <strong>${escapeHtml(review.shop_name || review.shop_id)}</strong>
            ${statusBadge(review.status)}
            ${statusBadge(review.artifact_type)}
          </div>
          <div class="review-meta">
            <span>${escapeHtml(displayName(review.reason))}</span>
            <span>${escapeHtml(review.review_id)}</span>
            ${timeBadge(review.updated, 'Updated')}
          </div>
        </div>
        ${renderApprovalPanel()}
      </div>
      <div class="review-card command-card">
        <div class="panel-head">
          <div class="panel-title">
            <strong>Review Controls</strong>
            <span class="muted">Manual actions for this shop</span>
          </div>
          <div class="actions">
            <button onclick="refreshSelectedReview()">Refresh review</button>
            <button onclick="triggerAction('trigger-crawl')">Trigger crawl</button>
            <button onclick="triggerAction('trigger-scrape')">Trigger scrape</button>
            <button onclick="triggerAction('regenerate-schema')">Regenerate schema</button>
            <button onclick="triggerAction('regenerate-pattern')">Regenerate pattern</button>
          </div>
        </div>
      </div>
      ${matrix ? renderSchemaWorkbench(detail, matrix) : renderNonSchemaReview(review, urls)}
    </div>`;
    if (matrix) postHighlightSoon();
    updateRefreshBanner();
    updateReloadState();
}

function renderApprovalPanel() {
    return `<div class="approval-actions">
      <button class="primary" onclick="approveReview()">Approve</button>
      <button class="danger" onclick="rejectReview()">Reject</button>
      <button onclick="needsRepair()">Needs repair</button>
    </div>`;
}

function renderNonSchemaReview(review, urls) {
    return `<div class="review-card">
      <div class="tab-panel">
        ${urls.length ? renderUrls(urls) : ''}
        <h3>Candidate Payload</h3>
        <textarea id="candidatePayload" oninput="markDirty()">${escapeHtml(JSON.stringify(review.candidate_payload, null, 2))}</textarea>
        <div class="actions"><button onclick="saveCandidate()">Save full JSON</button></div>
        <h3>Validation</h3>
        <pre>${escapeHtml(JSON.stringify(review.validation_summary, null, 2))}</pre>
      </div>
    </div>`;
}

function renderUrls(urls) {
    return `<h3>URL Pattern Preview</h3><div class="data-grid-wrap"><table><thead><tr><th>URL</th><th>Current</th><th>Candidate</th><th>Class</th></tr></thead><tbody>${
        urls.map(u => `<tr><td>${linkifyText(u.url)}</td><td>${u.current_pattern_match}</td><td>${u.candidate_pattern_match}</td><td>${escapeHtml(u.candidate_class)}</td></tr>`).join('')
    }</tbody></table></div>`;
}

function renderSchemaWorkbench(detail, matrix) {
    const pages = detail.pages || [];
    const schemas = currentSchemas();
    const page = pages.find(p => p.review_page_id === selectedPageId) || pages[0];
    if (page && !selectedPageId) selectedPageId = page.review_page_id;
    return `
    <div class="schema-workbench">
      <div class="preview-panel">
        <div class="panel-head">
          <div class="panel-title">
            <strong>Product View</strong>
            <span class="muted">${page ? `${productPageLink(page.url)} (${escapeHtml(page.role)})` : 'No saved page snapshot'}</span>
          </div>
          <div class="toolbar">
            <select onchange="selectedPageId=this.value; rerenderWorkbench()">
              ${pages.map(p => `<option value="${p.review_page_id}" ${p.review_page_id === selectedPageId ? 'selected' : ''}>${escapeHtml(p.role)}</option>`).join('')}
            </select>
          </div>
        </div>
        ${page ? `<iframe id="snapshotFrame" src="/api/review-pages/${page.review_page_id}/inspect" sandbox="allow-scripts"></iframe>` : ''}
      </div>
      <div class="right-panel">
        <div class="tabs">
          ${workbenchTab('selector', 'Selector')}
          ${workbenchTab('data', 'Extracted Data')}
          ${workbenchTab('diff', 'Schema Diff')}
          ${workbenchTab('json', 'JSON')}
        </div>
        ${renderWorkbenchPanel(detail, matrix, schemas)}
      </div>
    </div>`;
}

function workbenchTab(key, label) {
    return `<button class="tab ${activeWorkbenchPanel === key ? 'active' : ''}" onclick="setWorkbenchPanel('${key}')">${label}</button>`;
}

function renderWorkbenchPanel(detail, matrix, schemas) {
    if (activeWorkbenchPanel === 'data') return renderDataPanel(matrix);
    if (activeWorkbenchPanel === 'diff') return renderDiffPanel(schemas);
    if (activeWorkbenchPanel === 'json') return renderJsonPanel(detail);
    return renderSelectorPanel(schemas);
}

function renderSelectorPanel(schemas) {
    return `<div class="tab-panel">
      ${renderSchemaOrderControls(schemas)}
      <div class="field-grid">
        <label>Schema
          <select onchange="selectedSchemaIndex=Number(this.value); normalizeSchemaSelection(); rerenderWorkbench()">
            ${schemas.map((_, i) => `<option value="${i}" ${i === selectedSchemaIndex ? 'selected' : ''}>Schema ${i + 1} / ${schemas.length}</option>`).join('')}
          </select>
        </label>
        <label>Field
          <select onchange="selectedField=this.value; rerenderWorkbench()">
            ${selectorFields.map(field => `<option value="${field}" ${field === selectedField ? 'selected' : ''}>${displayName(field)}</option>`).join('')}
          </select>
        </label>
      </div>
      <div>
        <h3>Selected Selector</h3>
        <div class="selector-box">
          <input id="selectedSelector" value="${escapeHtmlAttr(currentSelector())}" oninput="setCurrentSelector(this.value); postHighlight(this.value)">
          <button class="primary" onclick="saveCurrentField()">Save field</button>
          <button class="danger" ${optionalFields.has(selectedField) ? '' : 'disabled'} onclick="deleteCurrentRule()">Delete rule</button>
        </div>
      </div>
      <div class="actions">
        <button onclick="applyPickedSelector()">Use clicked element</button>
        <button onclick="postHighlight(currentSelector())">Highlight field</button>
        <button onclick="postHighlightSchema()">Highlight schema</button>
        <button onclick="reloadMatrix()">Re-evaluate</button>
        <button onclick="discardEdits()">Discard edits</button>
      </div>
      <div class="picked" id="pickedElement">No element picked yet.</div>
      <h3>Schema Highlight Legend</h3>
      ${renderSchemaLegend(currentSchema())}
    </div>`;
}

function renderSchemaOrderControls(schemas) {
    if (!schemas.length) return '<div class="hint-card">No schemas available for ordering.</div>';
    return `<div class="schema-order-card">
      <div>
        <h3>Schema Order</h3>
        <div class="muted">Approval writes schemas in this order for the shop.</div>
      </div>
      <div class="schema-order-list">
        ${schemas.map((_, i) => `<button class="schema-chip ${i === selectedSchemaIndex ? 'active' : ''}" onclick="selectedSchemaIndex=${i}; normalizeSchemaSelection(); rerenderWorkbench()">Schema ${i + 1}</button>`).join('')}
      </div>
      <div class="schema-order-actions">
        <button ${selectedSchemaIndex === 0 ? 'disabled' : ''} onclick="moveCurrentSchema(-1)">Move up</button>
        <button ${selectedSchemaIndex >= schemas.length - 1 ? 'disabled' : ''} onclick="moveCurrentSchema(1)">Move down</button>
      </div>
    </div>`;
}

function renderDataPanel(matrix) {
    return `<div class="tab-panel">
      <div class="panel-title">
        <strong>Extracted Data Matrix</strong>
        <span class="muted">Raw extraction output for the selected schema</span>
      </div>
      ${renderApplyErrors(matrix)}
      ${renderExtractedRows(matrix)}
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
            ${schemas.map((_, i) => `<option value="${i}" ${i === selectedSchemaIndex ? 'selected' : ''}>Schema ${i + 1}</option>`).join('')}
          </select>
        </label>
        <label>Compare with
          <select onchange="compareSchemaIndex=Number(this.value); rerenderWorkbench()">
            ${schemas.map((_, i) => `<option value="${i}" ${i === compareSchemaIndex ? 'selected' : ''} ${i === selectedSchemaIndex ? 'disabled' : ''}>Schema ${i + 1}</option>`).join('')}
          </select>
        </label>
      </div>
      ${renderSchemaDiff(schemas[selectedSchemaIndex], schemas[compareSchemaIndex])}
    </div>`;
}

function renderJsonPanel(detail) {
    return `<div class="tab-panel">
      <div class="panel-title">
        <strong>Full Candidate JSON</strong>
        <span class="muted">Use field controls for normal edits; this is the raw payload.</span>
      </div>
      <textarea id="candidatePayload" oninput="markDirty()">${escapeHtml(JSON.stringify(detail.review.candidate_payload, null, 2))}</textarea>
      <div class="actions">
        <button onclick="saveCandidate()">Save full JSON</button>
        <button onclick="discardEdits()">Discard edits</button>
      </div>
      <h3>Validation</h3>
      <pre>${escapeHtml(JSON.stringify(detail.review.validation_summary, null, 2))}</pre>
    </div>`;
}

function renderApplyErrors(matrix) {
    const candidate = selectedCandidate(matrix);
    if (!candidate) return '';
    const failures = candidate.pages.filter(page => !page.apply_ok);
    if (!failures.length) return '';
    return `<div class="error-panel">
    <strong>Schema ${selectedSchemaIndex + 1} failed on ${failures.length} saved page${failures.length === 1 ? '' : 's'}</strong>
    ${failures.map(page => `<div>${escapeHtml(displayName(page.role))}: ${escapeHtml(page.error || 'Schema did not apply.')}</div>`).join('')}
  </div>`;
}

function renderExtractedRows(matrix) {
    const candidate = selectedCandidate(matrix);
    if (!candidate) return '<div class="muted">No extracted data for this schema.</div>';
    return `<div class="data-grid-wrap"><table class="data-grid raw-data-grid"><thead><tr><th>Page</th><th>Status</th><th>Raw Extracted Data</th></tr></thead><tbody>${
        candidate.pages.map(page => `
      <tr class="${page.apply_ok ? '' : 'failed'}">
        <td>${escapeHtml(displayName(page.role))}</td>
        <td><span class="${page.apply_ok ? 'status-ok badge' : 'status-failed badge'}">${page.apply_ok ? 'OK' : 'Failed'}</span></td>
        <td>${page.apply_ok ? rawDataCell(page.extracted) : expandableValue(page.error || 'Schema did not apply.')}</td>
      </tr>`).join('')
    }</tbody></table></div>`;
}

function selectedCandidate(matrix) {
    return matrix.candidates.find(c => c.schema_index === selectedSchemaIndex);
}

function rawDataCell(raw) {
    if (!raw) return '<span class="muted">No extracted data.</span>';
    const fields = Object.entries(raw).filter(([, value]) => hasRawValue(value));
    if (!fields.length) return '<span class="muted">No extracted data.</span>';
    return `<dl class="raw-data">${
        fields.map(([key, value]) => `
      <div>
        <dt>${escapeHtml(displayName(key))}</dt>
        <dd>${rawValue(value)}</dd>
      </div>`).join('')
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
    const fields = [...selectorFields, 'default_currency'];
    return `<div class="schema-diff">${
        fields.map(field => {
            const left = normalizeRuleValue(current ? current[field] : undefined);
            const right = normalizeRuleValue(compare ? compare[field] : undefined);
            const status = diffStatus(left, right);
            return `<div class="diff-row">
              <div class="diff-field">${escapeHtml(displayName(field))}</div>
              ${statusBadge(status)}
              <div class="diff-values">
                <div class="diff-value"><span class="diff-label">Selected</span>${expandableValue(left || 'not configured')}</div>
                <div class="diff-value"><span class="diff-label">Compare</span>${expandableValue(right || 'not configured')}</div>
              </div>
            </div>`;
        }).join('')
    }</div>`;
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
    await loadSelectedReview(false);
    await loadReviews();
}

async function refreshSelectedReview() {
    dirty = false;
    await loadSelectedReview(false);
    await loadReviews();
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
    return currentSchema()[selectedField] || null;
}

function currentSelector() {
    return (currentRule() || {}).selector || '';
}

function setCurrentSelector(selector) {
    const payload = schemasPayload();
    payload.schemas = payload.schemas || [];
    payload.schemas[selectedSchemaIndex] = payload.schemas[selectedSchemaIndex] || {};
    payload.schemas[selectedSchemaIndex][selectedField] = payload.schemas[selectedSchemaIndex][selectedField] || defaultRuleFor(selectedField);
    payload.schemas[selectedSchemaIndex][selectedField].selector = selector;
    const textarea = document.getElementById('candidatePayload');
    if (textarea) textarea.value = JSON.stringify(payload, null, 2);
    selectedDetail.review.candidate_payload = payload;
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

function applyPickedSelector() {
    if (!pickedSelector) return;
    setCurrentSelector(pickedSelector);
    const input = document.getElementById('selectedSelector');
    if (input) input.value = pickedSelector;
    postHighlight(pickedSelector);
}

async function saveCurrentField() {
    const rule = (schemasPayload().schemas[selectedSchemaIndex] || {})[selectedField] || null;
    await api(`/api/reviews/${selectedId}/schema-field`, {
        method: 'POST',
        body: JSON.stringify({schema_index: selectedSchemaIndex, field: selectedField, rule})
    });
    dirty = false;
    await loadSelectedReview(false);
    await loadReviews();
}

async function deleteCurrentRule() {
    if (!optionalFields.has(selectedField)) return;
    await api(`/api/reviews/${selectedId}/schema-field`, {
        method: 'POST',
        body: JSON.stringify({schema_index: selectedSchemaIndex, field: selectedField, rule: null})
    });
    dirty = false;
    await loadSelectedReview(false);
    await loadReviews();
}

function postHighlight(selector, label = displayName(selectedField)) {
    const frame = document.getElementById('snapshotFrame');
    if (frame && frame.contentWindow) frame.contentWindow.postMessage({
        type: 'crawler-review-highlight-selector',
        selector,
        label,
        color: schemaHighlightColors[label] || '#2563eb'
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
    return rules;
}

function postHighlightSoon() {
    setTimeout(() => {
        if (activeWorkbenchPanel === 'selector') postHighlight(currentSelector());
    }, 350);
}

async function approveReview() {
    await api(`/api/reviews/${selectedId}/approve`, {
        method: 'POST',
        body: JSON.stringify({notes: prompt('Notes') || null})
    });
    await loadReviews();
    await loadSelectedReview(false);
}

async function rejectReview() {
    await api(`/api/reviews/${selectedId}/reject`, {
        method: 'POST',
        body: JSON.stringify({notes: prompt('Notes') || null})
    });
    await loadReviews();
    await loadSelectedReview(false);
}

async function needsRepair() {
    await api(`/api/reviews/${selectedId}/needs-repair`, {
        method: 'POST',
        body: JSON.stringify({notes: prompt('Repair notes') || null})
    });
    await loadReviews();
    await loadSelectedReview(false);
}

async function saveCandidate() {
    const payload = JSON.parse(document.getElementById('candidatePayload').value);
    await saveCandidatePayload(payload);
}

async function saveCandidatePayload(payload) {
    await api(`/api/reviews/${selectedId}/candidate`, {method: 'POST', body: JSON.stringify(payload)});
    dirty = false;
    await loadSelectedReview(false);
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

async function discardEdits() {
    dirty = false;
    await loadSelectedReview(false);
}

async function triggerAction(action) {
    const detail = await api(`/api/reviews/${selectedId}`);
    const shopId = detail.review.shop_id;
    const result = await api(`/api/shops/${shopId}/${action}`, {method: 'POST', body: '{}'});
    alert(`${action}: ${result.affected} rows affected`);
}

function markDirty() {
    dirty = true;
    updateReloadState();
}

function updateReloadState() {
    const state = document.getElementById('reloadState');
    if (dirty) {
        state.textContent = 'Review has local edits';
        state.className = 'system-status dirty';
    } else if (selectedReviewNeedsRefresh) {
        state.textContent = 'Selected review changed elsewhere - refresh to update';
        state.className = 'system-status stale';
    } else {
        state.textContent = 'Queue auto refresh active';
        state.className = 'system-status';
    }
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
        toggle.setAttribute('aria-label', collapsed ? 'Expand pending reviews' : 'Collapse pending reviews');
        toggle.setAttribute('title', collapsed ? 'Expand pending reviews' : 'Collapse pending reviews');
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
    } else if (isNewerTimestamp(current.updated, selectedReviewUpdatedAt)) {
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

function statusBadge(status) {
    const normalized = String(status || '').toLowerCase();
    let cls = 'badge';
    if (normalized.includes('pending')) cls += ' badge-pending';
    else if (normalized.includes('approved') || normalized === 'same') cls += ' badge-approved';
    else if (normalized.includes('changed')) cls += ' badge-changed';
    else if (normalized.includes('missing')) cls += ' badge-missing';
    else if (normalized.includes('reject') || normalized.includes('failed')) cls += ' badge-rejected';
    else if (normalized.includes('repair') || normalized.includes('superseded')) cls += ' badge-repair';
    return `<span class="${cls}">${escapeHtml(displayName(status))}</span>`;
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
    return `<span class="time-badge" title="${escapeHtmlAttr(`${label}: ${formatDate(value)}`)}">${clockIcon()}${escapeHtml(elapsedTime(value))}</span>`;
}

function clockIcon() {
    return `<svg aria-hidden="true" viewBox="0 0 24 24"><circle cx="12" cy="12" r="9"></circle><path d="M12 7v5l3 2"></path></svg>`;
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
    const data = event.data || {};
    if (data.type !== 'crawler-review-selector-picked') return;
    pickedSelector = data.selector || '';
    const sample = data.text || data.src || data.href || '';
    const picked = document.getElementById('pickedElement');
    if (!picked) return;
    picked.innerHTML = `
    <strong>${escapeHtml(pickedSelector)}</strong>
    <div class="muted">${escapeHtml(data.tag || '')} ${escapeHtml(sample)}</div>`;
});

setInterval(async () => {
    try {
        await loadReviews();
    } catch (err) {
        document.getElementById('reloadState').textContent = String(err);
    }
}, 5000);

loadReviews().catch(err => document.getElementById('reviewList').textContent = err);
setSidebarCollapsed(sidebarCollapsed);
