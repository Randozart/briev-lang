# Discarded pattern bindings

Variant selection and binding materialization are separate operations. A
variant whose payload patterns are all wildcards needs the tag test but no
payload read, owned copy or binding. This includes a zero-payload variant.
`LlvmBackend::bind_pattern` returns without materializing such payloads;
named bindings retain their existing ownership behavior.

This rule prevents unreachable owned copies in tag-only predicates. It does
not establish reclamation for named bindings or locally returned objects.
Broader returned-value reclamation requires separate ownership proofs.

The native `enum_wildcard_allocations` regression counts allocator calls and
checks tag decisions at O0 and O3. It covers a single inline object payload,
multiple discarded payloads and a zero-payload variant. Retire the dedicated
lowering arm when shared demand-driven pattern lowering preserves these
observable contracts.
