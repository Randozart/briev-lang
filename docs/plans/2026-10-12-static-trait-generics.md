# Static trait-constrained generics (2026-10-12)

**Status:** ACTIVE. Branch: feat/trait-system-implementation (from main).
**Scope:** static (monomorphized) trait-constrained generics ONLY. dyn runtime
dispatch is explicitly DEFERRED (separate, backend-heavy, independently broken).

## Why

SPEC 8.6 specifies trait + trait-constrained generics (Static trait-constrained
generics are monomorphized by default). The stdlib net stack needs it: one
generic byte-sink writer (put_u16_be etc.) over both a fixed and a growing buffer
(design D of the bytes.bv work). Probes proved the feature is parse+AST only: a
trait method called on a bound type param returns Int instead of Self/T.

## Ground truth (reconnaissance 2026-10-12)

WORKS (parse/AST/call-site substitution):
- Parser: trait (parse_trait, definitions.rs:3167), impl (ImplDef), <T: Bound>
  (parse_type_params, types.rs:356 - TypeParam { name, bound: Option<Type> }).
- AST: TopLevel::Trait(TraitDef), TopLevel::Impl(ImplDef), Type::Dyn, Type::TypeVar.
  TraitDef { name, type_params, functions: Vec<Definition> } (top.rs:1169).
- Typechecker call-site generics: infer_call (mod.rs:1989) infers T from args via
  infer_defn_type_args and substitutes param/return via substitute_type_params.
  So bump_twice(t,5) DOES bind T=Tally at the call site.
- A dyn Trait receiver already resolves methods against trait requirements with a
  Self-aware signature match (resolve_method_call, mod.rs:6404) - strong precedent.

GAP (the whole bug):
- fn_type_params (mod.rs:82) stores only the NAMES of a defns type params, NOT
  their bounds. Inside the body the checker does not know T: Counter.
- resolve_method_call / infer_call have no branch for a receiver/arg whose type is
  a BOUND type parameter -> resolve against the bound traits functions with
  Self->T. So bump(this,amount) with this: T does not resolve; return falls to Int.
- No structural-conformance check that the concrete T actually provides the ops.

## Staged implementation (small, verified increments)

Each stage: implement, cargo test --lib stays green, a scratch probe advances,
explain to the human. The interpreter is the reference (AGENTS Rule 5): make the
interpreter resolve trait generics first, then the LLVM backend, then confirm the
same program runs identically.

S1 - Carry bounds into the typecheck context.
    Extend the defn/type-param registration so a type params bound trait is known
    while checking the body. Likely a map fn_type_param_bounds[name] -> Vec<(param,
    Option<trait>)> or enrich fn_type_params. No behavior change yet; just plumbing.
    Gate: existing tests green; a debug probe shows the bound is visible in-body.

S2 - Resolve a trait method called on a bound type param (the core fix).
    In resolve_method_call and infer_call: if the receiver/first-arg type is a type
    param T whose bound is trait C, look up the function in ctx.trait_defs[C],
    validate arg arity/types, and return the result type with Self->T substituted.
    Model it on the existing Type::Dyn branch (mod.rs:6404). Covers both the .m()
    UFCS form and the free-function form.
    Gate: probe P3 (bump_twice<T: Counter>) type-checks; return type is T not Int.

S3 - Default methods.
    A trait default (body-carrying function in TraitDef) is resolvable both inside a
    generic over T: C and as a call on a conforming concrete value. A concrete types
    own member overrides the default.
    Gate: probe P2 (bump_twice default) resolves on a conforming obj.

S4 - Structural conformance check at the call site.
    When T binds to a concrete type at a call to a T: C generic, verify that type
    structurally provides every required trait function (signature refinement, per
    SPEC 8.6). A missing/mismatched op is a helpful diagnostic (what/why/fix), not a
    later codegen panic. Explicit assertion (type Foo: C) documents intent.
    Gate: a non-conforming type at a T: C call is rejected with a clear message.

S5 - Interpreter execution.
    The reference interpreter runs a trait-generic program: the method call on T
    dispatches to the concrete bound types member. Monomorphization is implicit in
    the interpreter (it has the concrete value).
    Gate: probe runs in the interpreter, prints the expected value.

S6 - LLVM backend (monomorphized).
    Static trait generics are type-erased like existing generics (boxed i64 ABI,
    SPEC 9.1). A trait method call on T lowers to a call to the concrete types
    member (resolved per call-site instantiation). Verify no dyn/thunk path is
    needed. If codegen needs per-instantiation member resolution, add it.
    Gate: probe builds + runs natively, same output as the interpreter.

S7 - Tests + docs.
    Behavioral tests in the typechecker + a .bv example; update SPEC 8.6 if any
    detail is clarified; BUGS.md entry documenting the pre-fix gap + the fix.

## Non-goals (this branch)
- dyn runtime dispatch, fat pointers, thunk tables (deferred - separate campaign).
- Trait objects as values, trait inheritance, associated types, where-clauses
  beyond a single trait bound per param. Add only if a concrete need appears.

## Risks
- Self substitution must thread through nested calls (bump(bump(this))).
- A type param used in arg position (not just receiver) must also resolve its
  bound - infer_call needs the same treatment as resolve_method_call.
- Conformance over CONTRACTS (SPEC: refinement of pre/post) is advanced; v1 may
  check signatures only and note contract-refinement as a follow-up.
- Codegen: confirm the boxed-i64 erasure already routes member calls correctly;
  the dyn backend path is broken, so DO NOT depend on it.

## Implementation log (append-only)

2026-10-13 - S1+S2 DONE (commit 5034ed41). S1: fn_type_param_bounds (per-defn
registry) + active_type_param_bounds (transient, set while a defn body is
checked) + type_param_bounds() helper, threaded through CheckEnv /
make_typecheck_context. S2: resolve_bound_param_call() - an additive branch in
infer_call before the Int fallback. If a call argument's type is a bound type
param T:C and trait C declares the function, validate (Self->T) and return the
requirement's result (Self->T). Gate P3 passes (bump_twice<T: Counter> checks OK,
returns T). cargo test --lib: no regressions (the one non-GPU conformance failure
is a pre-existing malformed .dbvl fixture, BUGS.md).

2026-10-13 - S3 implemented. Three additive pieces, all reusing S2 machinery:
  (A) Trait DEFAULT bodies are now type-checked (they never were). A loop in
      check_program_with_target, modeled on the obj-member loop, checks each
      body-carrying trait function with self/params bound and
      active_type_param_bounds = {"Self": trait_name} - so a call to a sibling
      requirement inside the default (bump(this,amount), this: Self) resolves
      through resolve_bound_param_call with Self as the bound param.
  (B) Free-call form on a concrete conforming value (bump_twice(tally,5)): a
      sibling branch in infer_call after resolve_bound_param_call. First arg is
      the Self receiver; the rest match the trait fn's params after Self.
  (C) Method-call form (tally.bump_twice(5)): a branch in resolve_method_call's
      None-member arm, after the fn_return_types UFCS check and before the error.
  Shared: type_conforms_to_trait() (structural - the concrete type provides every
  REQUIRED (empty-body) trait fn; defaults are supplied by the trait) and
  resolve_conforming_trait_call() (finds a conforming trait declaring `name`,
  validates args Self->concrete, returns result Self->concrete). Trait scan is
  sorted for determinism. Gate: P2 (t.bump_twice(5)) and P2b (bump_twice(t,5)).
  Note: a concrete type's OWN member of the same name still wins - resolve_method_
  call finds it in type_members FIRST; the conformance branch only runs when the
  member is absent (the default-inheritance case).

2026-10-13 - S4 DONE. Call-site conformance check for trait-bounded generics.
  check_type_param_bounds() runs in infer_call's generic-defn branch right after
  infer_defn_type_args binds the type params: for each bounded param <T: C> it
  verifies the inferred concrete type conforms (type_conforms_to_trait), else a
  hard error naming the generic, the bound, the concrete type, the MISSING
  required op(s) (missing_trait_requirements), and the fix. Closes the soundness
  hole where a non-conforming concrete type silently returned a bogus type.
  Gate P4 (bump_twice(MyGadget, 5), MyGadget has no bump) is rejected:
  "'bump_twice' requires T: Counter, but 'MyGadget' does not provide bump. Add
  bump to 'MyGadget', or pass a type that conforms to Counter". P1/P2/P2b/P3
  unchanged. cargo test --lib: no regressions (same baseline failure set).

  KNOWN LIMITATION (follow-up): conformance is read from ctx.type_members (an
  obj/struct's OWN defn members) only. A type that provides a trait's methods
  through an `impl` block is NOT recognized as conforming by S3/S4 (it would be
  rejected). check_trait_assertion already merges impl functions via
  type_function_names; type_conforms_to_trait should do the same once a
  trait-bounded generic is called with an impl-conforming type. No test currently
  exercises this (the full suite passes), so it is deferred, not blocking.
