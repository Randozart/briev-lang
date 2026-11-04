# Indexed collection writes

2026-11-03 native implementation record.

Indexed reads use the declared At operation. Indexed assignments use the
explicit SetAt(index, value) operation when supplied by the receiver type.
The coll declaration scaffolds both capabilities from its declared sequence
member. No List name or histogram-specific code is involved.

The scaffolded write traps for negative or past-end indices, then stores
through the derived sequence. Fixed collections use their declared bound;
growable collections use their live length. Assignment does not grow storage.

LLVM preserves the existing RHS evaluation order, evaluates the receiver once,
and binds those values temporarily while dispatching SetAt. Ordinary pointer
and array stores retain their existing paths. Tests must cover direct and
nested collection receivers, user-named coll types, scalar and object elements,
bounds, RHS effects and O0/O2 native behavior. Type checking rejects declared collections
without SetAt and validates its argument types and arity. Shape-bearing
storage, indexed pin declarations and generic storage with unresolved element
types are checked through their existing indexing rules. The element must
be a type parameter of the enclosing callable for the parametric case. Ordinary records,
scalars and const pointers do not acquire a writable index. Library mode
registers declared types before checking, as the build and check pipelines do,
so shape metadata is available through the same universe query. Interpreter parity
remains required before declaring this feature complete across execution engines.

Evidence: histogram_record previously emitted the increment but no store;
all bins remained zero. The regression fixture is
 tests/fixtures/load/collection_index_store_gate.bv.

Array element storage is distinct from the callable ABI. Static record elements
occupy their complete inline images even when a function passes that type using
an address. Pointer array elements retain the existing word-sized handle ABI;
floating elements retain their resolved native width. Construction, member
reads/writes and row-view reads/writes must agree on this representation. The
fixed collection gate includes records, pointers, enum values and omitted zero-initialized
elements; the corresponding library tests include narrow scalar/handle boxing.

Inline members have a dedicated local exit continuation, independent of the
enclosing transaction's abort target. Dynamic dispatch phi inputs refer to the
actual ending block of an arm, including any inline-member continuation. A
bare member return must not return from, or run deferred cleanup belonging to,
the caller.

Enum classification uses the centralized destination-ownership query; it does
not depend on the retired enum-handle cache. Enum carriers remain distinct
from inline static-record images.

Malloc pointer images and tracked remaining lengths use target width. The
allocator call alone widens its size argument to the allocation ABI. Inline
generic member bindings preserve the operand representation when the element
parameter remains abstract, avoiding conversions to an unresolved fallback.
