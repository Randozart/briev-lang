# Caller-owned enum result pointers

Small enum-returning functions can fill caller-owned `%enum_out` storage and
return its address as an integer handle. The output pointer is aligned and
separate from the function's other live inputs, but it is **captured by the
returned handle**. The parameter must not carry LLVM `nocapture`.

This is an ABI fact independent of the enum name, its payload or the caller.
Returning a pointer via `ptrtoint` still makes the address observable after the
call. LLVM may exploit a false non-capture promise when optimizing later reads.
The signature emitter is the single source of this parameter contract; call
slots and enum construction keep their existing representation.

The optimized process-unit probe exposed this defect: O0 matched independent
OS queries, while O3/LTO corrupted both fields. An actual-IR experiment removing
only `nocapture` from `%enum_out` restored the values; removing `noalias` or
relaxing memory effects did not. Keep the native OS-value regression alongside
the signature gate. Reintroduce non-capture only if a future ABI no longer
returns or otherwise exposes the destination address.

Reference: [LLVM 17 parameter attributes](https://releases.llvm.org/17.0.1/docs/LangRef.html#parameter-attributes).
