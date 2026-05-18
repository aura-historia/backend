(() => {
    const selected = new Set();
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
        for (const element of selected) element.classList.remove('__crawler_review_selected');
        selected.clear();
    }

    function highlight(selector) {
        clearSelected();
        if (!selector) return;
        try {
            document.querySelectorAll(selector).forEach(element => {
                element.classList.add('__crawler_review_selected');
                selected.add(element);
            });
        } catch (_) {
        }
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
        highlight(selector);
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
        if (event.data && event.data.type === 'crawler-review-highlight-selector') {
            highlight(event.data.selector);
        }
    });
})();
