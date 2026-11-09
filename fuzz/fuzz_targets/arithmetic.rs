//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
#![no_main]
use libfuzzer_sys::fuzz_target;
fuzz_target!(|data: &[u8]| briev_fuzz::semantic::check(data, false));
