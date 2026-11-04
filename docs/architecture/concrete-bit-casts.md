# Concrete bit casts

Concrete `Bits<N>` values use an LLVM integer of width N. Casting a pointer
to this representation must emit `ptrtoint`; an unchanged pointer register
cannot be labelled as an integer. Integer inputs use truncation when narrowing
and zero extension when widening because this operation preserves raw bits.

The encoding constructor must convert the actual integer register width to a
pointer. Registered user constructors retain their canonical i64 storage-word
argument; adapt a narrower or wider raw input at that boundary.

The browser fixture `bit_string_roundtrip_wasm.rbv` covers pointer encoding,
Data conversion and widening, and `browser_cast_tuple.mjs` executes it together
with the existing tuple-return fixture. No tuple ABI change is involved.
