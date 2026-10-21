// fsm_dispatch_c.c — C reference for fsm_dispatch.bv.
// Same TCP-subset transition table, integer-coded; illegal edges add 1000000.
#include <stdio.h>
#include <stdlib.h>

static int step(int state, int event) {
    switch (state) {
        case 0: // Closed
            if (event == 0) return 1;  // PassiveOpen -> Listen
            if (event == 1) return 2;  // ActiveOpen -> SynSent
            return -1;
        case 1: // Listen
            if (event == 2) return 3;  // RecvSyn -> SynRcvd
            if (event == 6) return 0;  // Close -> Closed
            return -1;
        case 2: // SynSent
            if (event == 3) return 4;  // RecvSynAck -> Established
            if (event == 6) return 0;  // Close -> Closed
            return -1;
        case 3: // SynRcvd
            if (event == 4) return 4;  // RecvAck -> Established
            if (event == 6) return 0;  // Close -> Closed
            return -1;
        case 4: // Established
            if (event == 5) return 7;  // RecvFin -> CloseWait
            if (event == 6) return 5;  // Close -> FinWait1
            return -1;
        case 5: // FinWait1
            if (event == 4) return 6;  // RecvAck -> FinWait2
            if (event == 5) return 8;  // RecvFin -> Closing
            return -1;
        case 6: // FinWait2
            if (event == 5) return 10; // RecvFin -> TimeWait
            return -1;
        case 7: // CloseWait
            if (event == 6) return 9;  // Close -> LastAck
            return -1;
        case 8: // Closing
            if (event == 4) return 10; // RecvAck -> TimeWait
            return -1;
        case 9: // LastAck
            if (event == 4) return 0;  // RecvAck -> Closed
            return -1;
        case 10: // TimeWait
            if (event == 6) return 0;  // Close -> Closed
            return -1;
        default:
            return -1;
    }
}

int main(void) {
    const char *b = getenv("BOUND");
    long n = b ? atol(b) : 0;
    long state = 0;
    long sum = 0;
    for (long i = 0; i < n; i++) {
        int next = step((int)state, (int)(i % 7));
        if (next < 0) {
            sum += 1000000;
        } else {
            state = next;
            sum += state;
        }
    }
    printf("%ld\n", sum);
    return 0;
}
