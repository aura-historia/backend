(() => {
    const selected = new Map();
    const labels = [];
    let hover = null;

    function cssEscape(value) {
        if (window.CSS && CSS.escape) return CSS.escape(value);
        return String(value).replace(/[^a-zA-Z0-9_-]/g, ch => '\\' + ch);
    }

    function selectorFor(element) {
        if (!element || element.nodeType !== Node.ELEMENT_NODE) return '';
        if (element.id) return `#${cssEscape(element.id)}`;

        const parts = [];
        let current = element;
        while (current && current.nodeType === Node.ELEMENT_NODE && current !== document.body) {
            let part = current.localName.toLowerCase();
            const stableClasses = Array.from(current.classList || [])
                .filter(name => !name.startsWith('__crawler_review_'))
                .slice(0, 2);
            if (stableClasses.length) part += stableClasses.map(name => `.${cssEscape(name)}`).join('');

            const parent = current.parentElement;
            if (parent) {
                const siblings = Array.from(parent.children).filter(child => child.localName === current.localName);
                if (siblings.length > 1 && !stableClasses.length) {
                    part += `:nth-of-type(${siblings.indexOf(current) + 1})`;
                }
            }
            parts.unshift(part);
            const selector = parts.join(' > ');
            try {
                if (document.querySelectorAll(selector).length === 1) return selector;
            } catch (_) {
            }
            current = current.parentElement;
        }
        return parts.join(' > ');
    }

    function clearSelected() {
        for (const label of labels) label.remove();
        labels.length = 0;
        for (const [element, previous] of selected.entries()) {
            element.classList.remove('__crawler_review_selected');
            element.style.outline = previous.outline;
            element.style.outlineOffset = previous.outlineOffset;
            element.style.backgroundColor = previous.backgroundColor;
            element.style.boxShadow = previous.boxShadow;
        }
        selected.clear();
    }

    function remember(element) {
        if (selected.has(element)) return;
        selected.set(element, {
            outline: element.style.outline,
            outlineOffset: element.style.outlineOffset,
            backgroundColor: element.style.backgroundColor,
            boxShadow: element.style.boxShadow
        });
    }

    function highlight(selector, label, color) {
        clearSelected();
        if (!selector) return;
        try {
            document.querySelectorAll(selector).forEach(element => {
                remember(element);
                element.classList.add('__crawler_review_selected');
                if (label) addLabel(element, label, color || '#2563eb');
            });
        } catch (_) {
        }
    }

    function highlightSchema(rules) {
        clearSelected();
        if (!Array.isArray(rules)) return;
        for (const rule of rules) {
            const selectors = Array.isArray(rule.selectors) ? rule.selectors : [];
            const color = rule.color || '#2563eb';
            for (const selector of selectors) {
                try {
                    document.querySelectorAll(selector).forEach(element => {
                        remember(element);
                        element.style.outline = `3px solid ${color}`;
                        element.style.outlineOffset = '2px';
                        element.style.backgroundColor = `${color}1A`;
                        element.style.boxShadow = `0 0 0 9999px transparent`;
                        addLabel(element, rule.field || 'selector', color);
                    });
                } catch (_) {
                }
            }
        }
    }

    function addLabel(element, text, color) {
        const rect = element.getBoundingClientRect();
        if (!rect.width && !rect.height) return;
        const label = document.createElement('div');
        label.className = '__crawler_review_label';
        label.textContent = text;
        label.style.position = 'absolute';
        label.style.left = `${Math.max(0, rect.left + window.scrollX)}px`;
        label.style.top = `${Math.max(0, rect.top + window.scrollY - 22)}px`;
        label.style.zIndex = '2147483647';
        label.style.maxWidth = '220px';
        label.style.overflow = 'hidden';
        label.style.textOverflow = 'ellipsis';
        label.style.whiteSpace = 'nowrap';
        label.style.pointerEvents = 'none';
        label.style.padding = '3px 7px';
        label.style.borderRadius = '999px';
        label.style.background = color || '#2563eb';
        label.style.color = '#fff';
        label.style.font = '600 11px/1.3 ui-sans-serif, system-ui, sans-serif';
        label.style.boxShadow = '0 2px 8px rgba(15, 23, 42, 0.22)';
        document.body.appendChild(label);
        labels.push(label);
    }

    document.addEventListener('mouseover', event => {
        if (hover) hover.classList.remove('__crawler_review_hover');
        hover = event.target;
        hover.classList.add('__crawler_review_hover');
    }, true);

    document.addEventListener('mouseout', () => {
        if (hover) hover.classList.remove('__crawler_review_hover');
        hover = null;
    }, true);

    document.addEventListener('click', event => {
        event.preventDefault();
        event.stopPropagation();
        const selector = selectorFor(event.target);
        highlight(selector, 'picked', '#2563eb');
        window.parent.postMessage({
            type: 'crawler-review-selector-picked',
            selector,
            text: (event.target.innerText || event.target.textContent || '').trim().slice(0, 500),
            tag: event.target.localName,
            href: event.target.getAttribute && event.target.getAttribute('href'),
            src: event.target.getAttribute && event.target.getAttribute('src')
        }, '*');
    }, true);

    window.addEventListener('message', event => {
        if (event.source !== window.parent) return;
        if (event.data && event.data.type === 'crawler-review-highlight-selector') {
            highlight(event.data.selector, event.data.label, event.data.color);
        } else if (event.data && event.data.type === 'crawler-review-highlight-schema') {
            highlightSchema(event.data.rules);
        }
    });
})();
