# Language surfaces: `attr` metadata + chaining simplification

**2026-11-08.** Status: ACTIVE (plan; implementation sequenced after the small
items below).
**Operator decisions recorded (2026-11-08):**
1. `!>` is **hard-broken** (no alias).
2. `attr` owners are **optional** — bare `attr key: value;` is claimed by a
   plugin/consumer by key; owned `attr<owner> { … }` validates against the
   owner's schema.
3. Chaining drops **positional back-references** (`.N>>`, `prev(N)`)
   entirely; captures are named locals, referenced by name.
4. `into` is the capture keyword; the receiver-omission convention stays
   (chains are function calls); `it` is the **optional** explicit binding for
   a non-first argument position.
5. `accel` policy/report leave metadata: they are compiler modes (flags).
   `accel` is already a keyword.

**Plan defaults adopted (adjust before the owning section is implemented):**
owned single-line `attr<owner> key: value;` is allowed (symmetry with the
block); an unclaimed bare attr **warns**; flag names
`--accel=<keyword-only|try-all|try-all-force>` and
`--accel-report=<off|verbose>`; the chaining cast step `.(Type)` survives;
multiple `it` placeholders in one call are an error.

## Reference index (other plans this builds on — do not duplicate)

| Plan / record | What it owns |
|---|---|
| `docs/plans/2026-11-06-idiom-recognition-doctrine.md` | §12 idiom recognition doctrine, five gates, W1–W5, ledger enforcement |
| `docs/plans/2026-11-08-general-workstream-countable-body-and-folds.md` | P0 countable-body fix, P1 `approx`/`exact`, W4 why-nots, `seq` contract |
| `docs/plans/2026-11-08-intel-g1.5-opencl-flavored-spirv.md` | XPU Stage E/F (separate lane; do not mix with this work) |
| `docs/architecture/derivation-not-recognition.md` | §12 doctrine (gate 3 = numeric license; ledger) |
| `docs/architecture/keyword-taxonomy.md` | strategy vs ambiguity keywords; `seq` contract |
| `docs/architecture/universal-chaining.md` | current chaining spec (to be rewritten by §C) |
| `benchmarks/results/2026-11-06-intel-ptl-*`, `2026-11-08-intel-g1.5-stage-e.md` | gate evidence |

## Queue (order matters)

### 0. Small items (executed first; tracked in the general-workstream plan)

- **Per-`foreach` `approx` / `exact`** — the statement-modifier machinery
  from the `seq` work exists; wire the fold detector to the matched
  `foreach`'s modifiers. Precedence: `seq` > `foreach exact` >
  `foreach approx` > node `exact` > node `approx` > default. Same-level
  `approx`+`exact` contradiction = parse error (mirrors the node pair).
- **`seq` redundancy diagnostic** — warn where the declaration has no
  observable effect: integer folds (order-independent) and provably pure,
  disjoint-loop bodies (the accel analysis facts). Float folds never warned
  (rounding order is observable). Advisory; the constraint stays honored.

### A. Metadata — `attr` (hard break, owner optional)

**Boundary (recorded):**

| Construct | Owns | Never does |
|---|---|---|
| `op` + contracts | operator **behaviour/implementation** | representation; consumer metadata |
| `spec` (type bodies) | **physical representation** (width/layout/ABI) | behaviour; hints; non-physical metadata |
| `attr<owner>` | **non-physical consumer metadata** (backend/plugin/analysis/policy) | behaviour; layout; any change to language-level meaning |

Rules: (1) an attr may never change observable program meaning — semantics
belong to contracts/`spec`/keywords; (2) no representation/ABI; (3) never
weakens/discharges contracts; (4) ownership: compiler owns the envelope and
routing only; owners register key schemas (`config/meta-vocab.dbv` grows an
owner column); (5) every key names a consumer — unclaimed keys warn; (6) no
attr may be *required* to reach the efficient default; (7) legal at module /
type / defn / node / txn level; `spec` stays type-body-only.

**Syntax:**

```briev
attr jira_ticket_number: OPS-1234;   // bare — plugin-claimed key
attr<llvm> { overflow: wrapping; tbaa: "FloatNode"; }
attr<owner> key: value;              // owned single line
```

**Hard break:** `!>` is a compile error whose fix hint names `attr` /
`attr<owner>`; sweep all 48 in-tree sites.

**Migration audit (reclassify under the boundary):**
- `overflow` → semantics: becomes a type/contract property (backend mapping
  is a projection), NOT an attr.
- `fp_math`, `fp_contract`, `associative`, `commutative` → retired; the
  author surface is `approx`/`exact` (P1).
- `accel`, `accel_report` → compiler flags (see below).
- `readonly`, `inline_hint`, `unroll_hint`, `convergence`, `alloc_scope`,
  MCMC family (`search_space`, `cost_model`, `tolerance`,
  `allowed_mutations`) → owner-owned attrs (no semantic change).
- `tbaa` → `attr<llvm>`; trust note: it is a claim (ideally proof-derived
  later).

**`accel` policy/report (out of metadata):** compiler modes —
`--accel=<keyword-only|try-all|try-all-force>` (default keyword-only, try
mode) and `--accel-report=<off|verbose>`. `attr<accel>` does not exist. If a
per-module policy is ever needed, the `accel` keyword grows a payload
(`accel<force>`), never metadata.

**Work items:** parser/HIR surface; owner registry + plugin claiming API;
hard-break diagnostic; 48-site sweep; backend/analysis consumers rewired;
SPEC §8.9 rewrite; meta-vocab owner column; tests (bare/owned/unknown
owner/unknown key/unclaimed warning/hard-break error).

### B. Chaining — `into` + optional `it`, no `>>`

- `expr into name` — capture as a real local; later steps reference the name
  (no positional refs, no chain stack).
- `a.f(x)` = `f(a, x)` — receiver omission is the dot convention and stays;
  the regular flow is unchanged.
- `it` — optional explicit binding when the value must be a non-first
  argument (`.g(x, it)`); at most one; absent = argument 0; multiple = error.
- `.(Type)` cast step survives; `. (N, named)` staging is not adopted.
- Remove `.N>>` / `.name>>`, `Expr::Capture`? (capture remains, as a binding),
  the chain stack in typechecker/interpreter/codegen, and the entire `>>`
  disambiguation section (receiver-kind, terminators, `5.1>>f()` caveat).
- `>>`/`<<` remain bitwise shifts only.
- In-tree migration: zero real uses (docs only) → SPEC §4.x +
  `universal-chaining.md` rewrite, parser/typechecker/interpreter/codegen
  simplification, tests.

## Verification protocol

Small items: unit + IR/gate evidence as in the general-workstream plan.
Attr/chaining: behavioral tests (parser, HIR round-trip, formatter fidelity,
consumer routing, hard-break diagnostic), `_ll` determinism gate, full suite;
docs updated in the same commits (SPEC, taxonomy, chaining doc, INDEX).
