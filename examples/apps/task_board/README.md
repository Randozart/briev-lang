# Briev Task Board

A small fullstack application used to validate Briev through product-shaped
composition: native HTTP backend, browser frontend, JSON API, and later
PostgreSQL persistence.

## Status

Backend `/health` vertical. The first server intentionally handles one request
and exits; the sequential request loop is the next slice.

## Backend

```bash
./target/release/brievc build backend/main.bv --out backend
./backend/main
```

The server binds `127.0.0.1`, serves one `GET /health`, returns
`{"ok":true}`, and exits. This narrow scope makes the first network boundary
deterministic.

## Gate

```bash
bash tests/api_gate.sh
```

The gate builds the app, starts the backend on an isolated port, polls for
readiness, asserts the exact HTTP response and JSON body, and fails on any
stderr output, mismatch, or timeout.
