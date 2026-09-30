#include "codec.h"

static int shift(int c) { return c + 1; }

int codec_encode(const char *in) {
    return shift(in[0]);
}
