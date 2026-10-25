// lib/glue/web/web.js — the browser host module.
// Authored in part by Otto Westerlund (@odecode); adopted with attribution,
// see docs/contributors/otto-westerlund.md.
//
// 2026-10-07 (host-boundary decision record, D7): the browser is a LIBRARY
// over the webstack target's host-import namespace. A `frgn ... from
// "lib/glue/web/web.js"` names this file; its `export const` bodies are
// inlined into the generated shim at module scope, and the frgn's wasm
// import stub calls them by name. No protocol hashword, no GLUE language
// target — this file IS the host module.

export const console_log = (msg) => console.log(msg);
export const console_warn = (msg) => console.warn(msg);
export const console_error = (msg) => console.error(msg);
export const set_text = (elem, text) => elem ? (elem.textContent = text) : 0;
export const get_element_by_id = (id) => document.getElementById(id);
export const performance_now = () => performance.now();
export const get_canvas = (id) => document.getElementById(id);
export const present_frame = (ctx) => ctx && ctx.present ? ctx.present() : 0;
// 2026-10-07: window.location.href is the FULL URL; route matching compares
// against the path form ("/about"), so the host returns the PATHNAME.
export const location = () => window.location.pathname;
export const navigate = (url) => history.pushState(null, 0, url);
// 2026-10-07: synchronous XHR — the Briev fetch bindings are synchronous.
// Async fetch remains a future enhancement. The method-aware boundary keeps
// the HTTP status in host-module state: Briev reads it immediately after the
// fetch, before any other request can run.
let __briev_last_fetch_status = 0;
const __briev_http_request = (url, method, body) => {
  try {
    const xhr = new XMLHttpRequest();
    xhr.open(method, url, false);
    if (body) xhr.setRequestHeader('Content-Type', 'application/json');
    xhr.send(body);
    __briev_last_fetch_status = xhr.status || 0;
    return xhr.responseText;
  } catch (error) {
    __briev_last_fetch_status = 0;
    return '';
  }
};
export const fetch_url = (url) => __briev_http_request(url, 'GET', '');
export const fetch_url_method = (url, method, body) =>
  __briev_http_request(url, method, body);
export const last_fetch_status = () => __briev_last_fetch_status;
export const json_escape_string = (value) => {
  const text = String(value ?? '');
  return JSON.stringify(text).slice(1, -1);
};

export const fetch_method = (url, method, body, contentType) => {
  try {
    const xhr = new XMLHttpRequest();
    xhr.open(method, url, false);
    if (contentType) xhr.setRequestHeader('Content-Type', contentType);
    xhr.send(method === 'GET' || method === 'HEAD' ? null : body);
    return xhr.responseText;
  } catch (_) {
    return '';
  }
};

// Secure nonce source. Hex is the ABI-friendly form for JSON operation IDs.
export const web_random_bytes = (length) => {
  if (!Number.isInteger(length) || length < 1 || length > 65536 ||
      !globalThis.crypto?.getRandomValues) return '';
  try {
    const bytes = new Uint8Array(length);
    globalThis.crypto.getRandomValues(bytes);
    return new TextDecoder('utf-8').decode(bytes);
  } catch (_) { return ''; }
};
export const web_random_hex = (length) => {
  if (!Number.isInteger(length) || length < 1 || length > 65536 ||
      !globalThis.crypto?.getRandomValues) return '';
  try {
    const bytes = new Uint8Array(length);
    globalThis.crypto.getRandomValues(bytes);
    return Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('');
  } catch (_) { return ''; }
};

// 2026-10-22: bounded asynchronous HTTP handle table for I03. Promise values
// never cross the WASM ABI. Records stay owned until explicit release; every
// asynchronous continuation checks record identity before publishing results.
const asyncHttp = new Map();
let asyncHttpNext = 1;
let asyncHttpBytes = 0;
const asyncHttpMaxHandles = 32;
const asyncHttpMaxBytes = 32 * 1024 * 1024;
const asyncHttpMaxBody = 1024 * 1024;
const asyncHttpMethods = new Set(['GET', 'HEAD', 'POST', 'PUT', 'PATCH', 'DELETE', 'OPTIONS']);
const asyncHttpEventName = /^[A-Za-z_][A-Za-z0-9_]{0,63}$/;

const asyncHttpNotify = (handle, record) => {
  if (asyncHttp.get(handle) !== record || record.notified) return;
  record.notified = true;
  window.dispatchEvent(new CustomEvent(record.eventName, { detail: handle }));
};

const asyncHttpFail = (handle, record, error) => {
  if (asyncHttp.get(handle) !== record || record.status !== 1) return;
  record.status = 3;
  record.error = error;
  window.clearTimeout(record.timer);
  record.controller.abort();
  asyncHttpNotify(handle, record);
};

export const http_async_start = (url, method, body, optionsJson, eventName) => {
  if (asyncHttp.size >= asyncHttpMaxHandles || asyncHttpNext > 2147483647) return -4;
  let options;
  try { options = JSON.parse(optionsJson || '{}'); } catch (_) { return -1; }
  if (!options || typeof options !== 'object' || Array.isArray(options)) return -1;
  const timeout = options.timeout_ms === undefined ? 30000 : options.timeout_ms;
  const maxBytes = options.max_bytes === undefined ? asyncHttpMaxBody : options.max_bytes;
  if (!Number.isInteger(timeout) || timeout < 1 || timeout > 120000 ||
      !Number.isInteger(maxBytes) || maxBytes < 1 || maxBytes > asyncHttpMaxBody ||
      typeof options.content_type !== 'undefined' && typeof options.content_type !== 'string' ||
      typeof eventName !== 'string' || !asyncHttpEventName.test(eventName) ||
      typeof url !== 'string' || typeof method !== 'string' || typeof body !== 'string') return -1;
  const verb = String(method).toUpperCase();
  if (!asyncHttpMethods.has(verb)) return -2;
  if ((verb === 'GET' || verb === 'HEAD') && body !== '') return -1;
  let requestUrl;
  try {
    requestUrl = new URL(url, document.baseURI);
    if (requestUrl.protocol !== 'http:' && requestUrl.protocol !== 'https:') return -3;
  } catch (_) { return -3; }
  if (typeof fetch !== 'function' || typeof AbortController !== 'function') return -5;

  const handle = asyncHttpNext++;
  const record = { status: 1, httpStatus: 0, body: '', error: '', eventName,
    notified: false, retained: 0, controller: new AbortController(), timer: 0 };
  asyncHttp.set(handle, record);
  record.timer = window.setTimeout(() => {
    if (asyncHttp.get(handle) !== record || record.status !== 1) return;
    record.status = 3;
    record.error = 'timeout';
    record.controller.abort();
    asyncHttpNotify(handle, record);
  }, timeout);
  const headers = {};
  if (options.content_type) headers['Content-Type'] = options.content_type;
  const init = { method: verb, headers, signal: record.controller.signal };
  if (verb !== 'GET' && verb !== 'HEAD') init.body = body;

  Promise.resolve().then(() => fetch(requestUrl.href, init)).then(async (response) => {
    if (asyncHttp.get(handle) !== record || record.status !== 1) return;
    record.httpStatus = response.status;
    const declared = Number(response.headers.get('Content-Length'));
    if (Number.isFinite(declared) && declared > maxBytes) {
      asyncHttpFail(handle, record, 'response-too-large');
      return;
    }
    const reader = response.body?.getReader();
    let bytes;
    if (reader) {
      const chunks = [];
      let size = 0;
      try {
        while (true) {
          const part = await reader.read();
          if (asyncHttp.get(handle) !== record || record.status !== 1) { await reader.cancel(); return; }
          if (part.done) break;
          size += part.value.byteLength;
          if (size > maxBytes || asyncHttpBytes + size > asyncHttpMaxBytes) {
            try { await reader.cancel(); } catch (_) { /* The size failure remains authoritative. */ }
            asyncHttpFail(handle, record, 'response-too-large');
            return;
          }
          chunks.push(part.value);
        }
      } catch (error) {
        if (asyncHttp.get(handle) === record && record.status === 1) asyncHttpFail(handle, record, 'network');
        return;
      }
      bytes = new Uint8Array(size);
      let offset = 0;
      for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
    } else {
      // 2026-10-23: refuse non-streaming bodies because arrayBuffer() allocates
      // before enforcing max_bytes; targets this response-bound gap. Retire
      // only when the host boundary can cap bytes during body acquisition.
      asyncHttpFail(handle, record, 'host-error');
      return;
    }
    if (asyncHttp.get(handle) !== record || record.status !== 1) return;
    let text;
    try { text = new TextDecoder('utf-8', { fatal: true }).decode(bytes); }
    catch (_) { asyncHttpFail(handle, record, 'invalid-utf8'); return; }
    if (asyncHttp.get(handle) !== record || record.status !== 1) return;
    if (asyncHttpBytes + bytes.byteLength > asyncHttpMaxBytes) {
      asyncHttpFail(handle, record, 'response-too-large');
      return;
    }
    record.body = text;
    record.retained = bytes.byteLength;
    asyncHttpBytes += record.retained;
    record.status = 2;
    window.clearTimeout(record.timer);
    asyncHttpNotify(handle, record);
  }).catch((error) => {
    if (asyncHttp.get(handle) !== record || record.status !== 1) return;
    asyncHttpFail(handle, record, error?.name === 'AbortError' ? 'host-error' : 'network');
  });
  return handle;
};

export const http_async_status = (handle) => asyncHttp.get(handle)?.status ?? 0;
export const http_async_http_status = (handle) => asyncHttp.get(handle)?.httpStatus ?? 0;
export const http_async_body = (handle) => {
  const record = asyncHttp.get(handle);
  return record?.status === 2 ? record.body : '';
};
export const http_async_error = (handle) => {
  const record = asyncHttp.get(handle);
  return record?.status === 3 ? record.error : '';
};
export const http_async_cancel = (handle) => {
  const record = asyncHttp.get(handle);
  if (!record || record.status !== 1) return;
  record.status = 4;
  window.clearTimeout(record.timer);
  record.controller.abort();
};
export const http_async_release = (handle) => {
  const record = asyncHttp.get(handle);
  if (!record) return;
  asyncHttp.delete(handle);
  window.clearTimeout(record.timer);
  record.controller.abort();
  asyncHttpBytes -= record.retained;
};

export const query_parameter = (name, fallback) =>
  new URLSearchParams(window.location.search).get(name) ?? fallback;

// 2026-10-21: the host owns native sockets; the application owns policy. Each
// handle has an independent bounded FIFO. Notifications use ordinary window
// events so existing b-window transactions consume them without compiler-known
// callback names. Explicit close releases the handle and cancels stale callbacks.
const webSockets = new Map();
let webSocketNext = 1;
const wsNotify = (handle) => {
  queueMicrotask(() => {
    const channel = webSockets.get(handle);
    if (channel?.event) window.dispatchEvent(new CustomEvent(channel.event, { detail: handle }));
  });
};
export const ws_open = (url) => {
  try {
    const socket = new WebSocket(url);
    const handle = webSocketNext++;
    const channel = { socket, event: '', messages: [], bytes: 0, error: '' };
    webSockets.set(handle, channel);
    socket.addEventListener('open', () => wsNotify(handle));
    socket.addEventListener('close', () => wsNotify(handle));
    socket.addEventListener('error', () => {
      channel.error = 'The live connection failed. Check the service address and retry.';
      wsNotify(handle);
    });
    socket.addEventListener('message', (event) => {
      if (!webSockets.has(handle)) return;
      const text = event.data;
      const size = typeof text === 'string' ? new TextEncoder().encode(text).length : 0;
      if (typeof text !== 'string' || channel.messages.length >= 128 || channel.bytes + size > 1048576) {
        channel.error = 'The live message exceeds the text or queue limit; reconnect.';
        socket.close(1009, 'text queue limit');
        wsNotify(handle);
        return;
      }
      channel.messages.push({ text, size });
      channel.bytes += size;
      wsNotify(handle);
    });
    return handle;
  } catch (_) {
    return 0;
  }
};
export const ws_onmessage = (handle, event) => {
  const channel = webSockets.get(handle);
  if (!channel) return false;
  channel.event = event;
  if (channel.socket.readyState !== WebSocket.CONNECTING || channel.messages.length) wsNotify(handle);
  return true;
};
export const ws_status = (handle) => webSockets.get(handle)?.socket.readyState ?? 3;
export const ws_pending = (handle) => webSockets.get(handle)?.messages.length ?? 0;
export const ws_receive = (handle) => {
  const channel = webSockets.get(handle);
  const message = channel?.messages.shift();
  if (!message) return '';
  channel.bytes -= message.size;
  return message.text;
};
export const ws_error = (handle) => webSockets.get(handle)?.error ?? '';
export const ws_send = (handle, text) => {
  const channel = webSockets.get(handle);
  if (!channel || channel.socket.readyState !== WebSocket.OPEN || channel.socket.bufferedAmount > 1048576) return false;
  try { channel.socket.send(text); return true; } catch (_) { return false; }
};
export const ws_close = (handle) => {
  const channel = webSockets.get(handle);
  if (!channel) return;
  webSockets.delete(handle);
  channel.socket.close(1000, 'client closed');
};
export const event_after = (event, delay) => window.setTimeout(
  () => window.dispatchEvent(new CustomEvent(event)), Math.max(0, delay));
export const event_cancel = (handle) => window.clearTimeout(handle);

// 2026-10-23: keep unresolved account writes visible across a reload attempt.
// This targets browser lifecycle loss; remove the listener when no pending
// operations remain or the user explicitly confirms leaving them behind.
let warnBeforeUnload = false;
const beforeUnloadHandler = (event) => {
  if (!warnBeforeUnload) return;
  event.preventDefault();
  event.returnValue = '';
  return '';
};
export const web_set_unload_warning = (enabled) => {
  warnBeforeUnload = Boolean(enabled);
  if (warnBeforeUnload) window.addEventListener('beforeunload', beforeUnloadHandler);
  else window.removeEventListener('beforeunload', beforeUnloadHandler);
};
