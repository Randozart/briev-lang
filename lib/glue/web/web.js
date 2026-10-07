// lib/glue/web/web.js — the browser host module.
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
// 2026-10-07: synchronous XHR — the Briev `fetch(url) -> String` binding is
// synchronous (lib/std/web/fetch.bv). Async fetch is a future enhancement.
export const fetch_url = (url) => {
  const xhr = new XMLHttpRequest();
  xhr.open('GET', url, false);
  xhr.send();
  return xhr.responseText;
};
