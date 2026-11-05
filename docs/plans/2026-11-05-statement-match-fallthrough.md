# Statement match fallthrough correctness

The catalogue validator rejects valid input because statement match lowering
uses only the final arm's termination flag for its merge. An earlier arm can
continue into an emitted `unreachable`. A partial match's no-match edge also
continues regardless of arm termination.

Reproduce with a number enum arm containing a guarded return followed by a
returning wildcard. Preserve the emitted condition-chain edge and leave its
merge open. LLVM can remove impossible edges. Add behavioral coverage for both
arm orders, guarded returns, all-returning arms and empty fallbacks; verify
native execution and IR. Update backend contracts and BUGS with the cause.
Run final library/build/Praetor/determinism gates after app verification.

