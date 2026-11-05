# Structured-value lifetime closure

The HTTP catalogue probe validates correctly after the statement-match fix,
but Valgrind reports 16,683 lost bytes for six tiny JSON checks. Reclamation
diagnostics identify unclassified slices, indexed casts and intrinsic method
calls as barriers. These are language constructs, not JSON-specific effects.

Model a slice conservatively as a fresh allocation plus backing-storage alias;
resolve indexed expression types through the existing type checker; route pure
intrinsic methods through the existing intrinsic effect registry. Preserve all
unknown-call barriers. Validate local consumers and escaping slices, plus the
actual JSON catalogue probe under Valgrind. Extend only where further concrete
diagnostics identify a missing proof. Record any remaining barriers explicitly.

Update lifetime architecture comments and BUGS. Final library/build/Praetor
checks follow focused application validation per WORKFLOW. No timing or
optimization claim is made.

Additional discriminators found during integration: plain `let` bindings were
also loaded as if they destructured their initializer, making a list alias its
elements. Literal numeric constants carried the address of their global storage
as false provenance. Correcting these independent facts removed the remaining
528 fixed startup bytes and all socket-pool escape barriers. Unknown/custom
indexed operations remain barriers even with richer type information.

The existing contiguous-slice backend check now accepts the region allocator as
well as malloc: the view still allocates only its header and points into the
original column. The ownership pass changes allocation lifetime, not view shape.
