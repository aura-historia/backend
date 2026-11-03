#!/usr/bin/env python3
"""Generates the three review-detail mockups from one shell: selector tab with the
foreign-currency note, data tab with coverage/checks/notes, and the rule kinds."""
import pathlib

HERE = pathlib.Path(__file__).parent
CHEV = '<svg width="16" height="16" viewBox="0 0 16 16"><path d="M4 6l4 4 4-4" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>'
EXT = '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 4h6v6M20 4l-9 9M19 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1h5"/></svg>'
EYE = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/></svg>'
X = '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M6 6l12 12M18 6L6 18"/></svg>'

SHELL = """<!doctype html>
<html lang="en" data-theme="light">
<head>
<meta charset="utf-8">
<title>{title} — Crawler review (mockup)</title>
<link rel="stylesheet" href="console.css">
</head>
<body>
<header class="app-header">
    <h1>Crawler review</h1>
    <nav class="view-switch" aria-label="View">
        <a class="view-tab" href="#sources">Sources <span class="badge badge-warn">2</span></a>
        <a class="view-tab active" href="#reviews" aria-current="page">Reviews <span class="badge badge-warn">1</span></a>
    </nav>
    <div class="header-actions">
        <span class="system-status {status_class}" role="status">{status}</span>
        <button class="icon quiet" title="Switch to dark mode" aria-label="Switch to dark mode">
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M21 12.8A9 9 0 1 1 11.2 3 7 7 0 0 0 21 12.8z"/></svg>
        </button>
    </div>
</header>
<main class="app">
    <section class="sidebar">
        <div class="sidebar-head">
            <input type="search" placeholder="Search reviews" aria-label="Search reviews">
            <button class="icon quiet" title="Collapse the queue" aria-label="Collapse the queue"><svg width="16" height="16" viewBox="0 0 16 16"><path d="M10 4l-4 4 4 4" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg></button>
        </div>
        <div class="queue">
            <div class="group">
                <button class="group-head" aria-expanded="true">
                    <span class="chev">__CHEV__</span>
                    <span class="group-text">
                        <span class="nav-row"><span class="nav-title">Robert Young Antiques</span><span class="badge badge-warn">1 pending</span></span>
                        <span class="nav-meta">1 product schema · 1 URL pattern</span>
                    </span>
                </button>
                <div class="group-list">
                    <a class="nav-item active" href="#" aria-current="true">
                        <span class="nav-row"><span class="nav-title">Product schema</span><span class="badge badge-warn">Pending</span></span>
                        <span class="nav-meta">Manual schema required · 3m ago</span>
                    </a>
                    <a class="nav-item" href="#">
                        <span class="nav-row"><span class="nav-title">URL pattern</span><span class="badge badge-ok">Approved</span></span>
                        <span class="nav-meta">Manual URL pattern required · 4m ago</span>
                    </a>
                </div>
            </div>
            <div class="group">
                <button class="group-head" aria-expanded="false">
                    <span class="chev closed">__CHEV__</span>
                    <span class="group-text">
                        <span class="nav-row"><span class="nav-title">Holt Antique Furniture</span><span class="nav-meta">Reviewed</span></span>
                        <span class="nav-meta">2 product schemas · 1 URL pattern</span>
                    </span>
                </button>
            </div>
            <div class="group">
                <button class="group-head" aria-expanded="false">
                    <span class="chev closed">__CHEV__</span>
                    <span class="group-text">
                        <span class="nav-row"><span class="nav-title">Martlesham Antiques</span><span class="nav-meta">Reviewed</span></span>
                        <span class="nav-meta">1 product schema · 1 URL pattern</span>
                    </span>
                </button>
            </div>
            <div class="group">
                <button class="group-head" aria-expanded="false">
                    <span class="chev closed">__CHEV__</span>
                    <span class="group-text">
                        <span class="nav-row"><span class="nav-title">Loveday Antiques</span><span class="nav-meta">Reviewed</span></span>
                        <span class="nav-meta">1 product schema · 1 URL pattern</span>
                    </span>
                </button>
            </div>
        </div>
    </section>
    <section class="detail-shell">
        <div class="detail">
            <div class="page">
                <div class="page-head">
                    <div>
                        <div class="page-title"><h1>Robert Young Antiques</h1><span class="badge badge-warn">Pending</span><span class="badge">Product schema</span></div>
                        <div class="page-meta"><span>Manual schema required</span><span>·</span><span>opened <span class="time" title="3 Nov 2026, 19:41">4m ago</span></span><span>·</span><span>edited <span class="time" title="3 Nov 2026, 19:43">1m ago</span></span></div>
                    </div>
                    <div class="page-actions">
                        <button>Needs repair</button>
                        <button class="danger">Reject</button>
                        <button class="primary">Approve</button>
                    </div>
                </div>

                {notices}

                <details class="card history">
                    <summary><span class="chev closed">__CHEV__</span><strong>History</strong><span>3 events · latest: candidate edited <span class="time" title="3 Nov 2026, 19:43">3m ago</span> by a human</span></summary>
                </details>

                <div class="workbench">
                    <div class="card preview">
                        <div class="preview-bar">
                            <div class="preview-row">
                                <label for="pageSelect">Page</label>
                                <select id="pageSelect" aria-label="Review page">
                                    <option>Reviewer added · /products/137015-a-fine-louis-xv-provincial-three-drawer-commode</option>
                                    <option>Primary · /products/19th-century-english-naive-school-miniature-4</option>
                                    <option>Seed · /products/the-crucifixion</option>
                                </select>
                                <button class="quiet danger">Remove page</button>
                            </div>
                            <div class="preview-row">
                                <input class="mono" value="https://www.robertyoungantiques.com/products/137015-a-fine-louis-xv-provincial-three-drawer-commode" aria-label="Preview URL">
                                <button class="icon" title="Open in a new tab" aria-label="Open in a new tab">__EXT__</button>
                                <button>Add to review</button>
                            </div>
                        </div>
                        <div class="frame">
                            <div class="shop">
                                <div class="logo">ROBERT<br>YOUNG<br>ANTIQUES</div>
                                <div class="crumbs">Collection: <u>All Collections</u></div>
                                <div class="img"></div>
                                <h2>A FINE LOUIS XV PROVINCIAL THREE DRAWER COMMODE</h2>
                                <div class="price"><span class="{price_hl}">£4,800</span></div>
                                <div class="cta {state_hl}">SEND</div>
                            </div>
                        </div>
                    </div>
                    <div class="card inspector">
                        <div class="tabs">
                            <button class="tab {t_sel}">Selector</button>
                            <button class="tab {t_data}">Data</button>
                            <button class="tab">Diff</button>
                            <button class="tab">Removed pages</button>
                            <button class="tab">JSON</button>
                        </div>
                        <div class="tab-panel">
                        {panel}
                        </div>
                    </div>
                </div>
            </div>
        </div>
    </section>
</main>
</body>
</html>
"""

SCHEMAS = """<div class="section">
    <div class="section-head"><h3>Schemas</h3><span class="small muted">applied in this order</span></div>
    <div class="chips"><button class="chip">Schema 1</button><button class="chip active">Schema 2</button><button class="chip add">+ Add schema</button></div>
    <div class="toolbar"><span class="small muted" style="margin-right:6px">Schema 2:</span><button class="quiet">Move up</button><button class="quiet" disabled>Move down</button><button class="quiet danger">Delete</button><span style="flex:1"></span><button class="quiet" title="Highlight every field of this schema in the preview">Highlight all</button></div>
</div>"""

PICKED = """<div class="picked">
    <div class="picked-head"><span>Picked in the preview</span></div>
    <code>{sel}</code>
    <span class="sample">{sample}</span>
    <div class="actions"><button>Use as selector</button><button>Add as fallback</button></div>
</div>"""

SELECTOR_PRICE = SCHEMAS + """
<div class="section">
    <div class="section-head"><h3>Rule</h3></div>
    <label class="field">Field
        <select><option>Source listing ID</option><option>Title</option><option>Description</option><option selected>Price · note</option><option>Price estimate min</option><option>Price estimate max</option><option>State</option><option>Images</option></select>
    </label>
    <label class="field">Selector
        <span class="selector-row"><input class="mono" value='script[type="application/ld+json"]'><button class="icon" title="Highlight in the preview" aria-label="Highlight in the preview">__EYE__</button></span>
    </label>
    <div class="notice notice-warn field-note">
        <span class="notice-kind">Note</span>
        <span>Price names <strong>GBP</strong>, not the fallback currency EUR, on this page. The worker reads the currency the offer states; this blocks no approval.</span>
    </div>
    <div class="rule-grid">
        <label class="field">Matches<select><option>First match</option><option>All matches</option></select></label>
        <label class="field">Read<select><option>Text</option><option>Attribute</option><option>Image URL</option><option>Attribute or text</option><option selected>JSON-LD offer</option></select></label>
        <label class="field">Offer property<select><option>Availability</option><option selected>Price</option></select></label>
    </div>
    <div>
        <h4>Fallback selectors</h4>
        <div class="fallbacks" style="margin-top:8px">
            <div class="fallback"><input class="mono" value=".product__price .money"><button class="icon" title="Highlight" aria-label="Highlight">__EYE__</button><button class="icon danger" title="Remove" aria-label="Remove">__X__</button></div>
        </div>
        <div class="actions" style="margin-top:8px"><button class="quiet">+ Add fallback</button></div>
    </div>
    <div class="actions"><button class="primary-outline">Save field</button><button class="quiet danger">Delete rule</button><span style="flex:1"></span><button class="quiet">Discard edits</button></div>
    """ + PICKED.format(sel='script[type="application/ld+json"]', sample='“{"@context":"https://schema.org","@type":"Product","offers":{"price":"4800.00","priceCurrency":"GBP"…”') + """
</div>
<div class="section">
    <div class="section-head"><h3>Raw attributes</h3></div>
    <div class="selector-row"><select><option>dimensions</option><option>period</option><option>provenance</option></select><button>Add</button></div>
</div>"""

SELECTOR_STATE = SCHEMAS + """
<div class="section">
    <div class="section-head"><h3>Rule</h3></div>
    <label class="field">Field
        <select><option>Source listing ID · failed check</option><option>Title</option><option>Description</option><option>Price · note</option><option>Price estimate min</option><option>Price estimate max</option><option selected>State</option><option>Images</option></select>
    </label>
    <label class="field">Selector
        <span class="selector-row"><input class="mono" value='#contact_form button[type="submit"]'><button class="icon" title="Highlight in the preview" aria-label="Highlight in the preview">__EYE__</button></span>
    </label>
    <div class="rule-grid">
        <label class="field">Matches<select><option>First match</option><option>All matches</option></select></label>
        <label class="field rel">Read<select><option>Text</option><option>Attribute</option><option>Image URL</option><option selected>Attribute or text</option><option>JSON-LD offer</option></select>
            <div class="faux-menu" aria-hidden="true">
                <div><strong>Text</strong><span>The element's visible text</span></div>
                <div><strong>Attribute</strong><span>One named attribute, such as href or content</span></div>
                <div><strong>Image URL</strong><span>The best full-size image address of each element</span></div>
                <div class="active"><strong>Attribute or text</strong><span>The attribute when not blank, else the text (a submit button's value or label)</span></div>
                <div><strong>JSON-LD offer</strong><span>availability or price from the offers in a ld+json script</span></div>
            </div>
        </label>
        <label class="field wide">Attribute name<input value="value"></label>
    </div>
    <div style="height:240px"></div>
    <div>
        <h4>Fallback selectors</h4>
        <div class="actions" style="margin-top:8px"><button class="quiet">+ Add fallback</button></div>
    </div>
    <div class="actions"><button class="primary-outline">Save field</button><button class="quiet danger" disabled>Delete rule</button></div>
    """ + PICKED.format(sel='#contact_form button[type="submit"]', sample='“Send”') + """
</div>"""

DATA = """<div class="section">
    <div class="section-head"><h3>Coverage</h3><span class="badge badge-ok">6 of 6 pages</span></div>
    <div class="small muted">Every live page is covered by at least one schema. Schema 2 applies to all 6; Schema 1 applies to none — add a page of its template or delete it.</div>
    <div class="actions"><button>Evaluate on live pages</button><span class="small muted">evaluated <span class="time" title="3 Nov 2026, 19:40">6m ago</span></span></div>
</div>
<div class="section">
    <div class="section-head"><h3><span class="dot dot-bad"></span>Failed checks</h3><span class="badge badge-bad">1 check · 6 pages</span></div>
    <div class="small muted">A failed check keeps a generated schema out of auto-approval; a human approves it knowingly.</div>
    <div class="check-list">
        <div class="check">
            <div class="check-head bad"><strong>Source listing ID is blank</strong><span class="small muted">Schema 2 · all 6 pages</span></div>
            <div class="check-pages">
                <span class="page-tag"><span class="role">Primary</span><code>/products/19th-century-english-naive-school-miniature-4</code></span>
                <span class="page-tag"><span class="role">Seed</span><code>/products/the-crucifixion</code></span>
                <span class="page-tag"><span class="role">Seed</span><code>/products/delightfully-graphic-rectangular-washboard-copy</code></span>
                <span class="page-tag"><span class="role">Seed</span><code>/products/engaging-silhouette-running-fox-weathervane</code></span>
                <span class="page-tag"><span class="role">Added</span><code>/products/137015-a-fine-louis-xv-provincial-three-drawer-commode</code></span>
                <span class="page-tag"><span class="role">Added</span><code>/products/333808-love-bird-decorated-armchair</code></span>
            </div>
        </div>
    </div>
</div>
<div class="section">
    <div class="section-head"><h3><span class="dot dot-warn"></span>Notes</h3><span class="badge badge-warn">1 note · 2 pages</span></div>
    <div class="small muted">Information next to its field; notes block no approval.</div>
    <div class="check-list">
        <div class="check">
            <div class="check-head warn"><strong>Price names GBP, not the fallback currency EUR</strong></div>
            <div class="check-pages">
                <span class="page-tag"><span class="role">Added</span><code>/products/137015-a-fine-louis-xv-provincial-three-drawer-commode</code></span>
                <span class="page-tag"><span class="role">Added</span><code>/products/333808-love-bird-decorated-armchair</code></span>
            </div>
        </div>
        <div class="check">
            <div class="small muted">Price left empty on 4 pages (matched only empty elements): Primary, Seed ×3. The page still applies.</div>
        </div>
    </div>
</div>
<div class="section">
    <div class="section-head"><h3>Extracted from this page</h3><span class="badge badge-bad">Failed · 1 check</span></div>
    <div class="small muted">Schema 2 · Reviewer added · <code>/products/137015-a-fine-louis-xv-provincial-three-drawer-commode</code></div>
    <dl class="kv">
        <dt class="bad">Source listing ID</dt><dd><span class="blank">blank</span> <span class="badge badge-bad">Blank</span></dd>
        <dt>Title</dt><dd>A Fine Louis XV Provincial Three Drawer Commode</dd>
        <dt>Price</dt><dd>4800.00 GBP <span class="badge badge-warn">Foreign currency</span></dd>
        <dt>State</dt><dd>Send</dd>
        <dt>Description</dt><dd><details class="expandable"><summary>A fine Louis XV provincial three drawer commode in walnut, the shaped top above three long drawers with original brass handles and escutcheons, raised on short cabriole legs<span class="ellipsis">…</span></summary><pre>A fine Louis XV provincial three drawer commode in walnut, the shaped top above three long drawers with original brass handles and escutcheons, raised on short cabriole legs. French, circa 1760. Height 86 cm, width 128 cm, depth 62 cm.</pre></details></dd>
        <dt>Images</dt><dd><ol class="image-list"><li title="https://www.robertyoungantiques.com/cdn/shop/files/137015_1.jpg?v=1730"><a href="#">…/137015_1.jpg</a> <span class="xs muted">+2 fallbacks</span></li><li><a href="#">…/137015_2.jpg</a> <span class="xs muted">+2 fallbacks</span></li><li><a href="#">…/137015_3.jpg</a></li><li><a href="#">…/137015_4.jpg</a></li><li><a href="#">…/137015_5.jpg</a></li><li class="more">3 more images</li></ol></dd>
    </dl>
</div>"""


EMPTY_DETAIL = '''<div class="empty-detail"><strong>No review selected</strong><span>Pick a review in the queue. 1 review is pending; 2 sources need you.</span><span class="actions"><a class="view-tab" href="#sources" style="border:1px solid var(--line-strong)">See what needs you</a></span></div>'''

def write(name, title, panel, t_sel='', t_data='', price_hl='', state_hl='', notices='', status='', status_class='', scroll=0, empty=False):
    if empty:
        html = SHELL.format(title=title, panel='', t_sel='', t_data='', price_hl='', state_hl='', notices='', status='', status_class='')
        start = html.index('<div class="detail">') + len('<div class="detail">')
        end = html.index('</section>\n</main>')
        html = html[:start] + EMPTY_DETAIL + '</div>\n    ' + html[end:]
        html = html.replace('<a class="nav-item active" href="#" aria-current="true">', '<a class="nav-item" href="#">')
        html = html.replace('__CHEV__', CHEV).replace('__EXT__', EXT).replace('__EYE__', EYE).replace('__X__', X)
        (HERE / name).write_text(html)
        return
    html = SHELL.format(title=title, panel=panel, t_sel=t_sel, t_data=t_data, price_hl=price_hl,
                        state_hl=state_hl, notices=notices, status=status, status_class=status_class)
    html = html.replace('<body>', f'<body data-scroll="{scroll}">')
    html = html.replace('__CHEV__', CHEV).replace('__EXT__', EXT).replace('__EYE__', EYE).replace('__X__', X)
    (HERE / name).write_text(html)


write('review-selector.html', 'Review · Selector', SELECTOR_PRICE, t_sel='active', price_hl='hl',
      status='Unsaved edits', status_class='dirty')
write('review-data.html', 'Review · Data', DATA, t_data='active')
write('review-data-below.html', 'Review · Data (scrolled)', DATA, t_data='active', scroll=700)
write('review-kinds.html', 'Review · Rule kinds', SELECTOR_STATE, t_sel='active', state_hl='hl state', scroll=560,
      notices='<div class="notice notice-warn"><span class="notice-kind">Not evaluated</span><span>2 pages were added to the review after the last evaluation (Added · /products/333808-love-bird-decorated-armchair and 1 more). Evaluate again before approving.</span><span class="actions"><button>Evaluate on live pages</button></span></div>')
write('reviews-empty.html', 'Reviews · nothing selected', '', empty=True)
print('ok')
