# Task Board A6a — request hardening

**Date:** 2026-10-23

## Landed

- 4 KiB request ceiling;
- structured 400 for truncated/incomplete framing;
- structured 413 for over-limit requests;
- 2-second `SO_RCVTIMEO` budget for idle connections;
- 120-character title limit with `title_too_long`;
- frontend 400/413 notices;
- malformed/oversized/idle connections cannot enter route dispatch;
- same server process survives and answers `/health` afterward.

## A6b status

Structured `b-each` over `List<Task>` is blocked: the webstack renders fixed
vectors and `String`/`Int` collections, not collections of struct/obj elements
with per-item field projection. No app-specific rendering workaround added.

## Gates

- Task Board API gate: PASS (CRUD + raw truncated/oversized/idle cases)
- Task Board browser CRUD gate: PASS
- full `brievc check`: PASS
- conformance sweep: PASS
