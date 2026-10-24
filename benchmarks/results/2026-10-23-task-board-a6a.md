# Task Board A6a — request hardening

**Date:** 2026-10-23

## Landed

- 4 KiB request ceiling;
- structured 400 for truncated/incomplete framing;
- structured 413 for over-limit requests;
- 120-character title limit with `title_too_long`;
- frontend 400/413 notices;
- malformed/oversized connections cannot enter route dispatch;
- same server process survives and answers `/health` afterward.

## Gates

- Task Board API gate: PASS (includes raw truncated + oversized cases)
- Task Board browser CRUD gate: PASS
- full `brievc check`: PASS
- conformance sweep: PASS
