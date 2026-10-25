//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
// Authored in part by Otto Westerlund (@odecode); adopted with attribution.
// Browser-host behavioral regression, runnable with Node; no app or live server.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { ReadableStream } from 'node:stream/web';
const source = fs.readFileSync(new URL('../lib/glue/web/web.js', import.meta.url), 'utf8');
const calls = [];
globalThis.window = new EventTarget();
globalThis.CustomEvent = class extends Event {
  constructor(type, options = {}) { super(type); this.detail = options.detail; }
};
globalThis.document = { baseURI: 'http://localhost/' };
window.location = { search: '?api=http%3A%2F%2Flocalhost%3A8080' };
window.setTimeout = setTimeout;
window.clearTimeout = clearTimeout;
globalThis.XMLHttpRequest = class {
  open(...args) { this.args = args; }
  setRequestHeader(...args) { this.header = args; }
  send(body) {
    calls.push({ args: this.args, header: this.header, body });
    if (this.args[1] === 'broken') throw new Error('network');
    this.responseText = this.args[1] === 'denied' ? '{"error":"auth-failed"}' : '{"ok":true}';
  }
};
const sockets = [];
globalThis.WebSocket = class extends EventTarget {
  static CONNECTING = 0; static OPEN = 1;
  constructor(url) {
    super();
    if (url === 'broken') throw new Error('url');
    this.readyState = 0; this.bufferedAmount = 0; this.sent = []; sockets.push(this);
  }
  send(text) { if (this.failSend) throw new Error('send'); this.sent.push(text); }
  close(code) { this.closeCode = code; this.readyState = 3; this.dispatchEvent(new Event('close')); }
  message(data) { this.dispatchEvent(new MessageEvent('message', { data })); }
  open() { this.readyState = 1; this.dispatchEvent(new Event('open')); }
};
const host = await import(`data:text/javascript;base64,${Buffer.from(source).toString('base64')}`);
const response = (text, status = 200) => ({
  status,
  headers: { get: () => null },
  body: new ReadableStream({
    start(controller) { controller.enqueue(new TextEncoder().encode(text)); controller.close(); }
  })
});
for (const method of ['GET', 'POST', 'PUT', 'DELETE', 'HEAD']) {
  assert.equal(host.fetch_method('ok', method, '{"value":"é"}', 'application/json'), '{"ok":true}');
  assert.equal(calls.at(-1).args[2], false);
  assert.equal(calls.at(-1).body, ['GET', 'HEAD'].includes(method) ? null : '{"value":"é"}');
  assert.deepEqual(calls.at(-1).header, ['Content-Type', 'application/json']);
}
assert.equal(host.fetch_url('ok'), '{"ok":true}');
assert.equal(host.fetch_method('denied', 'POST', '', ''), '{"error":"auth-failed"}');
assert.equal(host.fetch_method('broken', 'POST', '', ''), '');
assert.equal(host.query_parameter('api', ''), 'http://localhost:8080');
assert.equal(host.query_parameter('missing', 'fallback'), 'fallback');
assert.equal(host.ws_open('broken'), 0);
const first = host.ws_open('ws://first'), second = host.ws_open('ws://second');
let notices = 0;
window.addEventListener('test-live', () => notices++);
assert.equal(host.ws_onmessage(first, 'test-live'), true);
assert.equal(host.ws_onmessage(9999, 'test-live'), false);
assert.equal(host.ws_send(first, 'early'), false);
sockets[0].open();
await Promise.resolve();
assert.equal(notices, 1);
assert.equal(host.ws_status(first), 1);
assert.equal(host.ws_send(first, 'rating'), true);
assert.deepEqual(sockets[0].sent, ['rating']);
sockets[0].message('one'); sockets[0].message('🌍'); sockets[1].message('foreign');
await Promise.resolve();
assert.equal(host.ws_pending(first), 2);
assert.equal(host.ws_receive(first), 'one');
assert.equal(host.ws_receive(first), '🌍');
assert.equal(host.ws_receive(first), '');
assert.equal(host.ws_receive(second), 'foreign');
sockets[0].bufferedAmount = 1048577;
assert.equal(host.ws_send(first, 'too much'), false);
sockets[0].bufferedAmount = 0; sockets[0].failSend = true;
assert.equal(host.ws_send(first, 'broken'), false);
sockets[0].dispatchEvent(new Event('error'));
assert.notEqual(host.ws_error(first), '');
sockets[0].close(1006);
await Promise.resolve();
assert.equal(host.ws_status(first), 3);
const beforeClose = notices;
host.ws_close(first);
sockets[0].message('stale');
await Promise.resolve();
assert.equal(notices, beforeClose);
assert.equal(host.ws_pending(first), 0);
assert.equal(host.ws_status(9999), 3);
assert.equal(host.ws_error(9999), '');
host.ws_close(9999);
sockets[1].message(new ArrayBuffer(8));
assert.equal(sockets[1].closeCode, 1009);
const overflow = host.ws_open('ws://overflow');
sockets[2].message('x'.repeat(1048577));
assert.equal(sockets[2].closeCode, 1009);
const full = host.ws_open('ws://full');
for (let index = 0; index < 129; index++) sockets[3].message('x');
assert.equal(sockets[3].closeCode, 1009);
host.ws_close(second); host.ws_close(overflow); host.ws_close(full);
let timers = 0;
window.addEventListener('test-timer', () => timers++);
host.event_cancel(host.event_after('test-timer', 5));
host.event_after('test-timer', 0);
await new Promise(resolve => setTimeout(resolve, 20));
assert.equal(timers, 1);

// The async ABI never returns a Promise and keeps UI work runnable while a
// request is pending. Completion events carry the owned handle exactly once.
let finishSlow;
globalThis.fetch = () => new Promise(resolve => { finishSlow = resolve; });
const asyncNotices = [];
window.addEventListener('http_done', event => asyncNotices.push(event.detail));
const slow = host.http_async_start('/slow', 'GET', '', '{}', 'http_done');
assert.equal(slow, 1);
assert.equal(host.http_async_status(slow), 1);
let painted = false;
queueMicrotask(() => { painted = true; });
await Promise.resolve();
assert.equal(painted, true);
finishSlow(response('ready'));
await new Promise(resolve => setTimeout(resolve, 0));
assert.deepEqual(asyncNotices, [slow]);
assert.equal(host.http_async_status(slow), 2);
assert.equal(host.http_async_http_status(slow), 200);
assert.equal(host.http_async_body(slow), 'ready');
assert.equal(host.http_async_body(slow), 'ready');
host.http_async_release(slow);
assert.equal(host.http_async_status(slow), 0);

// HTTP errors remain complete transport responses, with request metadata intact.
let postInit;
globalThis.fetch = async (_url, init) => { postInit = init; return response('{"error":"unauthorized"}', 401); };
const denied = host.http_async_start('/private', 'POST', '{"x":1}',
  '{"content_type":"application/json"}', 'http_done');
await new Promise(resolve => setTimeout(resolve, 0));
assert.equal(postInit.method, 'POST');
assert.equal(postInit.body, '{"x":1}');
assert.equal(postInit.headers['Content-Type'], 'application/json');
assert.equal(host.http_async_status(denied), 2);
assert.equal(host.http_async_http_status(denied), 401);
assert.equal(host.http_async_body(denied), '{"error":"unauthorized"}');
assert.deepEqual(asyncNotices, [slow, denied]);
host.http_async_release(denied);

// Stream bounds are enforced before response text is retained.
globalThis.fetch = async () => ({ status: 200, headers: { get: () => null }, body: new ReadableStream({
  start(controller) { controller.enqueue(new TextEncoder().encode('12345')); controller.close(); }
}) });
const tooLarge = host.http_async_start('/large', 'GET', '', '{"max_bytes":4}', 'http_done');
await new Promise(resolve => setTimeout(resolve, 0));
assert.equal(host.http_async_status(tooLarge), 3);
assert.equal(host.http_async_error(tooLarge), 'response-too-large');
assert.deepEqual(asyncNotices, [slow, denied, tooLarge]);
host.http_async_release(tooLarge);

// Cancellation and release suppress a completion already in flight.
let finishCancelled;
globalThis.fetch = () => new Promise(resolve => { finishCancelled = resolve; });
const cancelled = host.http_async_start('/cancel', 'GET', '', '{}', 'http_done');
await Promise.resolve();
host.http_async_cancel(cancelled);
assert.equal(host.http_async_status(cancelled), 4);
finishCancelled(response('ignored'));
await new Promise(resolve => setTimeout(resolve, 0));
assert.deepEqual(asyncNotices, [slow, denied, tooLarge]);
host.http_async_release(cancelled);
let finishReleased;
globalThis.fetch = () => new Promise(resolve => { finishReleased = resolve; });
const released = host.http_async_start('/release', 'GET', '', '{}', 'http_done');
await Promise.resolve();
host.http_async_release(released);
finishReleased(response('ignored'));
await new Promise(resolve => setTimeout(resolve, 0));
assert.deepEqual(asyncNotices, [slow, denied, tooLarge]);

// Timeout is a notified failure; it never looks like a successful empty body.
globalThis.fetch = () => new Promise(() => {});
const timedOut = host.http_async_start('/timeout', 'GET', '', '{"timeout_ms":5}', 'http_done');
await new Promise(resolve => setTimeout(resolve, 10));
assert.equal(host.http_async_status(timedOut), 3);
assert.equal(host.http_async_error(timedOut), 'timeout');
assert.deepEqual(asyncNotices, [slow, denied, tooLarge, timedOut]);
host.http_async_release(timedOut);

// Invalid options, methods, URL schemes and GET bodies fail synchronously.
assert.equal(host.http_async_start('/x', 'GET', '', 'null', 'http_done'), -1);
assert.equal(host.http_async_start('/x', 'TRACE', '', '{}', 'http_done'), -2);
assert.equal(host.http_async_start('javascript:alert(1)', 'GET', '', '{}', 'http_done'), -3);
assert.equal(host.http_async_start('/x', 'GET', 'body', '{}', 'http_done'), -1);
host.web_set_unload_warning(true);
const unloadPending = new Event('beforeunload', { cancelable: true });
Object.defineProperty(unloadPending, 'returnValue', { writable: true, value: undefined });
window.dispatchEvent(unloadPending);
assert.equal(unloadPending.defaultPrevented, true);
host.web_set_unload_warning(false);
const unloadClear = new Event('beforeunload', { cancelable: true });
Object.defineProperty(unloadClear, 'returnValue', { writable: true, value: undefined });
window.dispatchEvent(unloadClear);
assert.equal(unloadClear.defaultPrevented, false);
console.log('HTTP compatibility, async bounds/lifecycle/paint, unload warning, WebSocket FIFO and timers pass');
