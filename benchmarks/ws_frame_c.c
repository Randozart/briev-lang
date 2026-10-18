// ws_frame — C reference for the WebSocket frame-codec benchmark.
//
// Same LCG, same corpus builder, same per-frame validation, same per-byte
// payload checksum as ws_frame.bv. Prints acc, invalid, F_TOTAL*PASSES —
// the harness's correctness gate compares all three.

#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>

#define F_TOTAL 4096
#define PASSES  4
#define LCG_MOD 2147483647LL

static int64_t ws_len_field_len(int64_t plen) {
    if (plen <= 125) return 2;
    if (plen <= 65535) return 4;
    return 10;
}

static int64_t ws_mix_len(int64_t f, int64_t draw) {
    if (f % 3 == 0) return draw % 126;
    if (f % 3 == 1) return 1000 + draw % 1000;
    return 7000 + draw % 3000;
}

static int64_t when_mask_key(int64_t draw, int64_t maskflag) {
    if (maskflag == 0) return 0;
    return draw % 4294967296LL;
}

// The codec under test, C side: decode one frame header at off.
// Returns 0 on success (-1 on any failure); fills *out.
typedef struct {
    int fin, opcode, masked, payload_len, header_len;
    int64_t mask_key;
} ws_frame_c_t;

static const char *ws_decode(const uint8_t *buf, int64_t buf_len, int64_t off,
                             ws_frame_c_t *out) {
    if (off < 0 || off + 2 > buf_len) return "incomplete";
    int b0 = buf[off];
    int b1 = buf[off + 1];
    int len7 = b1 & 127;
    int64_t plen;
    int64_t base;
    if (len7 <= 125) {
        plen = len7;
        base = 2;
    } else if (len7 == 126) {
        if (off + 4 > buf_len) return "incomplete";
        plen = ((int64_t)buf[off + 2] << 8) | buf[off + 3];
        if (plen <= 125) return "protocol";
        base = 4;
    } else {
        if (off + 10 > buf_len) return "incomplete";
        plen = 0;
        for (int i = 0; i < 8; i++)
            plen = (plen << 8) | buf[off + 2 + i];
        if (plen <= 65535 || plen < 0) return "protocol";
        base = 10;
    }
    int masked = (b1 & 128) != 0;
    int64_t key = 0;
    if (masked) {
        if (off + base + 4 > buf_len) return "incomplete";
        key = ((int64_t)buf[off + base] << 24) | ((int64_t)buf[off + base + 1] << 16)
            | ((int64_t)buf[off + base + 2] << 8) | buf[off + base + 3];
    }
    out->fin = (b0 & 128) != 0;
    out->opcode = b0 & 15;
    out->masked = masked;
    out->mask_key = key;
    out->payload_len = plen;
    out->header_len = base + (masked ? 4 : 0);
    return NULL;
}

// ws_payload_byte: the unmasked payload byte i (mask applied here).
static int ws_payload_byte(const uint8_t *buf, int64_t off,
                           const ws_frame_c_t *f, int64_t i) {
    int raw = buf[off + f->header_len + i];
    if (!f->masked) return raw;
    int shift = 8 * (3 - (int)(i & 3));
    return raw ^ (int)((f->mask_key >> shift) & 255);
}

int main(void) {
    int64_t *lens = malloc(F_TOTAL * sizeof(int64_t));
    int64_t *keys = malloc(F_TOTAL * sizeof(int64_t));
    int64_t *masks = malloc(F_TOTAL * sizeof(int64_t));
    int64_t *ops = malloc(F_TOTAL * sizeof(int64_t));
    int64_t *offs = malloc(F_TOTAL * sizeof(int64_t));

    int64_t lcg = 12345;
    int64_t total = 0;
    for (int64_t f = 0; f < F_TOTAL; f++) {
        lcg = (lcg * 1103515245 + 12345) % LCG_MOD;
        int64_t plen = ws_mix_len(f, lcg);
        int64_t maskflag = (f % 2 == 0) ? 1 : 0;
        lens[f] = plen;
        keys[f] = when_mask_key(lcg, maskflag);
        masks[f] = maskflag;
        ops[f] = f % 3;
        offs[f] = total;
        total += ws_len_field_len(plen) + maskflag * 4 + plen;
    }
    uint8_t *buf = malloc(total);

    for (int64_t f2 = 0; f2 < F_TOTAL; f2++) {
        int64_t plen2 = lens[f2];
        int64_t maskflag2 = masks[f2];
        int64_t key2 = keys[f2];
        int64_t base = offs[f2];
        buf[base] = (uint8_t)(128 | (f2 % 3));
        int l7 = 127;
        if (plen2 <= 65535) l7 = 126;
        if (plen2 <= 125) l7 = (int)plen2;
        buf[base + 1] = (uint8_t)((maskflag2 * 128) | l7);
        int64_t mut = base + 2;
        if (plen2 > 125 && plen2 <= 65535) {
            buf[mut] = (uint8_t)((plen2 >> 8) & 255);
            buf[mut + 1] = (uint8_t)(plen2 & 255);
            mut += 2;
        }
        if (plen2 > 65535) {
            for (int sh = 0; sh < 8; sh++)
                buf[mut + sh] = (uint8_t)((plen2 >> (8 * (7 - sh))) & 255);
            mut += 8;
        }
        if (maskflag2 == 1) {
            buf[mut] = (uint8_t)((key2 >> 24) & 255);
            buf[mut + 1] = (uint8_t)((key2 >> 16) & 255);
            buf[mut + 2] = (uint8_t)((key2 >> 8) & 255);
            buf[mut + 3] = (uint8_t)(key2 & 255);
        }
        int64_t pstart = base + ws_len_field_len(plen2) + maskflag2 * 4;
        for (int64_t j2 = 0; j2 < plen2; j2++)
            buf[pstart + j2] = (uint8_t)((f2 + j2) & 255);
    }

    int64_t acc = 0;
    int64_t invalid = 0;
    int64_t ticks = 0;
    for (int64_t p = 0; p < PASSES; p++) {
        for (int64_t t = 0; t < F_TOTAL; t++) {
            ws_frame_c_t f;
            const char *err = ws_decode(buf, total, offs[t], &f);
            int64_t sum;
            if (err != NULL) {
                sum = -1;
            } else {
                int64_t want_hdr = ws_len_field_len(lens[t]) + masks[t] * 4;
                if (!f.fin || f.opcode != (int)(t % 3)
                    || f.payload_len != lens[t] || f.header_len != want_hdr
                    || (masks[t] == 1 && f.mask_key != keys[t])) {
                    sum = -1;
                } else {
                    sum = 0;
                    for (int64_t i = 0; i < f.payload_len; i++)
                        sum += ws_payload_byte(buf, offs[t], &f, i);
                }
            }
            if (sum == -1) {
                invalid++;  // one count per failed decode-or-validate tick
            } else {
                acc ^= sum;
            }
            ticks++;
        }
    }
    (void)ticks;

    printf("%lld%lld%lld\n", (long long)acc, (long long)invalid,
           (long long)(int64_t)F_TOTAL * PASSES);
    free(buf);
    free(lens);
    free(keys);
    free(masks);
    free(ops);
    free(offs);
    return 0;
}
