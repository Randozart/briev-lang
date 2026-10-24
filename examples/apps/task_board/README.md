# Briev Task Board

A small fullstack application used to validate Briev through product-shaped
composition: native HTTP backend, browser frontend, JSON API, and later
PostgreSQL persistence.

## Status

In-memory CRUD backend and a browser frontend with create/toggle/delete.
PostgreSQL persistence and richer rendering are the next slices.

## Backend

```bash
./target/release/brievc build backend/main.bv --out backend
./backend/main
```

The server binds port `18367` and handles one request per TCP connection
sequentially. State persists in memory across requests until the process exits.
Requests are capped at 4 KiB; truncated, oversized, and idle requests receive
structured 400/413 responses and do not enter API routing. Task titles are
limited to 120 characters. Idle sockets have a 2-second receive budget.

## API

```text
GET    /health
GET    /api/tasks
POST   /api/tasks                   {"title":"..."}
POST   /api/tasks/:id/toggle
DELETE /api/tasks/:id
```

## Frontend

```bash
bash tests/frontend_gate.sh
```

Builds the `.rbv` page and backend, serves the frontend, and uses real
Chromium to drive create, toggle, and delete through the native API and CORS
preflight boundary.

## API gate

```bash
bash tests/api_gate.sh
```

The gate builds the app, starts the backend, polls for readiness, and asserts
health, empty list, create, persistence, toggle, invalid body, unknown route,
and delete.
