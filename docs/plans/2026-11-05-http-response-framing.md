# Bounded HTTP response framing

Add a reusable incremental HTTP/1.0 and HTTP/1.1 GET response framer for the
recommender load client. Preserve a strict, explicit Content-Length profile:
reject transfer encoding, duplicate lengths, malformed headers, oversized and
truncated messages. Scan newly appended bytes with a three-byte overlap.
Reuse state and caller-owned storage between attempts.

Validate every-byte fragmentation, complete buffers, EOF, limits, reset and
sticky failures using a native fixture and Valgrind. Integrate the fixture with
the existing native test infrastructure. Application verification precedes final
compiler library tests, per WORKFLOW. No compiler optimization is proposed.

Document the supported wire profile, ownership and result contract in the
stdlib module and architecture documentation. Preserve the dated rationale when
extending support. This feature branch contains only framing; transport and
collector prerequisites remain in their separate PRs.
