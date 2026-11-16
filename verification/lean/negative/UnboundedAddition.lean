-- SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
import IntegerVC

-- The tempting unbounded-integer monotonicity rule is false for Briev Int.
example : (BrievIntegerVC.eval (fun _ => 0)
    (.add (.literal 9223372036854775807) (.literal 1))).toInt > 0 := by
  decide
