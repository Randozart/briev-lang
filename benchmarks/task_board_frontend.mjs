// Task Board A6 structured browser CRUD gate.
//
// Usage: node benchmarks/task_board_frontend.mjs <page-url>
//
// The page is served by a static file server; all HTTP actions go cross-origin
// to the native Briev backend. This proves create/toggle/delete through the
// real browser host boundary and asserts the backend remains the source of
// truth after every mutation.
import { chromium } from 'playwright';

const pageUrl = process.argv[2];
if (!pageUrl) {
    console.error('usage: node task_board_frontend.mjs <page-url>');
    process.exit(2);
}

const browser = await chromium.launch();
const page = await browser.newPage();
const consoleErrors = [];
const pageErrors = [];
page.on('console', (message) => {
    if (message.type() === 'error') consoleErrors.push(message.text());
});
page.on('pageerror', (error) => pageErrors.push(error.stack || String(error)));

let failures = 0;
function check(name, ok, detail = '') {
    const passed = Boolean(ok);
    if (!passed) failures += 1;
    console.log(`${passed ? 'PASS' : 'FAIL'} ${name}${detail ? `: ${detail}` : ''}`);
}

async function taskRows() {
    return {
        ids: await page.locator('ul.task-list li .task-id').allTextContents(),
        titles: await page.locator('ul.task-list li .task-title').allTextContents(),
        done: await page.locator('ul.task-list li .task-done').allTextContents(),
    };
}

async function waitForRows(expectedCount, timeoutMs = 3000) {
    const deadline = Date.now() + timeoutMs;
    let rows = await taskRows();
    while (rows.ids.length !== expectedCount && Date.now() < deadline) {
        await page.waitForTimeout(50);
        rows = await taskRows();
    }
    return rows;
}

await page.goto(pageUrl, { waitUntil: 'load' });
// The shim boots asynchronously; a b-window:load listener can miss the event.
// The visible Refresh button is the deterministic boundary trigger.
await page.locator('#rbv-button-2').click();
let rows = await waitForRows(0);
check('page loads without page errors', pageErrors.length === 0,
    pageErrors.join(' | '));
check('page loads without console errors', consoleErrors.length === 0,
    consoleErrors.join(' | '));
check('initial GET renders an empty structured list', rows.ids.length === 0,
    `got=${JSON.stringify(rows)}`);

await page.locator('#rbv-input-4').fill('browser made this');
await page.locator('#rbv-button-5').click();
rows = await waitForRows(1);
check('POST creates a rendered task',
    rows.ids.join('|') === '1'
        && rows.titles.join('|') === 'browser made this'
        && rows.done.join('|') === 'false',
    `got=${JSON.stringify(rows)}`);

await page.locator('#rbv-input-6').fill('1');
await page.locator('#rbv-button-7').click();
const deadline = Date.now() + 3000;
while (Date.now() < deadline) {
    rows = await taskRows();
    if (rows.done.join('|') === 'true') break;
    await page.waitForTimeout(50);
}
check('toggle updates the rendered task',
    rows.ids.join('|') === '1'
        && rows.titles.join('|') === 'browser made this'
        && rows.done.join('|') === 'true',
    `got=${JSON.stringify(rows)}`);

await page.locator('#rbv-button-8').click();
rows = await waitForRows(0);
check('DELETE removes the rendered task', rows.ids.length === 0,
    `got=${JSON.stringify(rows)}`);

await browser.close();

if (failures > 0) {
    console.error(`task board frontend gate: ${failures} failure(s)`);
    process.exit(1);
}
console.log('task board frontend gate: PASS');
