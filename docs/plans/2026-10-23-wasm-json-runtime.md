# wasm JSON runtime bring-up

**Date:** 2026-10-23
**Status:** J1/J2/J4 COMPLETE — Task Board parser + structured Chromium CRUD pass; J3 standalone parser fixture remains
**Branch:** `feat/task-board-structured`
**Base:** `5b247f46`

## Goal

Make `std/json.bv` parse correctly on wasm32 and feed a real
`List<TaskItem>` into the Task Board `b-each` view.

The compiler now emits wasm for the JSON parser; the remaining failure is a
browser runtime crash during `parse_tasks` / `parse_string`.

## Scope

### J1 — minimize the crash

- Build a minimal `.rbv` that calls `json_parse` with one scalar, one array,
  and one object.
- Capture the precise wasm trap/stack in Chromium.
- Reduce to the smallest parser entry point (`parse_string`, escape handling,
  `char_at`, `briev_bits_to_str`) before changing code.

### J2 — fix the runtime fault

Likely categories, in order:

1. String pointer/width mismatch in `parse_string` / `append_char`.
2. `briev_bits_to_str` host shim reading the wrong representation.
3. wasm memory growth invalidating a live typed-array view.
4. Parser loop termination/count corruption.
5. Tuple/enum payload lifetime in parsed frames.

No app-level parser or backend-rendering workaround.

### J3 — parser conformance gate

Standalone wasm browser gate for:

- scalar;
- array;
- object;
- escaped string;
- number;
- malformed input returning `Err`.

### J4 — Task Board integration

- Convert parsed `JsonValue` array into `List<TaskItem>`.
- Render title/done through `b-each`.
- Extend the existing Chromium CRUD gate to assert structured list items,
  not raw JSON.
- Keep create/toggle/delete API behavior unchanged.

## Gates

- minimized wasm parser fixture;
- Chromium parser gate;
- Task Board Chromium CRUD + structured rendering;
- full `rbv_gate.sh`;
- `cargo test --lib`;
- full runtime suite before merge.

## Non-goals

- No backend-rendered HTML.
- No host-side JSON parsing.
- No change to the public HTTP API.
- No performance tuning in this slice.

## Execution log

- J1 narrowed:
  - scalar, string, object, and title lookup parse correctly;
  - `parse_keyword("true", ...)` returns `BoolVal(true)` correctly;
  - a full object field `done:true` reaches the view as `BoolVal` with a
    **false payload**.
- Fixed along the way:
  - Data→String host shim reads `[i64 len][bytes]`, not NUL-terminated data;
  - function sret slots are typed, so nested enum constructors no longer
    overwrite the enclosing Result;
  - Bool payloads widen for literal pattern comparisons;
  - view materializers clear transaction context and no longer recurse through
    flush;
  - `briev_char_len`/`char_to_str` call sites use target ABI;
  - boxed Bool/Char call arguments unbox to target width.
- Structured Task Board rendering is implemented but blocked on the remaining
  BoolVal payload propagation bug in object pairs.

- J1/J2 resolved:
  - parsed object/list `done:true` reaches `b-each` correctly;
  - the earlier BoolVal report came from a flattened top-level view probe, not
    from JSON object parsing.
- J4 resolved:
  - keyed `b-each` applied item bindings only when creating an element, so a
    toggle that changed `done` on an existing key left the old DOM text;
  - existing keyed nodes now receive field updates;
  - item triggers bind only on creation so refresh does not attach duplicate
    listeners;
  - Chromium create/toggle/delete gate passes with structured rows.
