# Owners census 12 → 0 and the tail-loop exclusion's fate (2026-11-16)

Plan: `docs/plans/2026-11-16-memory-and-xpu-finish-grind.md` §M3/§M4/§M5.
Companion: `2026-11-16-kiosk-soak-truth.md` (the leak mechanism + soak method).

## 1. Result

| metric | before (baseline) | after |
|---|---|---|
| releases inserted | 65 (59 baseline file) | **73** |
| undecided owners | 12 | **0** |
| conservative drops | 2200 | **2196** |
| static-backed locals | 9 | 9 |

`brievc memcheck --deny-memory-ambiguity briev/service/daemon.bv` → **exit 0**.
The flag gates on undecided only (`src/main.rs:1972`), never on conservative
drops, so zero is the exact gate.

Releases went UP by 8 while undecided went to zero: the newly-provable
families are real frees, not relabelling. Drops went DOWN by 4 for the same
reason.

## 2. Which families discharged, and by what

### 2.1 The protobuf parser chain (4 families + 2 `packed_ints`)

`parse_dimension_loop` → `parse_shape_loop` → `parse_tensor_type_loop` →
`parse_type_loop` → `parse_value_name_loop`, plus `packed_ints_loop`, all have
the shape:

```
defn parse_dimension_loop(r: ProtoReader, dims: List<Int>) -> List<Int> {
    when !proto_done(r) {
        ...
        when field == 1 {
            let next: List<Int> = dims;
            next <- proto_varint(r);
            term parse_dimension_loop(r, next);     // self tail call
        };
        proto_skip_kind(r, wire);
        term parse_dimension_loop(r, dims);
    };
    term dims;                                       // base case: a PARAMETER
};
```

These functions never allocate: the result is the caller's `dims` (push grows
the same collection object). The pass could not say so, so each `next` local
sat in the census.

Three analysis fixes discharged them (`src/hir/owned_storage.rs`):

1. `param_return_summaries` now resolves through an **alias map**
   (`param_alias_map`): parameters plus `let` copies plus push-mutated
   collections. `<-` is identity-preserving because it grows the same
   collection object, so `next` still denotes `dims`.
2. **Self tail calls are neutral** for the identity question instead of
   disqualifying the summary. A tail-recursive handler returns whatever its
   base case returns; the recursive arm is validated separately (it must
   thread the same parameter at the same position).
3. The summary is resolved to a **fixpoint across callables**
   (`param_return_round` iterated): `let next = parse_dimension_loop(dr, dims)`
   only becomes an alias of `dims` once `parse_dimension_loop`'s own summary
   exists, so the chain settles one link per round.
4. `call_aliases_caller_param` then classifies such a call as a **Borrow**
   (caller storage) rather than an undecided owner.

The narrowness is deliberate: only a *parameter-origin* argument qualifies. A
call handing back an unresolved local still reports undecided — a leak must
never be relabelled as a borrow to make a census look better.

### 2.2 The attr helpers (2 families) — multi-candidate returns

```
defn ints_or(items: List<Int>, fallback: List<Int>) -> List<Int> {
    when items.Count#() == 0 { term fallback; };
    term items;
};
```

The result may be *either* parameter. `param_return` is therefore a SET of
candidate parameters, and freshness requires **every** candidate argument to
be caller-owned. Collapsing to one index could only choose between a leak and
a double free.

`conv_attr4` (`[d0,d0,d0,d0]` fallback, both candidates owned) becomes fresh;
`conv2d`/`maxpool` now free their pads arrays. **+7 releases** from this one
fix.

### 2.3 Source copy discipline (the rest)

| site | change |
|---|---|
| `lib/std/cli.bv` `arg` | `term fresh_string("")` — a bare `""` is static storage the caller may not free, which forced every argv read to leak |
| kiosk `select_path` | `fresh_string(DEFAULT_SOCKET)` for the const path |
| kiosk `dispatch_method`, `dispatch_value`, `dispatch` | `owned_literal(...)` for literal arms |
| kiosk `sentiment_json` | three literal arms → `owned_literal(...)` |
| kiosk `method_name` | copies inline instead of through a local (see §4) |

Each is the same rule: **every return path of a function must have the same
ownership contract as its siblings.** One static literal in a function that
otherwise allocates makes the whole function unfreeable.

## 3. Soak at the new state (900 calls, two runs, one daemon)

Same geometry as `2026-11-16-kiosk-soak-truth.md` (640×640×3 f32, 4 915 200 B).

```
run A (300 calls)           run B (600 more, same daemon)
1   149.0                   1   158.3
50  145.7                   100 167.7
100 153.6                   200 161.6
150 148.8                   300 165.3
200 149.8                   400 146.6
250 154.4
300 158.3
```

The curve oscillates in a ~145–168 MB band and **drops 19 MB** at call 700 of
the combined run. A leak cannot go down; this is the allocator releasing
arenas. No per-call trend.

Plateau comparison: the previous pin sat at 183 MB after staging; this one at
~149–158 MB. The difference is the eight families above — the response,
dispatch and sentiment strings are now freed instead of accumulating.

## 4. Known gap recorded (not papered over)

`json_get`/`resolve_str_val` return `Ok(payload)` where the payload is a
*pattern binding* of the argument (a JSON enum projection), not a parameter.
`borrow_result_summaries` only recognises bare-parameter returns, so a
`Result` whose Ok payload borrows caller storage is invisible to the pass.
`method_name` therefore copies inline (`fresh_string(unwrap(method))`) instead
of binding a local; the local would sit in the census forever while never
being freed.

Teaching `borrow_result_summaries` about `Ok(payload)` where the payload is a
`Match`-derived projection is the durable fix. Recorded here so the copy does
not silently outlive its excuse.

## 5. Enforcement now in CI (kiosk)

- `brievc memcheck --deny-memory-ambiguity briev/service/daemon.bv`
- `harness/memcheck_ratchet.sh`, three counters, three directions:

| counter | rule | why |
|---|---|---|
| undecided | `<= baseline` (0) | ambiguity may not reappear |
| releases | `>= baseline` (73) | a dropped free is an ownership regression |
| drops | `<= baseline` (2196) | **new**: an owner may not be converted into a silent permanent leak |

Both new failure modes were probe-tested: raising the releases baseline
reported "releases decreased", lowering the drops baseline reported
"conservative drops grew".

## 6. Gates run on this tree

`cargo test --lib` 3783 passed (incl. 3 new owned-storage tests); kiosk native
smoke (MATCH 7 steps, face rpc raw=0/jpeg=2), ML smoke, VAD RPC 187 chunks, VAD
sessions 187 chunks over two feeds, dashboard IR verify 624 calls, census
ratchet ok.

## 7. Merge with main (0b58ff45f) — verification, and an RSS finding

`origin/main` (96 commits: verifier/SMT, resource-effects narrowing of
"observable", thread lanes, correctness gates, PR #101) merged in. Two
conflicts, both mechanical: `BUGS.md` (both sides prepended; kept both) and
`src/hir/owned_storage.rs` (16 hunks — main's only change to that file is
`0712ce724 chore(fmt): format 435 files`, verified by comparing with whitespace
stripped, where the two differ only by trailing commas; took this branch's side
then re-ran rustfmt).

**Merged tree:** `cargo build --release` clean; `cargo test --lib` **3849
passed, 0 failed** (main added 61 tests). Kiosk gates on the merged compiler:
native smoke MATCH ×3 with `face rpc: raw=0, jpeg=2`, ML smoke, VAD RPC, VAD
sessions, dashboard IR verify (625 calls). **Census unchanged: 73 releases,
0 undecided, 2196 drops** — `--deny-memory-ambiguity` exits 0 and the ratchet is
green. So main's narrowing of "observable" did not move any daemon family.

### 7.1 RSS step-growth is pre-existing (NOT a merge regression)

A 400-call soak on the merged daemon showed the RSS climbing 169.1 → 197.4 MB
(~94 KB/call over the later stretch). Because the merged IR differs from the
pre-merge IR (main's codegen: `add nsw`, new `__print_float64` declare), the
earlier 900-call evidence could not be assumed to carry over, so the same
cadence was run on two more builds:

| build | commit | 400-call shape |
|---|---|---|
| oldest | `4aed72809` (before the daemon census edits) | 183.0 flat to 250, then +20.8 MB by 350, flat |
| pre-merge | `751c66932` | 164.4 flat to 150, then +25 MB by 400 |
| merged | `0b58ff45f` | 153.9 → +33 MB by 400 |

All three start near 150 MB and finish 190–204 MB, in **steps with flat
stretches** rather than a straight line, and the step positions differ per
build. Since the oldest build (which predates every change in this campaign)
shows the same class of movement, the growth is **not** attributable to the
merge or to the census work. It matches what the campaign already recorded on
2026-11-11: "residual growth is small per-request heap objects … a full plateau
is still open".

**Open item (not a merge blocker):** decide whether this is glibc arena
retention or a true per-request leak, by (a) sampling after `malloc_trim`, or
(b) counting malloc/free in `coll_arena.c` and diffing per call. Until then, do
not read a plateau claim from a 400-call window — the steps are larger than the
average slope.
