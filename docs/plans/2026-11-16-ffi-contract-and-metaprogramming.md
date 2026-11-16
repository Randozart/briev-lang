# Universal FFI Contract — data-described ABIs, metaprogramming-generated glue

**2026-11-16.** Status: **plan — research complete, implementation not started.**
This plan answers one question: *what is the smallest set of compiler facts that
lets a language be added to Briev as data plus generated code, with no Rust
match arm and no loss of efficiency?* It also answers the question that came up
with it: **does the new HIR layer change the answer?** (Yes — see §3.)

**Companions:** `glue-as-abi-generator.md` (the "Briev is layout-agnostic"
doctrine this plan makes operational), `macro-system.md` (the `$` intrinsics and
their admission rule), `glue-ffi.md`, `glue-pipeline.md`,
`frgn-export-glue-architecture.md`, `casting-protocol.md`, `protocol-model.md`,
`protocol-types.md`, `layout-contracts.md`, `ir-layers-and-naming.md`.
**Feeds:** Golden Rules 2 (derive the fastest default), 3 (no hidden special
treatment), 14 (stdlib is the extension mechanism), 15 (no type-name matching),
23 (no vocabulary matching), 24 (proofs, not shapes).

---

## 1. The goal, stated as a test

> Delete `lib/glue/c.bv`. Add `lib/glue/c/glue.dbvl` describing C. The compiler
> generates every protocol, cast lane, marshalling decision and header from that
> file, and the result is **byte-identical IR** plus unchanged benchmark
> numbers. Then add Python the same way — if that needs a Rust change, the
> schema is wrong.

Two corollaries that keep the plan honest:

- **Efficiency is not a descriptor property.** It comes from proof-licensed
  zero-copy: the descriptor declares *representation*, the prover licences
  *safety*, codegen takes the cheapest legal path. A descriptor that forces a
  copy where the representations coincide is a bug, not a trade-off.
- **Hardcoding must not be able to win.** If a hand-written glue path beats the
  generated one, the descriptor is missing a fact or the generator is naive —
  either way it is fixed by improving them, never by keeping the hand-written
  path.

---

## 2. What exists today (measured)

### 2.1 The pipeline, with the numbers that constrain everything

`src/compile.rs`, in order:

| Line | Step | Consequence for metaprogramming |
|---|---|---|
| 326 | `pm.run_ast(StageKind::Parsed)` | first AST hook; before resolution and typecheck |
| 401 | `pm.run_ast(StageKind::Resolved)` | last hook *before* the typechecker |
| 469 | `register_typedefs` | declared types enter the universe here |
| 501 | `check_types(...)` | **the only** typecheck in the build path |
| 608 | `boundary_marshalling::rewrite_boundary_marshalling` | protocol **axioms** rewrite boundary casts here |
| 657 | `pm.run_ast(StageKind::Typed)` | first hook with typed information |
| 738 | `pm.run_ast(StageKind::Normalized)` | normalizer has run |
| 838 | `verify_bodies` | proof gate |
| 907 / 960 / 1000 | `Verified` / `Allocated` / `Provenanced` AST hooks | later AST mutators |
| 1026 | `frgn_dispatch::resolve_single_frgn` per `frgn` | **after** every AST hook |
| 2158 | `analyze_program` → `build_hir` → emission | HIR is built **from the post-hook AST** |

Two facts fall out, and they decide the design:

1. **A generated protocol must exist before line 608** to be rewritten into
   binding calls, and before line 501 to be typechecked. That means `Parsed`
   (326) or `Resolved` (401) — *not* `Typed`.
2. **The typechecker runs exactly once and nothing re-checks after the AST
   hooks.** Anything synthesised at `Typed` and later is trusted code.

### 2.2 What the macro layer can already do

- **11 stages**, `PreLex … Linked`; AST stages are `Parsed … Provenanced`,
  IR stages `Generated … Linked` (`src/plugin/mod.rs:1-14`).
- **Synthesis**: `Quote$` parses a *string* into a `TopLevel` node
  (`src/macros/eval.rs:1275`) and `Insert$` splices it into the live program
  (`:1079`). Together they can synthesise any declaration, including
  `proto … { axiom CastTo(..) = f(#Lh); };`.
- **Layout as data**: `InjectTypeLayout$` takes `(type, size, [name,type,offset]…)`
  (`:1527`) — the exact hook a layout-agnostic language needs.
- **Introspection**: `TypeInfo$`, `CastPath$`, `ConfigGet$`, `FileRead$`,
  `EmitError$`/`EmitWarning$`/`EmitInfo$`.
- **Hygiene**: capability sandbox, gas budget, in-memory VFS (`FileWrite$`),
  macro lockfile, tainted-node filtering (generated nodes are excluded from
  `All$`/`Tag$`), transactional execution with rollback, expansion traces.
- **Compile-time functions**: `$defn`/`$txn` execute at compile time and are
  extracted before codegen.

### 2.3 The HIR layer — and why the question is not academic

HIR is built *after* every AST hook (`build_hir`, `src/backend/mod.rs:781`),
and **HIR emission is the default** (`hir_emit_enabled`, `src/backend/mod.rs:181`).
But HIR models strictly less of the boundary surface than the AST does:

| AST | HIR | Effect |
|---|---|---|
| `TopLevel::ProtocolDef` — protocols, variants, `cast_edges`, `cross_ops` | only `HirProgram::protocol_binding_fns: Vec<String>` (`src/hir/program.rs:367`), collected by `lower_protocol_bindings` (`src/hir/lower.rs:131`) | **the protocol structure is not representable**: no protocol name, no variant, no direction, no op contract |
| `ForeignBinding { from: FromSpec, target, buffer_mode, precondition, postcondition, default_watchdog, error_type/error_fields, is_optional, is_fire_forget, is_delivery, variadic }` | `HirForeign { name, link_symbol, params, ret }` (`src/hir/program.rs:229`, lowered at `src/hir/lower.rs:445`) | the entire FFI *contract* is dropped: where the symbol comes from, how it is called, what it may do, what it returns on error |
| `TypeDef.protocol` / `.parent` | kept (`HirTypeDef`) | this is why the destination model can still classify `CStr` boundaries today: the protocol lives on the **type declaration**, not on the axiom |

So today the HIR path is correct only because of **side channels**: AST-derived
`resolved_frgns` (`compile.rs:1026`, handed to the backend) and the `TypeUniverse`
`Cast.*` properties. Nothing in HIR itself states "this call is a C `char*`
borrow". A generated protocol is invisible to HIR; a generated `frgn`'s
`from`/ownership/discipline is invisible to HIR.

### 2.4 The existing descriptor is real, and export-shaped

`lib/glue/<lang>/glue.dbv` is loaded by name (`config/protocols.dbvl`: any
`#<Name>` protocol is a GLUE target resolved to `lib/glue/<name>/glue.dbv`), and
`GlueTarget` (`src/glue/config.rs:31-104`) already carries:

```
language, types_module, extension, bridge_kind, calling_convention, module_init,
protocols: { <Category>: { native, c_abi, wasm_abi } },
conversions: { to_abi, from_abi }, state: { decl, arg, ffi_type },
param_decl, fn_param_decl, templates{…}, native_{cc,prefix,suffix,include,link}…,
host_fns
```

Six languages exist (c, csharp, go, java, lua, node). It is consumed by
`frgn_dispatch` (path selection) and `glue/export.rs` (bindings generation).

What it does **not** carry — the gap this plan closes:

- **no ownership per position** (borrowed-for-call / transferred-in /
  transferred-out / retained);
- **no layout** for aggregates (size, offsets, alignment, stride) — that lives
  in `InjectTypeLayout$`, `packed.rs`, and runtime probes in `lib/std/dwarf.bv`;
- **no call discipline** (blocking? reentrant? callback allowed? GIL/main-thread
  affinity?) — invisible to the reactor and to the thread gate;
- **no error convention** (errno vs status vs sentinel);
- **no capability surface** (structs by value? callbacks? out-params? variadics?);
- and `ProtocolEntry` holds only type *names*: `{ native, c_abi, wasm_abi }`.

### 2.5 Where the import direction's semantics actually live

Hand-written Briev: `lib/glue/c.bv` declares `CStr: String<C_String>` and
`proto C_String: String { axiom CastTo(..) = cstr_to_briev(#Lh); axiom
CastFrom(..) = str_to_c(#Lh); op Concat(..) = cstring_concat(#Lh,#Rh); }`, with
`str_to_c` returning `handle + 8` (a zero-copy view). Those axioms are what
`rewrite_boundary_marshalling` (line 608) and the casting graph consume; the
round-trip obligation `CastFrom∘CastTo == id` is already *proved*
(`analysis/protocol_graph.rs:134-146`).

That is the machinery the plan must generate from data — not replace.

---

## 3. Verdict: three gaps, in dependency order

**G1 — Stage window.** Protocol-generating macros must run at `Parsed`/`Resolved`;
`Quote$`-synthesised items at `Typed` or later are too late for
`boundary_marshalling` and are never typechecked.

**G2 — Generated code is trusted.** One `check_types` call, no re-check. A wrong
generated `axiom CastTo` silently rewrites every boundary conversion in the
program.

**G3 — HIR is blind to the contract** (§2.3). Emission runs on HIR; the boundary
facts live only in AST side channels. This is the item the user asked about, and
it is not optional: *the FFI contract cannot be a first-class compiler fact until
HIR can represent it.*

---

## 4. The design

Stable axis first; volatile axis last.

### 4.1 Layer 1 — ABI core (shared, one file per target family)

`config/abi/<family>.dbvl` — `sysv-x86_64`, `win64`, `aapcs64`, `riscv64`,
`wasm32`, `nvidia-ptx`:

```
abi: {
  scalars: { int: i64; float: f64; ptr: 64; };
  aggregate: { small_max: 16; in_regs: "eightbyte"; hidden_ptr_threshold: 16; };
  stack: { align: 16; };
  symbol: { prefix: ""; mangling: "none"; };
  temporaries: { caller_frees: true; };
};
```

No language knowledge. This is the only place "i64/double/row-major" may appear.

### 4.2 Layer 2 — boundary value classes (the universal vocabulary)

Eight classes, fixed by the compiler, not by language:

```
value scalar(layout)
value ptr(layout)
value borrowed_bytes(len)      // callee must not retain
value owned_buffer(len)        // ownership transfers; who frees is declared
value opaque_handle(kind)      // uninterpreted; only ops below may touch it
value aggregate(layout)        // size/offsets/alignment/stride
value callback(fn, env)        // callable + user data; ABI form per language
value error_channel(kind)      // errno | status | sentinel | exception
```

Every language maps onto these; nothing in the compiler may name a language.

### 4.3 Layer 3 — language adapter (per language, small)

Extends the existing `glue.dbv` with the sections it lacks. This is the whole
delta on today's schema:

```
abi: "sysv-x86_64";              // which ABI core
values: {
  String: { class: borrowed_bytes;  zero_copy: true;
            to:   { op: str_to_c };              // Briev → boundary
            from: { op: cstr_to_briev; copy: true }; };
  List<T>: { class: borrowed_bytes; zero_copy: true;
             to: { op: list_to_c_ptr }; len: { op: list_len }; };
  Bool:    { class: scalar; layout: i8 };      // C _Bool
  CDouble: { class: scalar; layout: f64 };
  PyObject: { class: opaque_handle; kind: pyobj;
              retain_op: py_incref; release_op: py_decref; };
};
frees: { owned_buffer: "caller" };
discipline: { blocking: false; reentrant: false; needs_main_thread: false; };
errors: { class: status; success: 0; };
supports: [scalar, borrowed_bytes, owned_buffer, opaque_handle, aggregate];
```

`supports` is the capability list: a `frgn` requiring a class the adapter does
not list is refused with what/why/fix — the FFI twin of `backend/capabilities.rs`.

### 4.4 Layer 4 — protocol binding (Briev protocol → value class → ops)

The generated `proto` items wire the value classes into the casting graph, and
are **the only generated code that carries proof obligations** (round-trip
`CastFrom∘CastTo == id`, which `protocol_graph.rs` already discharges).

---

## 5. Metaprogramming as the generator

One generator, in Briev, per the macro-system contract (every `$` intrinsic it
uses must justify three non-GLUE uses; composition happens in `.bv`).

- **Stage `Resolved`** (before typecheck and marshalling — G1). Needs
  `ConfigGet$`/`FileRead$`/`Quote$`/`Insert$`/`InjectTypeLayout$`; none of them
  needs typed information, so the early stage costs nothing.
- **Emits**: (a) one `proto` per value class with its `CastTo`/`CastFrom`
  axioms naming the adapter ops; (b) the adapter `defn`s (or `frgn` declarations
  for ops the runtime provides); (c) `InjectTypeLayout$` for every aggregate;
  (d) `frgn` declarations the adapter says exist; (e) export headers/stubs from
  the existing `templates` (unchanged path).
- **Provenance**: generated items keep their origin (`Quote$` span + trace +
  lockfile entry), so a diagnostic inside generated glue names the descriptor
  line that produced it — not a synthetic span. Without this the "helpful
  diagnostics" rule collapses exactly where it matters.
- **Taint discipline**: generated nodes stay tainted (excluded from `All$`/`Tag$`)
  and are additionally recorded in the lockfile, so a re-run is diffable.

### 5.1 G2 — re-check what was generated

Two candidate fixes; the plan takes (b):

- (a) Move generation before `check_types` and accept that generated code is
  typechecked with the rest — but `Resolved` *is* before `check_types`
  (line 401 vs 501), so **generation already happens before typecheck**. The
  real gap is only for items synthesised at `Typed`+.
- (b) **Refuse `Typed`-and-later synthesis of `proto`/`TypeDef`/`frgn`**: a stage
  block that inserts a declaration at a stage where it cannot be checked is a
  compile error naming the correct stage. This closes G2 without a second
  typecheck pass, and it is a rule the macro layer can enforce mechanically
  (it already distinguishes generated nodes).

Body-bearing `defn`s (adapter helpers) are the one exception: they may be
generated at `Typed`, and the plan requires them to pass the existing
`verify_bodies` gate (line 838), which already runs after that stage.

### 5.2 G3 — make the contract a HIR fact

**Recommendation: extend HIR; do not keep the side channel.**

Add to `src/hir/program.rs`:

```rust
pub struct HirProtocol {            // one per `proto`
    pub name: String,
    pub base: String,               // e.g. "String"
    pub variants: Vec<HirVariant>,  // `String<C_String>`-style sub-protocols
    pub casts: Vec<HirCastEdge>,    // direction (To/From), target protocol, binding fn
    pub ops: Vec<HirProtocolOp>,    // op name + binding fn (Concat, …)
}

pub struct HirBoundary {            // extends HirForeign
    pub from: HirFromSpec,          // Literal | Registry | Protocol | Linked
    pub classes: Vec<HirValueClass>,// per-parameter + return
    pub ownership: Vec<HirOwnership>,// borrowed | transferred_in | transferred_out | retained
    pub discipline: HirDiscipline,  // blocking, reentrant, main_thread
    pub errors: Option<HirErrorChannel>,
    pub supports_ok: bool,          // resolved against the adapter's `supports`
}
```

and lower `TypeDef.protocol`/`.parent` unchanged (that path already works).

**Fidelity gate**: `lower_program_checked` (already exists, `compile.rs:446`)
becomes a *required* check on programs that contain generated items, and the
plan extends it so an unsupported boundary construct is reported rather than
dropped. Precedent: the HIR-fidelity work that added `Within`, `Exists`,
`FormattingAnnotation`, `InlineDefn` and kept `DerivationBlock` an explicit gap —
this is the same discipline, applied to the boundary surface.

**Why not Option B (keep the side channel)?** Because then the descriptor's facts
are consumed by AST analyses while emission runs on HIR: two sources of truth for
the same decision, drifting silently. That is the failure this plan exists to
prevent.

---

## 6. Phases

Each phase is independently verifiable, and each ends with a commit. **S0 gates
the rest**: if S0 fails, the design (not the plan's phases) needs rewriting.

### S0 — Spike (1 day, throwaway branches allowed)

Three questions, answered by measurement:

1. Does a `$(Resolved)` `Quote$`-generated `proto C_String { axiom CastFrom(…) =
   str_to_c(#Lh); }` reach `rewrite_boundary_marshalling` and produce the same
   IR as the hand-written `c.bv`?
2. Does `InjectTypeLayout$` for an aggregate flow into aggregate argument passing
   (SysV eightbyte classification)?
3. Does `lower_program_checked` on that program report zero unsupported boundary
   constructs — i.e. is today's HIR *good enough to notice* what it is missing?

Deliverable: a short findings appendix in this plan. **Gate:** (1) and (3) must
be yes; (2) may be "needs S3".

### S1 — Descriptor v2 (schema + parser + validation)

Extend `GlueTarget` with `abi`, `values`, `frees`, `discipline`, `errors`,
`supports`; keep every existing key working (six languages must not regress);
validate at load time with house-style errors. No behaviour change yet.

**Gate:** all six `glue.dbv` files load; `tests/glue_*` unchanged and green.

### S2 — ABI core files

Add `config/abi/*.dbvl` and the loader; make `ProtocolEntry`'s `c_abi` default
derive from the ABI core instead of being restated per language.
**Gate:** IR byte-identical for every glue test (pure refactor).

### S3 — Generator (Briev, stage `Resolved`)

Generate protocols, adapter ops, layout injections from the descriptor; keep the
hand-written `c.bv` in place behind a flag.
**Gate:** `c.bv`-generated vs hand-written → **byte-identical IR**; all six
language bridges build; `--diff` shows only generated items.

### S4 — HIR fidelity for the boundary surface

`HirProtocol` + `HirBoundary`; `lower_program_checked` extended; the destination
model reads `HirBoundary` instead of reconstructing from `HirForeign` +
`hir_type_protocols`.
**Gate:** destination-model classification for `CStr` unchanged (existing tests);
new tests for ownership/class per position; HIR/AST drift assertions green.

### S5 — Stage guard (G2) + generated-item diagnostics

Refuse `proto`/`TypeDef`/`frgn` synthesis at stages later than `Resolved`;
body-bearing generated `defn`s must pass `verify_bodies`; diagnostics name the
descriptor origin.
**Gate:** compile-fail fixtures for both refusals; a generated-axiom mistake
reported with the descriptor line.

### S6 — Second language (Python), the closure test

Implement Python purely as data + generated glue. **If any Rust change is
needed, the schema is wrong** — that is the test, not a stretch goal.
**Gate:** CPython bridge works with `PyObject*` ownership (retain/release
declared, not hardcoded); the C path stays byte-identical.

### S7 — Retirement + docs

Delete `c.bv`'s hand-written axioms; update docs (§7); record the acceptance
evidence in the descriptor's own comments.

**Parallel track D — stale surface sweep** (independent of the above, do it when
the compiler is otherwise busy): `#out`, `#frgn`, `frgn` without `from`,
top-level `term` — one removed-surface diagnostic path, one inventory from
`src/vocab.rs`, one live-file gate. `#!exit` (commit `bddf015ea`) is the pattern.

---

## 7. Spec and documentation to update

| Document | Change |
|---|---|
| `spec/SPEC.md` | new normative chapter: **Boundary contracts** — value classes, ownership per position, ABI cores, capability refusals; revise §19 (`frgn`) and the protocol chapter to reference the descriptor; state the stage rule for generated declarations |
| `docs/architecture/glue-as-abi-generator.md` | promote from insight to the operational model: descriptor → generated protocols → proof-licensed zero-copy |
| `docs/architecture/glue-ffi.md`, `glue-pipeline.md`, `frgn-export-glue-architecture.md` | import direction now descriptor-driven; ownership/discipline sections |
| `docs/architecture/macro-system.md` | the stage rule (what may be synthesised where), provenance requirement, and the generated-item re-check |
| `docs/architecture/ir-layers-and-naming.md` | HIR gains the boundary surface: `HirProtocol`, `HirBoundary`; what remains AST-only and why |
| `docs/architecture/protocol-model.md`, `protocol-types.md`, `casting-protocol.md` | protocols as generated data; the round-trip obligation becomes a *generated* obligation |
| `docs/architecture/layout-contracts.md`, `docs/architecture/intrinsics-vs-stdlib.md` | layout injection is the boundary-layout mechanism; descriptor is stdlib-side (survives `--no-stdlib`) |
| `docs/architecture/keyword-taxonomy.md` | no new keyword: the contract is data, not syntax (record the decision) |
| `docs/reference/MASTER-SYNTAX-REFERENCE.md`, `LEGACY-SURFACE-INDEX.md` | track D's removals |
| **new** `docs/architecture/ffi-contract.md` | the reference doc this plan produces |

---

## 8. Risks and non-goals

- **Risk: descriptor drift.** Two places describing the same conversion (data and
  a surviving `.bv`) is worse than one. Mitigation: S3 deletes as it generates,
  and the acceptance test is byte-identical IR, not "looks right".
- **Risk: managed-runtime cost is not marshalling.** GIL handoff, pinning, and
  loader lifecycle dominate; if the descriptor cannot state them, adding Python
  will look deceptively cheap and perform badly. S6 must measure, not just pass.
- **Risk: HIR expansion dilutes the IR's purpose.** Mitigation: only *boundary*
  facts enter HIR, and only ones a pass consumes (the derivation-not-recognition
  rule: a declared fact with no consumer is a liability).
- **Non-goal:** C++ name mangling, exceptions across the boundary, and
  header-parsing (cbindgen-style discovery). Declared contracts only; discovery
  is a separate plan.
- **Non-goal:** replacing `#Link`/`from #System` resolution.

## 8a. S0 spike — measured results (2026-11-16)

The spike asked three questions before the design commits to anything. Two are
answered; one is deferred as the plan allowed.

### (a) A `Resolved`-stage generated protocol **does** drive boundary marshalling — PASS

Fixture (throwaway): a `$(Resolved)` block that `Quote$`s a boundary type and
`Insert$`s it after the last top-level item, then a `frgn` + a cast to that
generated type:

```briev
$(Resolved) {
    Insert$(All$().Last$().After$(), Quote$("type CStrG: String<C_String> { };"));
};
import "glue/c.bv";
frgn c_strlen(p: Int) -> Int from #Link<libc>;
node main [!done][done] {
    let s: CStrG = "hi" as CStrG;
    Print#(c_strlen(s as Int));
    done = true; endprogram;
};
```

Emitted IR contains the zero-copy lane, not a copy:

```llvm
%t11 = call i64 @str_to_c(ptr %t13)      ; the axiom's binding, generated type in play
%t16 = call i64 @c_strlen(i64 %t16)
```

So the ordering claim in §2.1 holds in practice: a macro-generated `proto` /
sub-protocol type is visible to `rewrite_boundary_marshalling` (line 608) and to
the casting graph, and produces the same marshalling a hand-written `c.bv`
declaration would. **G1's fix is a stage rule, not a new mechanism.**

Incidental finding: `Quote$` is not documented in
`docs/architecture/macro-system.md` (the "AST Constructors" table lists only
`Import$`, `Defn$`, `Call$`, `Block$`), which is why the spike needed code
reading to find it. Docs debt for §7.

### (b) `InjectTypeLayout$` → aggregate argument passing — NOT MEASURED

Needs a struct passed by value across the boundary, which the current surface
reaches only through layout metadata plus an `frgn` signature — i.e. after S1/S3
give the descriptor somewhere to state it. Deferred as the plan allowed; it is
the one open risk in S3.

### (c) HIR **silently drops** the boundary surface — CONFIRMED, and worse than assumed

`BRIEV_HIR_AUDIT=1` on the same fixture:

```
HIR_AUDIT ok callables=210 types=27 values=731 sinks=2218 unsupported=0
```

`unsupported=0`. The program carries a generated protocol, an `frgn` with a
`#Link` source, and a boundary cast — and the lossless HIR audit reports
*nothing missing*. G3 is therefore not just "HIR cannot represent the contract";
**HIR does not notice that it cannot.** `lower_program_checked` cannot serve as
the fidelity gate for boundary facts until S4 extends it; using it as-is would
certify a program whose FFI contract was dropped on the floor.

Operational consequence for S4: the gate must be built *with* the nodes, and its
first red test should be this very fixture — today's `HIR_AUDIT ok` on a
boundary-bearing program is the regression to fix.

## 9. Decisions — recommended defaults (2026-11-16)

Each answer below is chosen for long-term language health: minimum permanent
knowledge in the compiler, minimum silent failure, maximum visibility at the
site where a fact matters. They supersede §9's original open questions.

### D1. Ownership is declared **per function**, not per language

**Decision.** The *class* of every boundary position (borrowed /
transferred-in / transferred-out / retained) is declared on the `frgn` (or on
the generated boundary signature), not in `glue.dbv`. The descriptor supplies
only the language's **conventions and ops**: who frees by default, and which
operations realise `retain`/`release` (e.g. `py_incref`/`py_decref`).

**Why.** Ownership is a property of the *function*, not the language: in one C
header `getenv` returns borrowed and `strdup` returns owned, with identical
types. Putting it in the descriptor forces a per-function fact into a
per-language file — unauditable exactly where it matters, and it would grow a
per-function exception list inside a language file. Declaring it at the call
site also gives the three consumers what they need without translation: the
destination model (already classifies `ZeroCopyWithLifetimeProof` /
`CopyAtBoundary`), the aliasing/lifetime proofs, and the thread gate.

This also matches the existing precedent — `frgn` already carries per-function
`precondition`, `postcondition`, `buffer_mode`, `error_type` — so ownership is
not a new kind of knowledge, just the missing member of an existing family.

**Guard.** The descriptor's `supports` list is consulted *before* the site
declaration: a class the language cannot honour is refused, rather than silently
downgraded.

### D2. ABI cores ship in `config/abi/`; language adapters stay in `lib/glue/`

**Decision.** ABI cores (`sysv-x86_64`, `win64`, `aapcs64`, `riscv64`, `wasm32`,
`ptx`) live in compiler config, validated at load. Language adapters stay where
they already are (`lib/glue/<lang>/glue.dbv`), so a project can add a language
without forking the compiler.

**Why.** The repository's existing dividing line is `--no-stdlib`: anything a
program needs without stdlib is compiler-side; everything else may be stdlib
data. A foreign call is a language feature (the parser owns `frgn`), so its ABI
must work with `--no-stdlib` — therefore config, not stdlib. The two roles are
also asymmetric in blast radius: a wrong ABI core is a **miscompile**, while a
wrong language adapter is a **refused or wrong bridge** in one project. Shipping
ABI rules per project would invite the first by accident.

The symmetry is deliberate and already present in the tree:
`config/protocols.dbvl` (compiler) alongside `lib/glue/*.bv` (stdlib).

### D3. The descriptor is versioned, and unknown keys are errors

**Decision.** Add `version: <N>`; the compiler declares the range it accepts and
refuses anything outside it. Within a supported version, **an unknown key is an
error**, not a silent ignore. Missing *optional* keys take documented defaults;
missing *required* keys error.

**Why.** Today's parser reads known keys via `entry.get(name)` and silently
ignores everything else, with no version field (`src/glue/config.rs:310-380`).
Once the file carries semantics (ownership conventions, discipline, capability
list), that is a silent-wrong-answer generator in both directions: a new
descriptor read by an old compiler loses `discipline` (so a blocking call looks
non-blocking to the reactor), and an old descriptor read by a new compiler takes
defaults for `values`, turning every boundary into an unknown class. A version
plus unknown-key rejection converts both into a loud error, which is the only
acceptable failure mode for a file that decides how a program talks to the
outside world.

The "unknown key is an error" half also buys typo protection for free — the
descriptor is hand-written data, and `discpline:` silently meaning "no
discipline" is exactly the class of bug this plan exists to prevent.

### D4. Track D splits: the **gate** lands before S3, the sweep runs anywhere

**Decision.** D1 (inventory + one removed-surface diagnostic path + live-file
gate) lands before S3 writes any generator template. D2 (sweeping remaining
files: `#out`, `#frgn`, `frgn` without `from`, top-level `term`) runs in
parallel and may finish later.

**Why.** The generator emits Briev *text* through `Quote$`. If templates exist
before the gate does, nothing stops them from baking in a removed form, and the
failure would surface only when someone re-runs generation on a touched file.
The gate is cheap ("is this live file using a form in the removed list?") and
makes the class closed rather than recurring — the `#!exit` treatment
(`bddf015ea`) generalised.

### D5. The descriptor may declare **facts**, never **strategies**

**Decision (new, not in the original list).** A descriptor key may state what
something *is* (representation, ownership convention, layout, discipline,
capability). It may never state a *preference*, cost, or "use the fast path"
hint. Any fast path is derived: representations coincide **and** the prover
licenses it.

**Why.** Rule 2's corollary — a keyword (here: a config key) that makes code
faster is a failing default. If `glue.dbv` could say "prefer zero-copy", it
would become a place where performance is asserted rather than proved, and the
day it is wrong nobody would notice (the code still runs, just copies). Keeping
the descriptor fact-only preserves the property the whole plan rests on:
**efficiency is a consequence of proven representations, never a declaration.**

### D6 (added while deciding). Generated items must be traceable to a descriptor line

**Decision.** Every generated protocol/`defn`/`layout` records
`(descriptor file, key path, line)` in the trace, and diagnostics inside
generated code name that origin. Generation without provenance is refused.

**Why.** The house rule is that every user-facing diagnostic says what is wrong
and how to fix it. For generated glue the fix is always "change this line of the
descriptor", so a diagnostic that points at synthetic spans is useless. This is
also what makes S3's byte-identical-IR gate auditable: the trace shows exactly
which descriptor key produced which IR construct.

---

## 10. What these decisions change in the phases

| Phase | Effect |
|---|---|
| S1 (descriptor v2) | adds `version` + unknown-key rejection (**D3**), `values`-free ownership (**D1**), `supports` (**D1/D5**) |
| S2 (ABI cores) | location fixed to `config/abi/` (**D2**) |
| S3 (generator) | blocked until D1-of-track-D (the removed-surface gate) is green (**D4**); must emit provenance (**D6**) |
| S4 (HIR fidelity) | `HirBoundary.ownership` is per-position, not per-language (**D1**) |
| S5 (stage guard) | also refuses descriptor strategies if any ever appear (**D5**) |
| Track D | D1 before S3, D2 parallel (**D4**) |

---

## 11. Refinement after S0 (2026-11-16)

S0 changed three things in the sequencing, and one in a decision.

### 11.1 A new S0.5, before any descriptor work

S0(c) showed HIR reports `unsupported=0` on a boundary-bearing program. The
cheapest way to lock that commitment is a **red test now**, before S1:
`tests/` gains a fixture (the S0 spike program) and an assertion that the HIR
boundary audit *does not* report "ok" while the contract is unrepresented. It
fails today by design, and is `#[ignore]`d with a pointer to S4, so it cannot be
forgotten and cannot block CI in the meantime. S4's job is to make it green by
adding the nodes — not by deleting the test.

Rationale: a plan whose central claim ("HIR must carry the contract") has no
executable statement is a wish. One ignored test is the smallest honest anchor.

### 11.2 D6 amended: trace to a **key path**, with a line by reverse lookup

D6 said "traceable to a descriptor line". `src/dbriev/v2.rs:88` shows
`DataValue` is `String | Int | Float | Bool | List | Map` — **no spans** — and it
is shared by every `.dbvl`/`.dbv` consumer (`config_tuning`, `protocols`,
`targets`, …). Adding spans is a parser-wide change with no other customer.

Decision: trace by `(descriptor file, key path)` — e.g.
`lib/glue/c/glue.dbvl : values.String.to.op` — plus a line number obtained by
searching the descriptor text for the key literal. That is honest (it names the
file and the key), needs no parser change, and can be upgraded to real spans
later if another feature ever wants them. The diagnostic wording must not claim
a precise line when it is a lookup: name the key path as the authoritative
location and the line as "near".

### 11.3 Order

| Phase | Change from §6 |
|---|---|
| **S0.5** new | HIR boundary audit red test (ignored, pointing at S4) |
| S1 | unchanged in intent; adds `version` to the six descriptors |
| S2 | unchanged |
| S3 | generator emits key-path provenance (D6 as amended) |
| S4 | turns S0.5's test green; `HirProtocol` + `HirBoundary`; the audit's "ok" must become "missing boundary facts: N" |
| S5–S7 | unchanged |

### 11.4 S1 made concrete

`src/glue/config.rs` only; no call-site changes if the new sections are additive:

1. `GlueTarget` gains `version: i64`, `values: HashMap<String, ValueClass>`,
   `frees: HashMap<String, String>`, `discipline: Discipline`,
   `errors: Option<ErrorChannel>`, `supports: Vec<String>`.
2. Per-level known-key sets (target, `values` entry, `discipline`, `errors`,
   `state`, `protocols` entry, `conversions`) and a rejection path:
   `unknown key 'discpline' in lib/glue/c/glue.dbvl (target 'c') — known keys: …`.
3. `version` outside the compiler's accepted range → error naming both.
4. The six descriptors under `lib/glue/*/glue.dbv` gain `version: 1`, and nothing
   else — so the change is provably additive.
5. Tests: an unknown key rejected; a missing `version` rejected; a version too
   new rejected; all six descriptors load and every existing glue test stays
   green (they exercise `frgn_dispatch` path selection and export generation).

**Gate for S1:** six descriptors load; `tests/glue_*` unchanged and green;
three new negative tests pass; no IR change (pure schema).

---

## 12. Progress log

### S1 — DONE (2026-11-16)

`src/glue/config.rs` only, additive:

- `GlueTarget.version` + `GLUE_SCHEMA_VERSION = 1`; `validate_version` refuses a
  missing, non-integral, or out-of-range version, naming the supported range.
- **Closed keys**: `validate_target_entry` → `validate_section` /
  `validate_named_entries` / `validate_direction` reject an unknown key at every
  level the compiler reads, with the key *path* in the message
  (`c : values.String`) — `.dbv` values carry no spans (`dbriev/v2.rs:88`), which
  is why D6 traces by path.
- The boundary-contract sections (`values`, `frees`, `discipline`, `errors`,
  `supports`) are **declared and parsed**, not yet consumed — so a descriptor can
  state the contract before S3 gives it consumers. Pinned by
  `boundary_sections_parse_when_well_formed`.
- All eight `lib/glue/*/glue.dbv` descriptors gained `version: 1;` — nothing else.
- Three negative tests pin D3: missing version, future version, unknown key at
  target and at section level.

**Gate met:** 12 `glue::config` tests, `tests/glue_bridge_tests` (13) and
`tests/glue_test` (11) green; `scripts/gate.sh quick` pass
(`20261116T195810Z-3e45437d48c1`); no emitter reads `version`, so IR is
unchanged by construction.

**Deviation from §11.4:** the six-field `values`/`discipline`/`errors` typed
model did not land here — validation runs on the raw entry map, which is enough
to close D3. The typed structs belong with S3, where something consumes them;
modelling them now would be unused knowledge (the derivation-not-recognition
rule: a declared fact with no consumer is a liability).

### S0.5 — folded into a BUGS entry (2026-11-16)

The planned "red test before S1" could not be honest without the S4 field it
would assert on. Replaced by `BUGS.md` §"HIR drops the FFI boundary surface",
which records the measurement (`unsupported=0`), the two structural causes
(`HirTypeDefKind` has no protocol; `HirForeign` drops `from`/contracts/flags),
and names the S0 fixture as S4's acceptance test.

### S2 — DONE (2026-11-16)

- `config/abi/{sysv-x86_64,win64,aapcs64,riscv64,wasm32,ptx}.dbvl` — the machine
  axis as data: pointer/int/float widths, stack alignment, aggregate passing
  (size threshold, classification, hidden-pointer threshold), symbol decoration,
  and whether the caller frees returned memory.
- `src/glue/abi.rs` — `AbiCore` + `load_core` / `load_core_from` / `parse_core` /
  `core_names`, with **closed keys** and named-field errors. Four tests: every
  shipped core parses and its `name` matches its file stem; the aggregate rules
  differ per family (wasm has none, SysV passes ≤16 bytes in registers); an
  unknown key is refused with its section; a missing integer is refused by name.
- `GlueTarget.abi: Option<String>`; `validate_abi_core` refuses a name that is
  not shipped and lists the known cores. `lib/glue/c/glue.dbv` declares
  `abi: "sysv-x86_64"`.

**Form finding.** The cores use the flat `.dbvl` line-table shape
(`key: value;` at top level, as `config/ir-lowering.dbvl` does) rather than the
nested form the glue descriptors use: nested quoted-mode maps reject bare integer
values ("Expected named field"), and ABI facts are integers. Recorded in the file
headers so the next author does not rediscover it.

**Gate met:** `glue::` 69/69, `tests/glue_bridge_tests` 13/13, `tests/glue_test`
11/11, `scripts/gate.sh quick` pass (`20261116T201939Z-7e8039ddc0f4`); Praetor
clean on both touched files.

**Scope note.** Only the C adapter names a core. The managed-language adapters
carry host-FFI type names (`ctypes.c_int64`, `jlong`, `IntPtr`), not a machine
ABI — declaring a core for them would assert something false. Deriving `c_abi`
defaults from the core (§4.1's "no restatement") therefore waits for S3, where
the generator knows which side of the boundary a type sits on.

### S3 prerequisite found (2026-11-16) — the .dbvl parser nests two levels

Trying to state the C adapter's boundary value classes in the descriptor failed:

```
values: { "String": { class: "borrowed_bytes"; to: { op: "str_to_c" }; … } };
```

→ `Expected named field at position 825`. The quoted-mode `.dbv` parser handles
**two** levels (`values: { "String": { … } }`, exactly like
`protocols: { "String": { native; c_abi } }`); `values.<Class>.to` is a third and
is rejected. Bare identifiers are also not valid values (they must be quoted), so
the first attempt failed twice for two different reasons.

Two ways forward, to pick in S3 rather than here:

1. **Flatten the schema** (recommended): `values: { "String": { class;
   zero_copy; to_op; from_op; from_copy; len_op } }` — two levels, no parser
   change, and the field names say "op" so the value is obviously an operation
   name rather than a nested record. The `values.<Class>.to` shape in §4.3 and in
   `glue-ffi.md` is documentation-only today, so flattening costs nothing.
2. Extend the quoted-mode parser to arbitrary nesting — a change to a parser
   every config file shares, for one consumer's convenience.

Until this is decided the descriptor carries `version`, `abi`, and the existing
sections only; the `values`/`discipline`/`errors`/`supports` keys are *accepted and
validated when flat*, but no shipped adapter uses them yet.

### S3 step 1 — DONE (2026-11-16): flat value classes, one entry

The flattening decision is implemented and the C adapter now carries its first
real contract datum:

```
c: { version: 1; abi: "sysv-x86_64";
     values: { "String": { class: "borrowed_bytes"; zero_copy: true;
                           to_op: "str_to_c";
                           from_op: "cstr_to_briev"; from_copy: true } }; … }
```

**Only `String` gets an entry.** The scalar classes are already determined by the
protocol plus the ABI core (S2) — writing `Int: { class: scalar; layout:
"int_bits" }` would restate a fact the compiler derives from the core, which is
exactly what D5 forbids. `String` is the class with genuine asymmetry: outbound is
zero-copy (the `[len][bytes]` data region *is* a C string), inbound must copy (a C
string carries no length prefix).

Validator changes: `values.<Class>` keys are now `class, zero_copy, layout, len,
len_op, to_op, from_op, from_copy, retain_op, release_op, kind`; the
nested-direction walker (`values.<Class>.to`) is deleted as dead.

**Second parser finding.** A multi-class `values` block in the shipped adapter
still failed to parse (`Expected named field`) while the same shape inline in a
test parses and a single-class block parses. I did not chase it to the byte: the
one-class form is the better design anyway (see above), and the failing variant is
no longer in the tree. Recorded so the next author knows that *multi-entry maps
inside a target entry* have a parser limit worth finding before bulking up a
descriptor — the same class of surprise as the nesting limit, and the same fix
(capture the exact offset in a unit test, then decide flatten-vs-extend).
