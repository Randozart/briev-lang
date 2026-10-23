# Briev Task Board

A small fullstack application used to validate Briev through product-shaped
composition: native HTTP backend, browser frontend, JSON API, and later
PostgreSQL persistence.

## Status

In-memory CRUD backend with a sequential request loop. Frontend and PostgreSQL
are the next slices.

## Backend

```bash
./target/release/brievc build backend/main.bv --out backend
./backend/main
```

The server binds port `18367` and handles one request per TCP connection
sequentially. State persists in memory across requests until the process exits.

## API

```text
GET    /health
GET    /api/tasks
POST   /api/tasks                   {"title":"..."}
POST   /api/tasks/:id/toggle
DELETE /api/tasks/:id
```

## Gate

```bash
bash tests/api_gate.sh
```

The gate builds the app, starts the backend, polls for readiness, and asserts
health, empty list, create, persistence, toggle, invalid body, unknown route,
and delete.
