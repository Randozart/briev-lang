#!/usr/bin/env bash
# rbv_gate.sh — the web-surface regression gate.
#
# 2026-10-07 (plan 2026-10-07-web-surface-completion.md, W1+W2): builds the
# web fixtures, checks the generated IR for call/declare agreement, then runs
# the wasm32 runtime gate. Guards the pointer-width / void-frgn /
# view-surface-liveness fixes and the unpacked-obj String-field init
# (BUGS.md:8800) — a funcref signature mismatch or an aggregate-vs-element
# store is accepted by some stages but rejected by llc / traps on wasm32.
#
# Usage: bash benchmarks/rbv_gate.sh
#   BRIEVC=/path/to/brievc   override the compiler binary
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

BRIEVC="${BRIEVC:-$ROOT/target/debug/brievc}"
[ -x "$BRIEVC" ] || BRIEVC="$ROOT/target/release/brievc"
[ -x "$BRIEVC" ] || { echo "brievc not found (build first, or set BRIEVC)"; exit 1; }

OUT_ROOT="$(mktemp -d /tmp/opencode/rbvgate.XXXXXX)"

# Build a fixture; echo its output dir (empty on failure).
build_fixture() {
    local fixture="$1"
    local name; name="$(basename "${fixture%.rbv}")"
    local out="$OUT_ROOT/$name"
    mkdir -p "$out"
    "$BRIEVC" build "$fixture" --split --out "$out" >/dev/null || {
        echo "build failed: $fixture" >&2; return 1;
    }
    echo "$out"
}

# IR call/declare agreement for a built fixture.
check_ir() {
    local out="$1" name="$2"
    local ll="$out/$name.ll"
    [ -f "$ll" ] || { echo "no IR emitted at $ll" >&2; return 1; }
    if command -v python3 >/dev/null; then
        python3 "$ROOT/benchmarks/rbv_ir_check.py" "$ll" || return 1
    else
        echo "SKIP: python3 not available (IR call/declare check)"
    fi
}

# 1. Router smoke — IR + runtime behavior.
ROUTER_OUT="$(build_fixture "$ROOT/tests/fixtures/router.rbv")" || exit 1
check_ir "$ROUTER_OUT" router || exit 1
if command -v node >/dev/null; then
    node "$ROOT/benchmarks/rbv_gate.mjs" "$ROUTER_OUT/router.wasm" || exit 1
else
    echo "SKIP: node not available (runtime gate)"
fi

# 2. Unpacked-obj String-field init (BUGS.md:8800) — build + IR.
#    The old emitter's invalid `store [1 x ptr]` made the build itself fail.
OBJ_OUT="$(build_fixture "$ROOT/tests/fixtures/obj_init.rbv")" || exit 1
check_ir "$OBJ_OUT" obj_init || exit 1

# 3. Member txn on a plain top-level obj var (BUGS.md:8921) — build + IR +
#    the member-txn variant must be a top-level export and the shim must
#    reference it. Before the fix the wasm exported no `go`, so the click
#    resolved a missing export at runtime.
OBJ_ROUTER_OUT="$(build_fixture "$ROOT/tests/fixtures/obj_router.rbv")" || exit 1
check_ir "$OBJ_ROUTER_OUT" obj_router || exit 1
if ! grep -q "define void @go_router" "$OBJ_ROUTER_OUT/obj_router.ll"; then
    echo "obj_router: member txn variant @go_router not emitted" >&2; exit 1
fi
if ! grep -q "_txn(\"go_router\")" "$OBJ_ROUTER_OUT/obj_router.mjs"; then
    echo "obj_router: shim does not bind the member txn variant" >&2; exit 1
fi
echo "OK obj_router (member txn variant emitted + bound)"

# 4. Popstate (back/forward) — the window-scoped trigger `b-window:popstate`
#    must bind `window` in the shim and the sync txn must be a top-level
#    export. Before the window-scope parse, the shim used `el.addEventListener`
#    (the element, not `window`) and the txn was not routed to a window
#    listener.
POP_OUT="$(build_fixture "$ROOT/tests/fixtures/popstate.rbv")" || exit 1
check_ir "$POP_OUT" popstate || exit 1
if ! grep -q "window.addEventListener(\"popstate\"" "$POP_OUT/popstate.mjs"; then
    echo "popstate: shim does not bind window.popstate" >&2; exit 1
fi
if ! grep -q "_txn(\"sync_route\")" "$POP_OUT/popstate.mjs"; then
    echo "popstate: shim does not bind the sync_route txn" >&2; exit 1
fi
if ! grep -q "define void @sync_route" "$POP_OUT/popstate.ll"; then
    echo "popstate: sync_route txn variant not emitted" >&2; exit 1
fi
echo "OK popstate (window listener + sync_route exported)"

# 5. Multi-page B2 — each .rbv bundles to ONE self-contained HTML with a
#    plain <a href> cross-link (zero external refs).
check_bundle() {
    local fixture="$1" name="$2" expect_link="$3"
    local out="$OUT_ROOT/bundle_$name"
    mkdir -p "$out"
    local err="$out/build.err"
    "$BRIEVC" build "$fixture" --out "$out" >/dev/null 2>"$err" || {
        echo "bundle build failed: $fixture" >&2; return 1;
    }
    # A view-trigger-bound no-param txn must NOT false-warn "never dispatched"
    # (2026-10-07 fix: warn_undispatched_txns skips live txns).
    if grep -q "never dispatched" "$err"; then
        echo "false 'never dispatched' warning for $fixture:" >&2
        grep "never dispatched" "$err" >&2; return 1
    fi
    local html="$out/$name.html"
    [ -f "$html" ] || { echo "no bundle at $html" >&2; return 1; }
    if grep -qE "<script[^>]*src=|<link[^>]*href=|fetch\(['\"]https?:" "$html"; then
        echo "bundle has external refs: $html" >&2; return 1
    fi
    grep -q "<a href=\"$expect_link\"" "$html" || {
        echo "missing cross-link '$expect_link' in $html" >&2; return 1;
    }
    echo "OK bundle $name (self-contained, links $expect_link)"
}
check_bundle "$ROOT/examples/multi_page_a.rbv" multi_page_a "multi_page_b.html" || exit 1
check_bundle "$ROOT/examples/multi_page_b.rbv" multi_page_b "multi_page_a.html" || exit 1

# 5b. File-based routing (Part 2c): `brievc web examples/` builds every page in
#     the folio.toml [web.pages] set + generates the shared nav. Check the
#     generated nav.html has the cross-links, the nav.json lists both pages,
#     and each page's HTML carries its declared key (data-briev-page).
WEB_OUT="$OUT_ROOT/web_dir"
mkdir -p "$WEB_OUT"
# Copy the page set + folio.toml into an isolated dir (so the build doesn't
# write artifacts into examples/).
cp "$ROOT/examples/multi_page_a.rbv" "$ROOT/examples/multi_page_b.rbv" "$ROOT/examples/folio.toml" "$WEB_OUT/" || exit 1
"$BRIEVC" web "$WEB_OUT" >/dev/null 2>"$WEB_OUT/web.err" || {
    echo "brievc web failed:" >&2; cat "$WEB_OUT/web.err" >&2; exit 1;
}
[ -f "$WEB_OUT/nav.html" ] || { echo "no nav.html generated" >&2; exit 1; }
[ -f "$WEB_OUT/nav.json" ] || { echo "no nav.json generated" >&2; exit 1; }
grep -q "<a href=\"multi_page_a.html\">a</a>" "$WEB_OUT/nav.html" || { echo "nav missing link to page a" >&2; exit 1; }
grep -q "<a href=\"multi_page_b.html\">b</a>" "$WEB_OUT/nav.html" || { echo "nav missing link to page b" >&2; exit 1; }
grep -q "\"key\": \"a\"" "$WEB_OUT/nav.json" || { echo "nav.json missing key a" >&2; exit 1; }
grep -q "\"key\": \"b\"" "$WEB_OUT/nav.json" || { echo "nav.json missing key b" >&2; exit 1; }
grep -q 'data-briev-page="a"' "$WEB_OUT/multi_page_a.html" || { echo "page a missing stamped key" >&2; exit 1; }
grep -q 'data-briev-page="b"' "$WEB_OUT/multi_page_b.html" || { echo "page b missing stamped key" >&2; exit 1; }
echo "OK brievc web (nav.json + nav.html + stamped keys)"

# 6. Stranger-loads-page smoke (Phase 3.1 gate): build the counter in bundle
#    mode and load it in a real Chromium — the node gate (step 1) stubs the
#    host and cannot prove the page loads from file://, the boot flush lands,
#    or a real click round-trips to the DOM. Skips gracefully if Playwright /
#    Chromium is unavailable (no sudo / pacman needed — the browser downloads
#    to ~/.cache/ms-playwright via `npx playwright install chromium`).
if command -v node >/dev/null && node -e "import('playwright')" >/dev/null 2>&1; then
    COUNTER_OUT="$OUT_ROOT/counter"
    mkdir -p "$COUNTER_OUT"
    "$BRIEVC" build "$ROOT/examples/counter.rbv" --out "$COUNTER_OUT" >/dev/null || {
        echo "counter build failed" >&2; exit 1;
    }
    node "$ROOT/benchmarks/rbv_browser_smoke.mjs" "$COUNTER_OUT/counter.html" || exit 1
    echo "OK counter (stranger-loads-page smoke: real browser, file://, click round-trip)"
else
    echo "SKIP: stranger-loads-page smoke (Playwright/Chromium not available)"
fi

echo "rbv_gate: OK"
