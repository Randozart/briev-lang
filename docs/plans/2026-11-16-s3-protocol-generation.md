# S3 — Generate the boundary protocols from the descriptor

**2026-11-16.** Status: **plan — research complete, implementation not started.**
Scope: the half of S3 that can be finished and proved: emit the *protocol* half of
a language adapter from `glue.dbv` and prove the emitted program is byte-for-byte
the same machine code as the hand-written one. The *operation* half
(`str_to_c`, `cstr_to_briev`, …) stays hand-written in this phase.

**Parent plan:** `2026-11-16-ffi-contract-and-metaprogramming.md` (S3, decisions
D1–D6). **Companions:** `macro-system.md` (stage rule), `glue-ffi.md`
(descriptor schema), `glue-as-abi-generator.md` (doctrine).

---

## 1. The success test, narrowed

> Split `lib/glue/c.bv` into a hand-written operation half and a generated
> protocol half. Generate the protocol half from `lib/glue/c/glue.dbv`. Compile the
> C-boundary corpus with each and require **byte-identical LLVM IR**.

Narrowing matters. The parent plan's test ("delete `c.bv`, generate everything")
cannot be met first: `str_to_c` and `cstr_to_briev` are *Briev code* — the
generator would have to synthesise function bodies, which is a much larger claim
than "the descriptor knows the protocol". The split test proves the claim that
actually matters now: **the protocol surface is data, not Rust and not
hand-written Briev**.

---

## 2. Findings (measured)

### 2.1 The multi-entry `values` parse failure: hypotheses tested, none confirmed

A single-entry `values` block parses in the shipped adapter; the six-class version
failed with `Expected named field at position 825`. `S3.0`'s characterisation test
(`dbriev::v2::tests::characterise_named_map_shapes`) now pins the shapes that
**do** parse, and every structural hypothesis was refuted by it:

| Shape | Result |
|---|---|
| `a: 1; b: 2;` | parses |
| one nested map as a value | parses |
| nested map with a quoted key (`"X": { … }`) | parses |
| **two** quoted nested entries | parses |
| three quoted nested entries | parses |
| two quoted nested entries, bool in the first | parses |
| **two sibling nested maps** in one entry (`a: {…}; b: {…};`) | parses |

So it is not the entry count, not the nesting depth alone, not the quoted keys,
not booleans, and not sibling maps. The failure is **content-specific**, and the
suspects left are the field *names* and values of the six-class block — notably
`layout`, `zero_copy`, `to_op`, `from_op`, `from_copy` and the class names
`Int`/`Float`/`Bool`/`Data`/`Char`. `class` itself is refuted (it appears in the
passing cases).

**Exact failing text** (preserved for the next session — reinsert it after
`abi: "sysv-x86_64"; ` in `lib/glue/c/glue.dbv` and the same offset appears):

```
values: { "String": { class: "borrowed_bytes"; zero_copy: true; to_op: "str_to_c";
          from_op: "cstr_to_briev"; from_copy: true };
          "Int": { class: "scalar"; layout: "int_bits" };
          "Float": { class: "scalar"; layout: "float_bits" };
          "Bool": { class: "scalar"; layout: "i8" };
          "Data": { class: "borrowed_bytes"; zero_copy: true };
          "Char": { class: "scalar"; layout: "i32" } };
```

**Bisection result (P0, 2026-11-16): the block itself parses.** Three new cases
were added to the characterisation test — six classes with one field each, one
class with five fields, and the six classes with their real field sets — and all
three **pass**. So the failing text is not the `values` block, not its class
names, and not its field names: it is the block **in context**, i.e. inside the
`c: { … }` entry alongside `protocols`, `conversions` and `state`.

That moves the suspect set to entry-level interaction. The next measurement, and
it is one test away: parse the **whole** `lib/glue/c/glue.dbv` text from the test
(read the file, splice the block back in, `parse_document_quoted`) and then delete
one sibling section at a time (`protocols`, then `conversions`, then `state`)
until the offset disappears. Whichever removal fixes it names the interaction.

Practical consequence: the descriptor ships with **one** value class today
(`String`, the one whose asymmetry matters), which side-steps the unknown without
claiming it is solved. `glue::config` is green with that shape.

### 2.2 Where generated output has to live

- `lib/glue/c/` currently holds **only `glue.dbv`** — `GlueTarget.types_module`
  points at `glue/c/types.bv`, which does not exist for C. (The other languages
  reference `glue/<lang>/types.bv`; only some exist.) A dangling path in the
  descriptor is itself a finding: it means the export path silently has no types
  module for C.
- `lib/glue/c.bv` is imported by six call sites (`examples/glue-host/*.bv`,
  `lib/compiler/soa_reorder.bv`), so its **path must stay stable**. The generated
  file therefore goes *beside* it, and `c.bv` shrinks to an import plus the
  operation half:

  ```
  lib/glue/c.bv            hand-written ops + `import "glue/c/protocols.bv";`
  lib/glue/c/protocols.bv  GENERATED — `type CStr: String<C_String>` + the proto
  ```

### 2.3 Command surface

`src/main.rs:38-40` dispatches `export` / `bindings` / `extension` to
`glue::export::run_*_cli`. A `render` subcommand slots in the same place and calls
the new emitter — no new plumbing shape.

### 2.4 The IR gate already has a template

`tests/str_to_c_no_memcpy.rs` builds a fixture with `--llvm` and asserts on the
module text; `benchmarks/ll_determinism_check.sh` compiles a corpus twice and
byte-compares. The parity gate is the second pattern applied to two *different
compilers* (hand-written vs generated) rather than two processes.

---

## 3. Design

### 3.1 The generator is Rust, and that is not a doctrine violation

The parent plan put the generator at Briev stage `Resolved`. Two facts argue for a
Rust emitter first:

- A Briev macro would re-parse `glue.dbv` from `FileRead$` text, giving **two
  parsers of one file** and guaranteed drift. (A `GlueDescriptor$` intrinsic would
  avoid that, but the macro-system contract requires three non-GLUE uses per new
  intrinsic.)
- The doctrine being protected is "adding a language is data, not Rust". A Rust
  emitter contains **no language knowledge** — it walks `glue.dbv` generically.
  The forbidden thing is `if lang == "c"`, not "written in Rust".

So: `src/glue/gen.rs` — `render_protocols(&GlueTarget, &AbiCore, &[DescriptorValue]) -> String`,
pure, no I/O, fully testable; plus a thin CLI wrapper. Migrating it into Briev
later is a *possible* follow-up, not a requirement, and by then the output is
pinned by the parity gate either way.

### 3.2 What it emits

From `values:` (D1's per-position facts live on the `frgn`, so this is only the
*representation* half) plus the protocol sections already in `glue.dbv`:

```
// GENERATED by `brievc glue render c` from lib/glue/c/glue.dbv — do not edit.
// Descriptor: lib/glue/c/glue.dbv : values.String (class, zero_copy, to_op, from_op, from_copy)
type CStr: String<C_String> { };

proto C_String: String {
    axiom CastTo(String<UTF8>)   = cstr_to_briev(#Lh);
    axiom CastFrom(String<UTF8>) = str_to_c(#Lh);
};
```

- **Ordering is sorted**, not map-iteration order (project rule: every collection
  feeding IR is ordered).
- **Provenance** in the header names the descriptor and the key path (D6 as
  amended: key path authoritative, line by lookup).
- The emitted text must be *stable* under re-running: generator output is
  committed, and a `--check` mode fails CI when the file on disk differs from what
  the generator would produce. That is the anti-drift mechanism, and it is what
  makes the generated file reviewable in a diff.

### 3.3 Why commit the generated file

Alternatives considered:

| Option | Verdict |
|---|---|
| Generate at build time into a cache, never committed | No diff to review; a descriptor change that alters IR is invisible until a benchmark moves. Rejected. |
| Commit generated `.bv`, with `--check` in CI | Reviewable, bisectable, and the parity gate has something concrete to compare. **Chosen.** |
| Generate into `OUT_DIR`-style tmp and import from there | Fragile with the import resolver, and adds a path that only exists during a build. Rejected. |

### 3.4 First step is not the generator

The parser finding (§2.1) gates everything: a generator that cannot read a
multi-entry map is untestable against real descriptors. S3.0 characterises the
bug; only then does S3.1 start.

---

## 4. Phases

| Phase | Content | Gate |
|---|---|---|
| **S3.0** | Characterisation test for the multi-entry parse failure: build the minimal failing string in a unit test, assert the error and its offset; then fix the parser (or, if the rule turns out to be a genuine syntax limit, record it and choose the encoding). | The characterisation test fails before the fix and passes after; all `.dbvl`/`.dbv` config tests stay green (the parser is shared — `config_tuning`, `protocols`, `targets` all read it) |
| **S3.1** | `src/glue/gen.rs`: `render_protocols` (pure) + provenance header + sorted output + `--check` comparison helper. Unit tests on a synthetic target: one value class, several, none; a class whose `to_op` and `from_op` differ; a class with `from_copy: true`. | `cargo test --lib -- glue::gen` green; rendered text is stable across runs (compile the generator twice in one test and compare) |
| **S3.2** | CLI `brievc glue render <lang> [--check]`; writes `lib/glue/<lang>/protocols.bv`. | `--check` exits non-zero when the file is stale; zero when fresh; a stale file is *not* rewritten in check mode |
| **S3.3** | Split `lib/glue/c.bv`: move the `type CStr` / `proto C_String` half into the generated file, keep the ops. | `tests/str_to_c_no_memcpy` green; all six import sites still compile |
| **S3.4** | IR parity gate: `tests/glue_gen_parity.rs` compiles the C-boundary corpus twice (hand-written vs generated) and requires byte-identical `.ll`. | Identical bytes, or the descriptor is missing a fact |
| **S3.5** | Docs (§6) + `INDEX.md` + plan progress entry. | — |

Estimated: S3.0 is the unknown (an hour or an afternoon depending on the cause);
S3.1–S3.2 half a day; S3.3–S3.4 half a day; S3.5 an hour.

---

## 5. Risks

- **Shared parser.** S3.0 touches a parser every config file uses. The mitigation
  is the corpus of existing config tests plus `config_tuning`'s ledger; any change
  must keep `glue::`, `config_tuning` and the `.dbvl`-reading tests green in the
  same commit.
- **The dangling `types_module`.** `glue/c/types.bv` does not exist. Either the
  descriptor is wrong or the export path tolerates absence; S3 must decide rather
  than leave a claim that does not hold. Small, but it is the kind of stale fact
  that makes later work look broken.
- **`#Lh`/`#Rh` operator contract.** The generated axioms must reproduce the exact
  operand markers. The parity gate catches a mistake, but only if the corpus
  exercises both directions (out and in) — the corpus must include a C→Briev
  direction, not just Briev→C.
- **Generated code is still unchecked** (G2 from the parent plan, S5). Until S5
  lands, generated protocols at `Resolved` are typechecked (they precede
  `check_types`) — which is the safe side of the gap, and worth stating in the
  generated file's header so nobody assumes more.

## 6. Documentation to update with the code

| Document | Change |
|---|---|
| `glue-ffi.md` | "Generated protocol half" section: what is generated, what stays hand-written, the `--check` rule |
| `macro-system.md` | note that the generator is Rust *for now* and why; the stage rule already covers the generated text if it later moves to Briev |
| `docs/plans/2026-11-16-ffi-contract-and-metaprogramming.md` | S3 status + the parser finding resolution |
| `docs/plans/INDEX.md` | S3 progress line |
| `data-briev.md` / `config-ledger.md` | if S3.0 changes a parser rule: the nesting/separator rule as it actually is |

## 7. Open decisions

1. **Parser fix vs encoding workaround.** If §2.1 turns out to be a genuine
   syntax limit (not a bug), do we flatten further (one class per key:
   `value_String_class: …`) or fix the parser? Recommendation: fix the parser —
   flat-per-key prefixes would make the descriptor unreadable and every future
   nesting needs the same dodge.
2. **`types_module`.** Generate it too (`glue/c/types.bv` from `values:`), or
   delete the key? Recommendation: generate it in S3.3 — the export path already
   expects it, and it is the same data.
3. **`--check` in CI now or later.** Recommendation: add it in S3.2 and wire it
   into CI in the same commit that first commits a generated file, so drift cannot
   start.
