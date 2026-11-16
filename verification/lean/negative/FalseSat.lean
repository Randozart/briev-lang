-- SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
import SolverResponse

open BrievAssurance

-- Intentional false theorem: the checking gate must reject it via `decide`.
example : classifyLines ["sat", "(define-fun unsat () Int 1)"] = .unsat := by
  decide
