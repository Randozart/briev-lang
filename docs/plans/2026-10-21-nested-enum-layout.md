# Nested enum layout — Option B

**Date:** 2026-10-21
**Status:** ACTIVE — B0 typed-context slice.
**Branch:** `feat/nested-enum-layout`
**Base:** safe Phase C `cab089b9` on `feat/small-sum-unboxing`
**Rejected predecessor:** `experiment/nested-sret` (`2ed66530`) — fixed
`%enum_out+2` scratch/passthrough attempt; BOUND=50M segfaults.
**Goal:** lifetime-safe, per-enum-instance inline layout for nested enum
payloads; remove nested `malloc(16)` for FSM without the Option-A fixed 32-byte
image.

## Why Option B

Option A (fixed 32-byte sret image with one nested area):

- covers `Ok(TcpState.X)`;
- wastes three words per enum even when flat;
- caps nesting at one level;
- needs another redesign for tuples/deeper nesting.

Option B computes a layout from the concrete enum instance and variant payload
types:

```
image = 1 tag word + max(payload words over variants)
```

with recursive nested-enum payloads, tuple sums, and a boxed fallback for
unknown/recursive/deeper-than-cap payloads. It is the long-term representation.

## Prerequisite: concrete enum instance

The backend currently constructs by variant name and generic payload type. It
does not always know the concrete instance (`Result<TcpState,FsmError>` vs
`Result<Int,Int>`). Layout must be keyed by the **concrete instance**, because:

- `TcpState` payload = nested enum (2+ words);
- `Int` payload = 1 word;
- `WsFrame` payload = handle (1 word).

Therefore B0 (typed construction context) precedes layout.

## B0 — Typed construction/match context

### Work

- Add `FunctionContext.expected_enum_type: Option<Type>` and
  `item_return_type: Option<Type>`.
- Set `item_return_type` in defn emission from the declared return type.
- Set `expected_enum_type` around:
  - a `let x: Enum = <init>` initializer;
  - a `term <expr>`/`Tail` return expression;
  - constructor payload arguments (using the variant payload type substituted
    with the expected instance's type args);
  - match arms (already know `scrut_ty`; propagate the expected result type).
- Save/restore the context around every nested emission; no global leakage.
- No layout/codegen change.

### Tests

- `Result<TcpState,FsmError>` constructor sites see a concrete expected type.
- `Result<Int,Int>` sites see the flat instance.
- `Result<WsFrame,String>` sites see the struct-handle instance.
- A site with no expected type remains `Unknown` (boxed layout fallback).

### B0 result (2026-10-21)

Landed `expected_enum_type` / `item_return_type` plumbing:

- let annotations seed the initializer;
- term/tail seeds from the defn return type;
- enum payload emission propagates the variant payload type through generic
  substitution;
- `BRIEV_DIAG_ENUM_TYPE=1` reports variant + expected instance + payload type.

This also fixed a latent call bug: `emit_enum_construct` was passed the
destination register (`%t0`) instead of the variant name, so payload lookup
always missed. It now receives the variant name (`Ok`, `TcpState::Listen`, …).

FSM diagnostics now show `Result<TcpState,FsmError>` and nested
`TcpState`/`FsmError` instances correctly. No layout/codegen change yet.

## B1 — Concrete `EnumLayoutPlan`

### Derivation

For a concrete enum instance `E<args>`:

- `tag_words = 1`.
- For each variant payload type (substituting `args`):
  - scalar / handle / string / list / struct handle → 1 word;
  - direct enum instance → `1 + nested_payload_words` (inline);
  - tuple → sum of element words;
  - recursive or depth > `max_enum_inline_depth` → 1 word, boxed;
  - unknown → 1 word, boxed.
- `payload_words = max(variant words)`.
- `image_words = 1 + payload_words`.

Config: `max_enum_inline_depth` in `config/targets.dbvl` (default 2) so deep
recursion cannot explode code size; beyond it, box.

Plan key: canonical concrete type key (`Enum<args>`), not the bare enum name.

### Tests

- `Result<TcpState,FsmError>` → tag + nested enum payload.
- `Result<Int,Int>` → 2 words.
- `Result<WsFrame,String>` → 2 words (handles).
- `Result<(TcpState, Int), String>` → tuple layout.
- recursive `Option<Option<...>>` → inline until cap, then boxed.
- layout equality/determinism across map order.

### Gate

Layout covers every enum in the corpus; no fixed 16-byte assumption remains in
construction, match, size calculation, or `malloc`.

## B2 — Layout-aware image and match

- Use `image_words * 8` for:
  - sret caller slots,
  - boxed enum `malloc`,
  - any enum image copy.
- Store outer tag at word 0; payload words follow the layout.
- Match tag reads word 0.
- Single-payload binding:
  - scalar/handle: same as today;
  - inline enum: payload is an interior pointer to the nested word range;
  - tuple: member offsets from layout.
- Boxed fallback uses the same layout, so nested inlines remain valid inside
  heap images.

### Gate

No `malloc(i64 16)` or `alloca i64, i64 2` assumption remains for enum images;
all offsets derived from the plan.

## B3 — Nested construction and passthrough

- Direct nested constructor writes into the outer image's nested range; outer
  payload word stores the interior pointer.
- Nested enum value lifetime = outer image lifetime (caller sret slot or heap).
- Returned local enum lets in sret defns bind `%enum_out`, and sret calls in
  their initializer receive the same slot (passthrough).
- Sret call graph:
  - sret caller may call sret callee with its own out slot;
  - non-sret enum-returning caller remains boxed unless it is proven local;
  - non-enum callers must consume the slot locally (no state/global/return).
- No callee-frame scratch pointer.

### Safety rules

- never return a pointer to a caller-frame slot except through `%enum_out`;
- never inline a payload whose depth exceeds the config cap;
- never inline a payload that is a call result or variable whose lifetime is
  not the outer image;
- fall back to boxed on any doubt.

### Gate

- `fsm_dispatch`: `malloc(16)` count 0, BOUND=5 and BOUND=50M correct, no
  segfault.
- `ws_frame`: BOUND=5 and BOUND=50M correct; passthrough no dangling.
- `spsc_ring`: remains 0 boxes, ~1.9x.
- `local_enum_smoke`: correct.

## B4 — Tuple/multi-payload and generics

- Multi-payload variants lay out every element by word count.
- Tuple pattern extraction uses offsets.
- Generic enum instances use substituted payload types.
- Deeper nesting and recursive payloads stay boxed beyond cap.
- Tests for `Result<(TcpState, WsFrame), String>` and recursive wrappers.

## B5 — Promotion and measurement

- Full runtime suite all MATCH.
- `cargo test --lib`, tier-1, CI LLVM 18.
- Quick/timing effect-shape guard unchanged.
- Benchmark comparison: fsm, spsc, ws, boxed-heavy corpus; no cache regression.
- Only then retire the conservative sret/boxed split.

## Acceptance summary

| Gate | Requirement |
|---|---|
| Concrete types | every constructor site typed or `Unknown` |
| Layout | deterministic, target-depth cap, boxed fallback |
| Images | all sizes/offsets from layout |
| Nested lifetime | caller-owned outer image only |
| FSM | 0 nested boxes, BOUND=5/50M correct |
| WS | passthrough correct at both bounds |
| SPSC | 0 boxes, ~1.9x |
| Suite | all MATCH, no regression |
| Tests | lib + tier-1 + CI green |
| Guards | quick/timing PASS |

## Risks

1. Concrete enum type plumbing is the hard prerequisite; without it, B1 cannot
   be correct.
2. Recursive/generic layouts can explode; the depth cap and boxed fallback are
   mandatory.
3. Image size changes affect boxed allocations and cache behavior; measure.
4. Tuple payload offsets must agree between construction and match.
5. Exported/foreign/dynamic enum surfaces must remain boxed.
6. Sret passthrough must not create a caller-frame escape; the call graph
   filter is a safety mechanism, not an optimization.

## Rollback

- Safe production candidate: `cab089b9` on `feat/small-sum-unboxing`.
- Failed fixed-nested attempt: `experiment/nested-sret` (`2ed66530`).
- This branch: `feat/nested-enum-layout`; each slice is a commit.

## Commit log

| Commit | Change |
|---|---|
| `_plan_commit_` | plan |
| _this commit_ | B0 typed construction/match context |
| _pending_ | B1 EnumLayoutPlan |
| _pending_ | B2 layout-aware images/match |
| _pending_ | B3 nested construction/passthrough |
| _pending_ | B4 tuple/generic |
| _pending_ | B5 promotion |
