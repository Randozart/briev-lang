// rbv_browser_smoke.mjs — Phase 3.1 "stranger loads a page" smoke.
//
// Usage: node benchmarks/rbv_browser_smoke.mjs <path/to/counter.html>
//
// Builds nothing; expects a pre-built self-contained .html (bundle mode:
// inline CSS + shim + wasm base64). Launches a real Chromium via Playwright,
// loads the page from file://, and asserts the web surface actually works in
// a browser — the things the node gate (rbv_gate.mjs) cannot prove:
//   - the page loads from file:// with NO console errors / page errors
//     (a real bug found + fixed: createApp read runtime._instance.exports
//      before the async _init resolved, throwing "Cannot read properties of
//      null (reading 'exports')" in a real browser)
//   - the seeded instance's b-text reflects the real state (main.count = 5,
//     the Briev-side seed) on load — __web_boot emits an initial flush of
//     every state field after init_state, so the shim shows the real state,
//     not the HTML literal (0)
//   - clicking the + button increments it (6) — a real click -> _txn -> wasm
//     -> flush -> DOM update round-trip
//   - clicking + again increments (7) — not a one-shot
//   - clicking Reset zeroes it (0)
//   - the anonymous <Counter /> instance renders (b-text present, 0)
//
// Exits 0 on pass, 1 on any failure. The gate (rbv_gate.sh step 5) skips
// gracefully if Playwright/Chromium is unavailable.
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

const htmlPath = process.argv[2];
if (!htmlPath) {
    console.error('usage: node rbv_browser_smoke.mjs <counter.html>');
    process.exit(2);
}
if (!fs.existsSync(htmlPath)) {
    console.error(`no such file: ${htmlPath}`);
    process.exit(1);
}

let chromium;
try {
    ({ chromium } = await import('playwright'));
} catch (e) {
    console.error(`playwright not importable: ${e.message}`);
    process.exit(3);
}

let failures = 0;
function check(name, ok, detail = '') {
    console.log(`${ok ? 'PASS' : 'FAIL'} ${name}${detail ? `: ${detail}` : ''}`);
    if (!ok) failures++;
}

const browser = await chromium.launch();
const page = await browser.newPage();

// Capture console errors + uncaught page errors — a stranger sees these.
const consoleErrors = [];
const pageErrors = [];
page.on('console', (msg) => {
    if (msg.type() === 'error') consoleErrors.push(msg.text());
});
page.on('pageerror', (err) => pageErrors.push(String(err)));

const fileUrl = 'file://' + path.resolve(htmlPath);
await page.goto(fileUrl, { waitUntil: 'load' });
// The wasm boots async (createApp -> instantiate -> __web_boot). Give it a
// moment, then poll for the seeded value to appear (the flush has landed).
await page.waitForTimeout(500);

// The seeded (Briev-side) instance: span id rbv-span-0. __web_boot flushes
// the real state (main.count = 5, the Briev-side seed) to the DOM on load, so
// the b-text shows 5 — not the HTML literal (0).
const seeded = page.locator('#rbv-span-0');
let seededText = (await seeded.textContent() || '').trim();
// Poll for the boot flush to land.
for (let i = 0; i < 20 && seededText !== '5'; i++) {
    await page.waitForTimeout(100);
    seededText = (await seeded.textContent() || '').trim();
}
check('page loads from file:// (no page error)', pageErrors.length === 0,
    pageErrors.length ? pageErrors.join(' | ') : '');
check('no console errors', consoleErrors.length === 0,
    consoleErrors.length ? consoleErrors.join(' | ') : '');
check('seeded b-text reflects the Briev-side seed (5) on load', seededText === '5',
    `got=${JSON.stringify(seededText)}`);

// Click + -> increment_main -> flush -> 6.
await page.locator('#rbv-button-1').click();
let plusText = '';
for (let i = 0; i < 30; i++) {
    await page.waitForTimeout(100);
    plusText = (await seeded.textContent() || '').trim();
    if (plusText === '6') break;
}
check('click + increments seeded to 6', plusText === '6', `got=${JSON.stringify(plusText)}`);

// Click + again -> 7 (idempotent round-trip, not a one-shot).
await page.locator('#rbv-button-1').click();
let plus2Text = '';
for (let i = 0; i < 30; i++) {
    await page.waitForTimeout(100);
    plus2Text = (await seeded.textContent() || '').trim();
    if (plus2Text === '7') break;
}
check('click + again increments seeded to 7', plus2Text === '7', `got=${JSON.stringify(plus2Text)}`);

// Click Reset -> 0.
await page.locator('#rbv-button-3').click();
let resetText = '';
for (let i = 0; i < 30; i++) {
    await page.waitForTimeout(100);
    resetText = (await seeded.textContent() || '').trim();
    if (resetText === '0') break;
}
check('click Reset zeroes seeded to 0', resetText === '0', `got=${JSON.stringify(resetText)}`);

// The anonymous (HTML-side) <Counter /> instance renders: b-text present, 0.
const anon = page.locator('#rbv-span-4');
let anonText = (await anon.textContent() || '').trim();
check('anonymous <Counter /> instance renders (b-text 0)', anonText === '0',
    `got=${JSON.stringify(anonText)}`);

await browser.close();

if (failures > 0) {
    console.error(`rbv_browser_smoke: ${failures} failure(s)`);
    process.exit(1);
}
console.log('rbv_browser_smoke: OK');
