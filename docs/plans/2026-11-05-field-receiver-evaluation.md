# Evaluate field receivers once

The HTTP runner opened two sockets for `sock_tcp_nonblocking().fd`: the field
dispatcher evaluated its receiver, then the general field loader evaluated it
again. One descriptor leaked per request. Preserve the already-evaluated typed
register through the field loader, leaving all tuple, packed, event, array and
pointer paths intact. Audit both call sites.

Use a stateful factory counter as the minimal regression, checking direct and
nested projections, assignment RHS, and tuple projection. Verify native O0/O3
execution, generated call counts and descriptor closure in the actual client.
Update BUGS and backend contracts. Run final library/build/Praetor/determinism
checks after focused app verification, per WORKFLOW.
