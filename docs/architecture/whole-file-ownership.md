# Whole-file descriptor ownership

`read_file(path)` owns each descriptor it successfully opens. It closes that
descriptor after the reading helper returns, on EOF and read-error paths alike.
Open failures own no descriptor. The caller owns the returned String/Result,
not a file handle; repeated calls must not accumulate open descriptors.

The result continues to describe the open/read operation. A close result does
not overwrite a successful read or the original read errno. On Linux a failed
close must not be retried blindly because the descriptor number may be reused.

The native regression compares the next available descriptor across 640 reads:
normal proc data, empty input, growth beyond two buffer boundaries, EISDIR after
a successful open and ENOTDIR during open. It also checks content lengths and
preservation of errno. This is a resource correctness gate, not a timing test.
