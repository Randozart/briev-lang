# Trait default effects in allocation regions

The live client memory probe exposes an unclassified call chain through stdlib
WebSocket/SHA-1 builders. Concrete methods are indexed, but trait defaults are
not: `sink.put_two(...)` and free-call sibling dispatch inside its body suspend
regions. Prove the general case with a small declared trait and two concrete
implementations, including an escaping implementation and name-shadow controls.

Register defaults as Self-generic bodies, matching existing frontend/backend
specialization identity. Resolve chained results and free-call concrete methods
without assuming purity. Required/implicit/unknown methods remain conservative.
Preserve concrete top-level and object-member precedence. Verify whole-call
escape facts, native memory behavior, final library suite and Praetor baseline.
Update ownership architecture and BUGS; preserve existing rationale comments.
