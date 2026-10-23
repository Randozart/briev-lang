# Nested enum / JSON correctness fixes — record

**Date:** 2026-10-23
**Branch:** `feat/fullstack-task-board`
**Base:** `45b8850a`

## Fixes

- nested enum payload construction/match layout agreement;
- call-argument enum type propagation;
- inline non-constructor enum payload copy using declared payload type;
- return-shaped sret lifetime guard for callable txns;
- String-concat cleanup block tracking for match phi predecessors;
- JSON empty-object lookup boundary.

## Gates

- `cargo test --lib`: **3,302 passed**
- new daily-use golden tests: **3 passed**
- Task Board CRUD gate: **PASS**
- effect-shape quick guard: **PASS**
- LLVM determinism quick gate: **ALL DETERMINISTIC**
- full runtime suite: **all MATCH**

## Benchmark note

The full-suite ratios were collected on a noisy machine. They are correctness
evidence, not regression proof. The relevant structural regression gate is the
three new behavior fixtures plus determinism; benchmark-sensitive changes get
their own controlled A/B before any performance claim.
