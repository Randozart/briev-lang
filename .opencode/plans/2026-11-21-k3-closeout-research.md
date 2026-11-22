# K3 closeout — research notes: what is actually true, and what is left

Status: RESEARCH (no code changed, no state-mutating command run).
Scope: is the K3 memory campaign finished, and is the trunk `BUGS.md` entry it
produced correct?

Short answer, in two parts:

1. **K3 R2 is implemented, measured, committed and on `origin/main`.** The
   measurements are good and the mechanism is sound (§2).
2. **Two records are wrong and one soak did not finish.** The "wave-8 contract
   regression" is not a regression — it is the contract gate finally being
   installed on the CLI path, and pre-merge Briev accepted *refuted* programs
   (§3). And the 600-call R3 soak died at ~500 calls; the plan doc quotes that
   run's staircase but not its termination (§4).

---

## 1. Verified end-state (read-only checks)

| Thing | Value | How verified |
|---|---|---|
| `origin/main` / `origin/intel-xpu-harvest` / lane `HEAD` | `0698324e7` | `git rev-parse` in `briev-intel-xpu` |
| K3 R2 commit on main | `1f9f3c823` (ancestor of `0698324e7`) | `git merge-base --is-ancestor` |
| Docs commits on main | `0d3d23ac8` (D39), `82bdd11f9`, `0698324e7` (RSS staircase) | `git log --oneline -8` |
| Kiosk pin | `compiler.lock` = `4caace291`, results doc `88233f5` | `git log` + file read |
| R2 on the pin's lineage | **not** (pin predates R2) | ancestry check |

`briev-lang`'s local `main` shows `7579dfd6c` only because that worktree's refs
were read before the last push; the remote ref is the authority and says
`0698324e7`.

## 2. What R2 actually bought (unchanged, re-derived on one geometry)

30 `detect_faces` calls, identical 5 MB zero frame, identical builder:

| build | leaked bytes | leaked allocations | per-call |
|---|---|---|---|
| baseline (`4caace291` + Phase C, `exit(0)` patched) | 2,627,912 | 37,451 | 58.0 KB |
| R2 (`1f9f3c823`, same patch) | 2,071,591 | 30,131 | 39.5 KB |

−21.2 % bytes / −19.5 % allocations over 30 calls; −32 % in per-call terms.
Direct-leak attribution on the same geometry (indirect children included in the
totals above; see §5 for the bucket list):

| allocating function | baseline B / n | R2 B / n |
|---|---|---|
| `conv2d` | 74,208 / 3,092 | **0 / 0** |
| `transpose4` | 8,424 / 351 | **0 / 0** |
| `maxpool` | 2,760 / 115 | **0 / 0** |
| `tensor_alloc` | 100,640 / 2,471 | 99,392 / 2,437 |
| `conv_attr4` | 40,104 / 1,671 | 39,264 / 1,636 |

The three families that vanished are exactly the ones whose owned temporaries
were handed to borrowing parameters — the R2 mechanism, visible in the
allocation profile rather than inferred from it.

## 3. Correction: the "wave-8 contract-join regression" is a misdiagnosis

`BUGS.md:1` states that trunk rejects the kiosk daemon with 41 contract
refutations because wave-8 *lost* a conditional-reassignment fact join, and
that `4caace291` accepts the reducer. Both halves are wrong.

### 3.1 What the compiler actually does

Minimal pair, both compilers, same sources, same `config` (verified identical:
`config/contracts.dbvl` = `policy: deny;` in both trees):

```briev
defn f(x: Int) [x > 10][true] -> Int { term x; };
defn use() -> Int { term f(1); };
```

| compiler | verdict |
|---|---|
| `4caace291` (pre-merge) | **exit 0, binary emitted** — a provable violation of `[x > 10]` is accepted |
| `3bf2fc375` / `0698324e7` (main) | `invalid operation 'call to 'f' — the contract is REFUTED at this call: '(1 > 10)''` |

So the pre-merge compiler does not enforce the call-site contract gate *at
all*. The recorded reproducer never distinguished a fact *join*: every variant
of it contains `chan(n)` with `[i >= 0]` and an unconstrained `n`, so it is
unprovable on its own merits. It "passed" before because nothing was checked.

### 3.2 Why the gate appeared

* `config/contracts.dbvl:20` declares `policy: deny;` (flipped 2026-10-20).
* `f8c36f21a feat(contracts): enforce the call-site proof gate (deny)` is
  already an ancestor of `4caace291` — the checker code existed.
* What was missing is the **installation**: `install_contract_policy` in
  `src/main.rs` + `install_contract_gate_from_config` in `src/pipeline.rs:1028`,
  added by `3db43e4c2` ("Enforcing `deny` on the CLI reached further than the
  tier1 sample…"), which is **not** an ancestor of `4caace291`.
* `src/typechecker/mod.rs:2203-2212` is the gate; on `Verdict::Refuted` it names the
  counterexample, otherwise it names the missing proof.

That commit is itself the staged-tightening pattern done correctly: it ships
with a `warn` stage, an explicit exemption row (`exempt:` roots, announced on
stderr), and a test that no acceptance root can be exempted
(`exemptions_name_real_directories_and_never_acceptance_roots`). Compare
rustc's lint levels, which move from `allow` to `warn` to `deny` across
releases rather than flipping a whole language at once
(<https://doc.rust-lang.org/rustc/lints/levels.html>).

### 3.3 Consequence for the 41 sites

They are real obligations, not a defect. Grouping from the merged-tree build
(`/scratch/k3-asan/build-merged.log`, 41 refutations):

| class | sites | example |
|---|---|---|
| loop induction over a tensor extent | most | `((i >= 0) && (i < result.n))` from `foreach i in 0..result.n` |
| projection through `unwrap` | 4 | `((0 >= 0) && (0 < unwrap(pr).n))`, `unwrap(bias).n` |
| module constant comparison | 2 | `(((SP_SLOT_VAD >= 0) && (SP_SLOT_VAD <= 1)) && …)` |
| genuinely unguarded index | rest | `(base >= 0) && (base < t.n)` with `base` computed |

The first three are the *same shape* as the two prover gaps `3db43e4c2` fixed
for tier1 ("the source already decided this and the prover did not read it"): a
fact that exists syntactically but is not derived. They belong in the prover as
general derivations (extent facts from `foreach`, `unwrap` field projection,
constant folding through `dbv` rows), not in 41 daemon guards. The fourth class
is where the daemon must become honest — its own contract has to say what the
index means.

Repair path (own plan, not part of K3): census the 41 sites into those four
classes, derive the three general facts, then guard or contract the remainder.
The wrong move is what `compiler.lock:5-13` currently says — "loads once the
checker regression is fixed". There is no such regression to fix; waiting is
unbounded.

### 3.4 Records that must be corrected (GRIND step)

* `BUGS.md:1-54` — retract the regression claim; keep the 41-site census as a
  *kiosk honesty* item, with the `f(1)` vs `[x > 10]` evidence (there is
  precedent in this file for retraction: the `#144` misdiagnosis).
* `briev-kiosk-avatar/compiler.lock:5-13` — the pin note's reason.
* `docs/results/2026-11-21-k3-r2-argument-temporaries.md` — same correction.

## 4. The 600-call soak did not finish

`/scratch/k3-asan/soak-r2h/` (R2 daemon, `frame_path`, sample every 50):

```
calls  vmrss_mb  dRssAnon
50     147.8     +5.1
150    153.4     +10.7
200    153.4     +10.7      <- flat
250    158.9     +16.2
300    159.0     +16.3      <- flat
350    151.5     +8.8       <- steps DOWN
400    151.5     +8.8
450    154.4     +11.7
500    154.4     +11.8      <- flat
```

Then at ~call 501–550: `rpc.RpcError: connection closed mid-frame`. The daemon
wrote **nothing** to `a.daemon.log`, produced no core (`coredumpctl` has no
entry), and the wrapper's own `kill` ran only after `soak.py` had already
failed. The plan doc quotes this run's staircase (142.7 → 159.0 → 151.5 →
154.4) but not the fact that it stopped at 500 instead of 600.

What the curve still supports: no slope through 500 calls. Steps go down as
well as up, and `MALLOC_ARENA_MAX=1` does not remove them (recorded), so the
staircase is not arena retention — it is the instrument. The ASan two-point
slope stays the primary evidence.

Hypotheses for the termination, ranked, with the test that settles each:

| # | hypothesis | discriminating test | reading |
|---|---|---|---|
| H1 | pre-existing, not R2 | same 600-call arm against `build-fixed12/daemon-fixed12-exit` | dies at the same count ⇒ pre-existing |
| H2 | per-connection resource bound | repeat on one process with 600 *separate connections* | survives ⇒ per-connection state |
| H3 | 5 s recv timeout (`briev/service/daemon.bv:734` `sock_set_recv_timeout(conn, 5)`) | log the daemon-side `read_len` return; timestamp client sends | timeout ⇒ the last request was starved >5 s |
| H4 | stack depth from the recursive request loop | `ulimit -s 65536` for one run | survives ⇒ depth |
| H5 | arena/capacity growth (`sp_open_coll(67108864)`, `lib/saltpetre/session.bv:38`) | print `coll_arena_off` at each reset | climbing offset ⇒ unrewound arena |

H4 is **already falsified by measurement**: `handle_conn` is self-recursive in
the linked binary (`objdump` → `<handle_conn> -> <handle_conn>`, LLVM keeps the
call because it is followed by `sp_request_reset()`), but its frame is
**96 bytes** (`push` ×6 + `sub $0x28,%rsp` + return address), so 8 MB of stack
is ~87,000 requests — a 500-call death is 0.6 % of that. Stack depth is not the
cause, and it is also not worth "fixing" for this bug (though a self-call in a
defn tail position that LLVM declines to turn into a loop is a general lowering
gap worth its own record).

## 5. Open work (ordered by evidence strength)

1. **Finish the soak truthfully** (§4): run both builds, capture the daemon's
   exit status and signal with `wait`, and record the termination wherever it
   happens. A soak that stops at 500 is a result, not a footnote.
2. **R2b — String argument temporaries.** R2 hoists collection temporaries only.
   A String temp handed to a borrowing parameter still leaks: naming it
   round-trips through `let_binding_types`, and the ordered argument adapter
   emits `inttoptr` on an already-`ptr` register (invalid IR, observed at
   `owned_string(proto_bytes(r))` in `parse_attr_loop`). Binding the *declared*
   type and letting the read path answer `ptr` would let the hoist cover
   Strings — the target bucket is the largest one left:
   `briev_bits_to_str <- briev_str_substr` 229,546 B / 892 allocations per 30
   calls.
3. **R4 — `Result` boxes and the `conv_attr4` residue.** `tensor_take <-
   run_node` 100,573 B / 2,453 and `conv_attr4 <- run_node` 39,264 B / 1,636
   are shape-level allocations, ~1–3 KB/call.
4. **Re-run the ownership census** (`harness/memcheck_ratchet.sh`) to confirm
   releases ≥ 103, drops ≤ 2221, undecided 0 after the final R2 state.
5. **Kiosk honesty on main** (§3.3) — the 41 sites, own plan, blocked on
   nothing but attention.

## 6. Recommendation

The K3 memory campaign is done *as a milestone*: the mechanism is in main, the
measurements reproduce, and the biggest allocation family is at zero. Two
things should not be signed off as "finished" without correction:

* the `BUGS.md` / pin note story (§3) — it is factually wrong today and it
  blocks the kiosk on a fix that will never come;
* the incomplete soak (§4) — either finish it or say plainly that the arm
  terminated at 500.

---

## Sources

Local (all read-only):
`BUGS.md:1-54` · `config/contracts.dbvl:20` · `src/main.rs` `install_contract_policy`
(`git show 3db43e4c2 -- src/main.rs`) · `src/pipeline.rs:1028` ·
`src/typechecker/mod.rs:2203-2212` · `lib/saltpetre/session.bv:27-50` ·
`briev/service/daemon.bv:718-755` · `/scratch/k3-asan/build-merged.log` ·
`/scratch/k3-asan/soak-r2h/a.curve.txt` · `/scratch/k3-asan/asan.base30.log` ·
`/scratch/k3-asan/asan.r2h.log`.

External, single point: rustc lint levels as the reference model for a staged
`allow → warn → deny` tightening
(<https://doc.rust-lang.org/rustc/lints/levels.html>). The rest of this note is
a question about this repository's own records, so web research would add
noise rather than evidence.

---

## Amendment (2026-11-22, after the closeout measurements)

This note's §3 asserted the BUGS.md entry was wrong **simply**: that the 41
refutations were only the gate being newly installed, and the
"conditional-reassignment fact join" story was a misdiagnosis. Measurement
during GRIND refines that in one direction and refutes it in another.

**What still holds.** Finding A is confirmed and is the larger error in the
original entry: the A/B table compared a pre-gate compiler against a
post-gate one. `4caace291` accepts `f(1)` against `[x > 10]` — a provably
violated contract — so its "0 errors, binary emitted" arm never passed
anything. `git merge-base --is-ancestor 3db43e4c2 4caace291` → NO, re-verified.

**What does not.** The mechanism story was not a misdiagnosis. On the merged
compiler a direct `let c = chan(i)` discharges `get(8, c)` (0 refutations)
while `let c = 0; when n > 1 { c = chan(i); };` does not (1 refutation), for a
shape where both are provable (`0 ∈ [0,8)`). So there genuinely is a lost fact
at the conditional join — a prover incompleteness, independent of the gate.

**Corrected conclusion** (see `BUGS.md`'s corrected 2026-11-21 entry): the 41
sites split into provable-but-lost-fact sites (compiler repair: the
conditional-join fact handling) and genuinely unproven obligations (kiosk-side
guards/contracts). Both are real work; neither is a wave-8 regression to
revert. The earlier flat statement "the checker is fine, the consumer is
wrong" was too strong in the opposite direction and is superseded here.

**Soak amendment (§4).** The 600-call A/B was completed rather than reported
as an incomplete arm: `soak-closeout/{fixed12,r2h}` on the identical geometry
show both daemons ALIVE through 600/600 (`fixed12` 143.7 → 188.6 MB,
`r2h` 143.7 → 173.1 MB, both harness-killed, not self-terminated). The
`soak-r2h` death around call 500 was therefore a run-specific condition and
NOT a pre-existing daemon weakness (H1 falsified in that direction), nor is it
R2-related. R2's ~15 MB lower footprint persists across the full run, matching
the ASan slope.
