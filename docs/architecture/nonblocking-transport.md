# Linux nonblocking transport

`std/net/nonblocking.bv` exposes one-step sends and receives. Calls use
MSG_DONTWAIT and never retry internally; callers retain offsets after short
writes and reschedule EAGAIN/EINTR according to their deadline. Sends suppress
SIGPIPE and return negative errno. Receives append only bytes actually read;
positive requested capacity makes zero an unambiguous EOF result.

`sock_tcp_nonblocking` creates NONBLOCK/CLOEXEC descriptors atomically.
After an EINPROGRESS connect becomes ready, `sock_pending_error` reads and clears
SO_ERROR; readiness alone does not establish a successful connection.

`std/os/poll.bv` stores the Linux x86-64 eight-byte pollfd ABI. PollSet owns its
allocation but borrows descriptors. It accepts finite waits, clears stale events,
returns EINTR, rejects invalid bounds, and permits idempotent free through aliases
of the same object. Callers must drain readable bytes even when HANGUP is set.
Close descriptors separately; do not copy a PollSet's raw allocation address.

The modules use raw Linux x86-64 syscalls. They do not implement connection
deadlines, HTTP framing, DNS, TLS, or automatic retries. The native integration
test uses controlled local peers and an ephemeral reserved refusal port.
