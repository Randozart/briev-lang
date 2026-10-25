# Correct ordinary collection assignment

Task-12 native integration exposed same-type local assignments dispatched to
InsertAt instead of replacing their binding. Minimal repro:

```briev
let values: List<Int> = [];
foreach idx in 0..2 {
    let entry: List<Int> = [idx+1];
    values = values + entry;
};
```

Before: length 2, elements are heap handles. After: 1,2. LLVM emission in
emit_stmt.rs must distinguish whole-value assignment from explicit insertion.
The additive collection_assignment.rs seam derives eligibility from declared
operator metadata and exact type equality. Existing state/member storage takes
precedence; arrow insertion and legacy different-type fallback are untouched.

Native regression includes integer concat, object replacement and subsequent
explicit arrow insertion. Unit tests cover existing/new slots, unknown bindings
and different RHS types. Architecture commentary is updated without removing
historical rationale. No performance claim or optimization-path replacement.

The fix is prepared on fix/collection-assignment-rebind from main, with matching
edits on the active integration branch for the server gates. No commits yet:
WORKFLOW requires review and commit approval. Build passes; three unit tests and
the native regression pass. Full library/determinism/Praetor results are recorded
with the task-12 verification once complete.
