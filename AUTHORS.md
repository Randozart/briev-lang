# Contributors

Briev is developed by Randy Smits-Schreuder Goedheijt and contributors.

## Project lead

- **Randy Smits-Schreuder Goedheijt** ([@Randozart](https://github.com/Randozart))
  — language design, compiler, runtime, stdlib, backends, documentation.

## Contributors

- **Otto Westerlund** ([@odecode](https://github.com/odecode)) —
  static trait-constrained generics and the `.sbv` synthesizability gate;
  compiler correctness fixes for liveness, `needs_state`, const globals,
  pointer provenance, tuple/match lowering, imports and call dispatch;
  the pure-Briev net stack (ByteSink, TCP sockets, HTTP/1.1, WebSocket,
  Base64, SHA-1); PostgreSQL v3 wire client; strict JSON parsing and
  stringify; portable browser stdlib and transport; numeric/CSR primitives;
  and the recommender application and browser/WASM work that exercised them.

See `docs/contributors/otto-westerlund.md` for the PR/commit ledger.

## Attribution

Original commit authorship is preserved where work is cherry-picked. Adapted
work records the original author and source commit in the commit body and in
the contributor ledger. See `NOTICE` and `README.md`.
