// Task Board A4 browser gate.
//
// Usage: node benchmarks/task_board_frontend.mjs <page-url> <expected-json>
//
// The page is served by a static file server; `/api/tasks` is fetched
// cross-origin from the native Briev backend. The backend sends the CORS
// headers, so this proves the real browser boundary rather than stubbing
// `fetch_url`.
import { chromium } from 'playwright';

const pageUrl = process.argv[2];
const expectedJson = process.argv[3];
if (!pageUrl || !expectedJson) {
    console.error('usage: node task_board_frontend.mjs <page-url> <expected-json>');
    process.exit(2);
}

const browser = await chromium.launch();
const page = await browser.newPage();
const consoleErrors = [];
const pageErrors = [];
page.on('console', (message) => {
    if (message.type() === 'error') consoleErrors.push(message.text());
});
page.on('pageerror', (error) => pageErrors.push(String(error)));

let failures = 0;
function check(name, ok, detail = '') {
    const passed = Boolean(ok);
    if (!passed) failures += 1;
    console.log(`${passed ? 'PASS' : 'FAIL'} ${name}${detail ? `: ${detail}` : ''}`);
}

await page.goto(pageUrl, { waitUntil: 'load' });
// The shim boots asynchronously; a b-window:load listener can miss the event.
// The visible Refresh button is the deterministic boundary trigger.
await page.locator('#rbv-button-1').click();

const status = page.locator('#rbv-span-0');
const tasks = page.locator('#rbv-pre-3');
let statusText = '';
let taskText = '';
for (let attempt = 0; attempt < 50; attempt += 1) {
    statusText = ((await status.textContent()) || '').trim();
    taskText = ((await tasks.textContent()) || '').trim();
    if (statusText === 'ready' && taskText === expectedJson) break;
    await page.waitForTimeout(100);
}

check('page loads without page errors', pageErrors.length === 0,
    pageErrors.join(' | '));
check('page loads without console errors', consoleErrors.length === 0,
    consoleErrors.join(' | '));
check('load trigger fetches the API', statusText === 'ready',
    `got=${JSON.stringify(statusText)}`);
check('task JSON reaches reactive view state', taskText === expectedJson,
    `got=${JSON.stringify(taskText)} want=${JSON.stringify(expectedJson)}`);

await browser.close();

if (failures > 0) {
    console.error(`task board frontend gate: ${failures} failure(s)`);
    process.exit(1);
}
console.log('task board frontend gate: PASS');
