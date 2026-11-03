# Guarded tail returns

LLVM statement emission tracks whether a terminal recursive call emitted a
tail-loop backedge. That state belongs to the emitted path. A guarded body can
take the backedge while its false continuation reaches later terminal calls.
At the guarded merge, reset both termination and backedge state before emitting
the continuation. Otherwise the later call's return is suppressed and the
function receives a default zero return.

The regression uses a guarded recursive call followed by a distinct terminal
call on wasm32. This is the shape that made float digit extraction return zero
and caused recommendation cards to display `0 / 5 predicted` for valid scores.
