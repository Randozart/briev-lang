-- SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
import IntegerVC

open BrievIntegerVC

-- Deliberately false: sequential assignment must retain the second value.
example : execute (fun _ => 0) [(0, .literal 1), (0, .literal 2)] 0 = 1 := by
  decide
