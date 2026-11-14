-- SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
import Lean

namespace BrievAssurance

inductive Verdict where
  | sat
  | unsat
  | unknown
  deriving DecidableEq, Repr

-- This model starts AFTER line splitting and whitespace normalization.
-- It is not yet an extracted or refined model of the Rust implementation.
def classify (line : String) : Verdict :=
  if line = "unsat" then .unsat
  else if line = "sat" then .sat
  else .unknown

def classifyLines : List String → Verdict
  | [] => .unknown
  | line :: rest => if line = "" then classifyLines rest else classify line

theorem unsat_iff_exact (line : String) :
    classify line = .unsat ↔ line = "unsat" := by
  unfold classify
  split
  · simp_all
  · split <;> simp_all

theorem sat_iff_exact (line : String) :
    classify line = .sat ↔ line = "sat" := by
  unfold classify
  split
  · simp_all
  · split <;> simp_all

-- Model text cannot override the leading status.
example : classifyLines ["sat", "(define-fun unsat () Int 1)"] = .sat := by
  decide

-- An error before a status cannot authorize a proof.
example : classifyLines ["(error invalid-assertion)", "unsat"] = .unknown := by
  decide

example : classifyLines ["unknown", "unsat"] = .unknown := by
  decide

example : classifyLines ["", "unsat", "(error model-unavailable)"] = .unsat := by
  decide

#print axioms unsat_iff_exact
#print axioms sat_iff_exact

end BrievAssurance
