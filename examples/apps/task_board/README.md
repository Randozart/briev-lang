# Briev Task Board

A small fullstack application used to validate Briev through product-shaped
composition: native HTTP backend, browser frontend, JSON API, and later
PostgreSQL persistence.

## Status

In-memory CRUD backend and a read-only browser frontend. PostgreSQL and
browser mutations are the next slices.

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

## Frontend

```bash
bash tests/frontend_gate.sh
```

Builds the `.rbv` page and backend, seeds one task, serves the frontend, and
uses real Chromium to prove the browser reads native API state through CORS.

## API gate

```bash
bash tests/api_gate.sh
```

The gate builds the app, starts the backend, polls for readiness, and asserts
health, empty list, create, persistence, toggle, invalid body, unknown route,
and delete.
