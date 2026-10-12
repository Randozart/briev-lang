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
