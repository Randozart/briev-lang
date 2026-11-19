//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int32_t __briev_argc;
void* __briev_argv;
uint8_t* __briev_spawn_output(const uint8_t* command);
int64_t ShellCmd(int64_t command);

static void check_capture(size_t length) {
    char command[80];
    if (length == 0) strcpy(command, "exit 0");
    else snprintf(command, sizeof(command), "printf '%%0%zud' 0", length);
    uint8_t* result = __briev_spawn_output((const uint8_t*)command);
    assert(*(int64_t*)result == (int64_t)length);
    for (size_t i = 0; i < length; ++i) assert(result[8 + i] == '0');
    assert(result[8 + length] == 0);
    free(result);
}

int main(void) {
    size_t lengths[] = {0, 4095, 4096, 4097, 8192};
    for (size_t i = 0; i < sizeof(lengths) / sizeof(lengths[0]); ++i)
        check_capture(lengths[i]);
    const char* command = "printf 'a\\000b'; exit 9";
    size_t length = strlen(command);
    uint8_t* input = malloc(8 + length + 1);
    assert(input);
    *(int64_t*)input = (int64_t)length;
    memcpy(input + 8, command, length + 1);
    uint8_t* result = (uint8_t*)(uintptr_t)ShellCmd((int64_t)(uintptr_t)input);
    assert(*(int64_t*)result == 3);
    assert(memcmp(result + 8, "a\0b\0", 4) == 0);
    free(input);
    free(result);
    return 0;
}
