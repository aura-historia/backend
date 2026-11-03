// Screenshots every mockup in light and dark at 1440x900, deviceScaleFactor 2.
const path = require('path');
const fs = require('fs');
const {chromium} = require('playwright');

const dir = __dirname;
const out = path.join(dir, '..', 'mockup-shots');
fs.mkdirSync(out, {recursive: true});
const files = process.argv.slice(2).length
    ? process.argv.slice(2)
    : fs.readdirSync(dir).filter(f => f.endsWith('.html'));

(async () => {
    const browser = await chromium.launch();
    const context = await browser.newContext({viewport: {width: 1440, height: 900}, deviceScaleFactor: 2});
    const page = await context.newPage();
    for (const file of files) {
        for (const theme of ['light', 'dark']) {
            await page.goto('file://' + path.join(dir, file));
            await page.evaluate(t => {
                document.documentElement.dataset.theme = t;
                const scroll = Number(document.body.dataset.scroll || 0);
                const shell = document.querySelector('.detail-shell');
                if (shell && scroll) shell.scrollTop = scroll;
            }, theme);
            await page.waitForTimeout(150);
            const name = `${path.basename(file, '.html')}-${theme}.png`;
            await page.screenshot({path: path.join(out, name)});
            console.log(name);
        }
    }
    await browser.close();
})();
