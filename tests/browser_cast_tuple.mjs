//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { resolve, extname, basename } from 'node:path';
import { createRequire } from 'node:module';
const { chromium } = createRequire(import.meta.url)(process.env.PLAYWRIGHT_MODULE || 'playwright');
const root = resolve(process.argv[2]);
const mime = { '.html': 'text/html', '.mjs': 'text/javascript', '.js': 'text/javascript', '.wasm': 'application/wasm' };
const server = createServer(async (request, response) => {
  const name = basename(new URL(request.url, 'http://localhost').pathname);
  try {
    response.writeHead(200, { 'content-type': mime[extname(name)] || 'application/octet-stream' });
    response.end(await readFile(resolve(root, name)));
  } catch { response.end(); }
});
await new Promise(done => server.listen(0, '127.0.0.1', done));
const browser = await chromium.launch({ channel: process.env.PLAYWRIGHT_CHANNEL || 'chrome' });
try {
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  for (const [name, expected] of [
    ['bit_string_roundtrip_wasm', ['hello', 'hello', '127']],
    ['tuple_return_wasm', ['6']],
  ]) {
    await page.goto(`http://127.0.0.1:${server.address().port}/${name}.html`);
    await page.waitForFunction(values => {
      const actual = [...document.querySelectorAll('span')].map(element => element.textContent.trim());
      return JSON.stringify(actual) === JSON.stringify(values);
    }, expected);
    assert.deepEqual(errors, [], name);
  }
  console.log('WASM bit encoding, Data preservation, widening and tuple round trips pass');
} finally {
  await browser.close();
  server.close();
}
