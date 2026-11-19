# Stack-safe recursive reference evaluation

## Evidence and mechanism

The baseline compiler (`efd99d7b`, debug build) aborts with `thread 'main' has
overflowed its stack` when a test block parses, stringifies, reparses and
stringifies a small nested JSON object. A larger shell stack is a finite
workaround, not an interpreter recursion policy.

Guard recursive expression evaluation with `stacker::maybe_grow`. Keep the
small guard wrapper separate from the large expression-dispatch frame, so the
stack-space check happens before that frame is allocated. This preserves the
existing evaluation order, scope and return/error semantics for all expression
forms and handles non-tail as well as tail recursion.

## Verification and documentation

Add a low-stack thread regression for recursive/mutually recursive evaluation,
including a runtime error after growth, and a subprocess regression for the
real JSON round trip. Demonstrate baseline failure before the implementation,
then run focused, library and quick gates and Praetor. Record actual outcomes;
do not claim bounded total memory or parser-stack coverage.

Update `BUGS.md` and `docs/architecture/overview.md`; preserve expression
dispatch comments. The stack-switching implementation belongs to the maintained
`stacker` dependency; this change adds no local unsafe stack manipulation.

## Hypothesis refuted — experiment retired

The same input also overflowed a 64 MiB stack. A baseline GDB backtrace showed
repeated `call_user_fn("int_to_str")`: its named `d: Data` reached `d as String`
as an Int address, recursively invoking the numeric formatting lane. The
stack-growth experiment was cancelled and its dependency/wrapper removed.
The correction is recorded in `2026-11-19-interpreter-data-casts.md`.
