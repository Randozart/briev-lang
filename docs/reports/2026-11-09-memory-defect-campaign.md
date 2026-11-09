# Memory-defect campaign — results

## Environment

- Revision: `test/defect-discovery-harnesses` = `origin/main` (`114b2a54`) + the
  test/docs commits. **No `src/` change** on this branch, so every finding below
  is inherited from `origin/main`.
- x86_64 Linux, clang 21.1.8, Valgrind 3.26.0.
- Harness: `tests/memory_campaign.rs`.

## Coverage run

- Corpus sweep: `reclamation_aliases`, `reclamation_bounded`,
  `structured_lifetimes`, `tuple_values`, `file_descriptor_lifetime` under
  ASan-`O0`/`O1` — clean.
- Generated families 0–4, seeds 1/7/2026, 4–6 cases each, ASan-`O0`/`O1` +
  Valgrind — families 0/2/3/4 clean; **family 1 fails**.
- Duration: a full family×seed sweep with Valgrind is on the order of minutes;
  the default `cargo test --all` smoke is bounded to family 0.

## Confirmed defect: publish-into-incoming-parameter leaks

```
Valgrind: 16 bytes in 1 block definitely lost   (exit 91)
allocated in make, reached through publish
```

Minimal repro `tests/fixtures/memory/publish_into_parameter_leak.bv`:

```briev
defn make() -> Ptr<Bit<8>> { let p: Ptr<Bit<8>> = Malloc#(16); Store#(p,1,8); term p; };
defn publish(h: Holder) -> Bool { h.addr = make(); term true; };
defn exercise() -> Int {
    let h: Holder = Holder { addr: 0 as Ptr<Bit<8>> };
    publish(h);
    term Load#(h.addr, 8);
};
```

**Isolation (the control that proves it is the compiler, not the test):**

| Program | Valgrind |
|---|---|
| `h.addr = Malloc#(16)` **directly in the caller** | clean (exit 0) |
| `h.addr = make()` **inside a callee** (`publish`) | **16 bytes definitely lost** |

Root cause: `src/analysis/reclamation/walk.rs:354` `record_incoming_store`
installs a barrier that disables region reclamation when storage is published
into a projection of an incoming receiver; `propagate_incoming_store` (`:778`)
propagates it to callers. The rationale comment (`:786`) says the
over-approximation is "safe" because it "only disables region reclamation" —
but disabling reclamation for heap storage means never freeing it. This barrier
was added 2026-11-08 to stop reclamation from freeing storage a caller still
reads (a use-after-free); it converted that UAF into a leak.

**It is independently confirmed by a standing guard:** running
`tests/reclamation_runtime.rs::native_aliases_borrowed_payloads_and_nested_returns_are_memory_safe`
(the near-identical `retain_in_argument` shape) fails under Valgrind on this
tree. Main's CI is green only because the Default-gates job does not install
Valgrind. Logged in `BUGS.md` (OPEN 2026-11-09).

## Methodology defect found and fixed

The first version of the harness linked with `clang -fsanitize=address` on the
emitted `.ll`. **That does not instrument memory accesses at all** — clang only
links the runtime for `.ll` input. Verified: a one-load `.ll` compiled with ASan
had zero shadow checks. Consequently the harness caught only `free`-family
errors and *silently missed use-after-free reads and overflows* — including a
generator bug (an 8-byte write into a `size`-byte allocation) that went
unreported.

`opt -passes=asan` is the IR instrumenter, but on the emitted Briev module it
registered globals and set a `nosanitize_address` module flag without adding
access checks. Also, LSan did not report the confirmed leak here, and there is
no `ubsan` `opt` pass. **Resolution:** Valgrind is the primary oracle (access
checks and leak detection with no IR instrumentation); ASan is kept only for
interceptor-based `free`-family errors. A planted use-after-free
(`MEM_DEMO_FAULT=1`) is now caught (Valgrind `Invalid read`, exit 91), proving
the corrected detector works.

## Reproduction

```bash
# The confirmed leak (expect a Valgrind definite-leak failure)
MEM_FAMILY=1 MEM_CASES=4 MEM_SEED=1 MEM_ARTIFACTS=target/memory-f1 \
  cargo test --test memory_campaign generated_memory_programs_are_memory_safe -- --nocapture

# The detector self-test (expects a failure only because the fault is planted)
MEM_DEMO_FAULT=1 MEM_FAMILY=0 MEM_CASES=1 \
  cargo test --test memory_campaign generated_memory_programs_are_memory_safe -- --nocapture
```

## Honesty

- One confirmed defect, found by the campaign and corroborated by an existing
  guard test. Families 0/2/3/4 were clean at this budget; that is not a proof of
  absence.
- No performance claim. Valgrind is ~10–30× slower; the campaign trades speed
  for the only oracle that actually saw the bug.
- The generator itself had a bug (allocation smaller than the store); it was
  fixed, and would have been invisible under the original ASan-only setup.
