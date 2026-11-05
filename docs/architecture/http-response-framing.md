# Incremental HTTP response framing

`std/net/http_response.bv` frames one final GET response in caller-owned
`ByteBuf` storage. `http_response_frame(header_limit, body_limit)` creates reusable
state; `http_response_reset` prepares it for another connection without freeing
the buffer. The caller appends initialized bytes and passes EOF explicitly.

`http_response_progress` returns 0 (incomplete), 1 (exactly complete), or -1
(invalid; sticky until reset). A complete frame exposes `status`, `header_bytes`
and `body_bytes`. Header scanning resumes with a three-byte delimiter overlap.
The caller must retain the same bytes until reset and own/free the raw buffer.

Supported profile: HTTP/1.0 and HTTP/1.1, final status 200–599, strict CRLF,
case-insensitive header names, one nonnegative decimal Content-Length. Whitespace
is accepted around the length but not within it. Limits are checked before
arithmetic or body consumption. Duplicate lengths, transfer encoding, malformed
headers, over-limit and truncated frames are rejected. Extra bytes already in
the buffer are rejected. A later packet is outside this completed-frame check.
For 204 and 304 the supported profile permits no body and absent/zero length.

This is deliberately not a general browser decoder: no interim responses,
chunked framing, EOF-delimited bodies, HEAD, CONNECT, upgrades, redirects or TLS.
Unsupported wire profiles fail rather than generating misleading measurements.

The native `http_response_gate` fixture exercises bytewise fragmentation, exact
limits, EOF, malformed headers, reset and sticky errors at O0/O3. Extend the
tests and this contract together when adding a wire profile.
