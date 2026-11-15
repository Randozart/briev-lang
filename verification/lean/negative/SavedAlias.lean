-- SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
import IntegerVC

open BrievIntegerVC

-- Deliberately false: copying slot zero to slot one does not create an alias.
example : execute (fun _ => 7) [(1, .variable 0), (0, .literal 0)] 1 = 0 := by
  decide
