// Task Board A5 browser mutation gate.
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
page.on('pageerror', (error) => pageErrors.push(String(error)));

let failures = 0;
function check(name, ok, detail = '') {
    const passed = Boolean(ok);
    if (!passed) failures += 1;
    console.log(`${passed ? 'PASS' : 'FAIL'} ${name}${detail ? `: ${detail}` : ''}`);
}

async function tasksText() {
    return ((await page.locator('#rbv-pre-9').textContent()) || '').trim();
}

async function waitForTasks(expected, timeoutMs = 3000) {
    const deadline = Date.now() + timeoutMs;
    let text = await tasksText();
    while (text !== expected && Date.now() < deadline) {
        await page.waitForTimeout(50);
        text = await tasksText();
    }
    return text;
}

await page.goto(pageUrl, { waitUntil: 'load' });
// The shim boots asynchronously; a b-window:load listener can miss the event.
// The visible Refresh button is the deterministic boundary trigger.
await page.locator('#rbv-button-2').click();
let text = await waitForTasks('[]');
check('page loads without page errors', pageErrors.length === 0,
    pageErrors.join(' | '));
check('page loads without console errors', consoleErrors.length === 0,
    consoleErrors.join(' | '));
check('initial GET reaches the view', text === '[]', `got=${JSON.stringify(text)}`);

await page.locator('#rbv-input-4').fill('browser made this');
await page.locator('#rbv-button-5').click();
text = await waitForTasks('[{"id":1,"title":"browser made this","done":false}]');
check('POST creates a task and refreshes state', text === '[{"id":1,"title":"browser made this","done":false}]',
    `got=${JSON.stringify(text)}`);

await page.locator('#rbv-input-6').fill('1');
await page.locator('#rbv-button-7').click();
text = await waitForTasks('[{"id":1,"title":"browser made this","done":true}]');
check('toggle POST updates backend state', text === '[{"id":1,"title":"browser made this","done":true}]',
    `got=${JSON.stringify(text)}`);

await page.locator('#rbv-button-8').click();
text = await waitForTasks('[]');
check('DELETE removes backend state', text === '[]', `got=${JSON.stringify(text)}`);

await browser.close();

if (failures > 0) {
    console.error(`task board frontend gate: ${failures} failure(s)`);
    process.exit(1);
}
console.log('task board frontend gate: PASS');
