-- SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
import Lean

namespace BrievIntegerVC

-- Deliberately restricted to total wrapping integer operations. Division,
-- memory, calls, effects and control flow need separate semantics/refinement.
abbrev Word := BitVec 64
abbrev State := Nat → Word

inductive Expr where
  | literal (value : Word)
  | variable (name : Nat)
  | add (left right : Expr)
  | sub (left right : Expr)
  | mul (left right : Expr)

def eval (s : State) : Expr → Word
  | .literal v => v
  | .variable x => s x
  | .add l r => eval s l + eval s r
  | .sub l r => eval s l - eval s r
  | .mul l r => eval s l * eval s r

def update (s : State) (x : Nat) (v : Word) : State :=
  fun y => if y = x then v else s y

def subst (x : Nat) (rhs : Expr) : Expr → Expr
  | .literal v => .literal v
  | .variable y => if y = x then rhs else .variable y
  | .add l r => .add (subst x rhs l) (subst x rhs r)
  | .sub l r => .sub (subst x rhs l) (subst x rhs r)
  | .mul l r => .mul (subst x rhs l) (subst x rhs r)

theorem substitution_preserves_evaluation (s : State) (x : Nat) (rhs e : Expr) :
    eval s (subst x rhs e) = eval (update s x (eval s rhs)) e := by
  induction e with
  | literal v => rfl
  | «variable» y => by_cases h : y = x <;> simp [subst, eval, update, h]
  | add l r hl hr => simp [subst, eval, hl, hr]
  | sub l r hl hr => simp [subst, eval, hl, hr]
  | mul l r hl hr => simp [subst, eval, hl, hr]

inductive Assertion where
  | equal (left right : Expr)
  | signedLt (left right : Expr)
  | and (left right : Assertion)
  | implies (left right : Assertion)

def holds (s : State) : Assertion → Prop
  | .equal l r => eval s l = eval s r
  | .signedLt l r => (eval s l).toInt < (eval s r).toInt
  | .and l r => holds s l ∧ holds s r
  | .implies l r => holds s l → holds s r

def substAssertion (x : Nat) (rhs : Expr) : Assertion → Assertion
  | .equal l r => .equal (subst x rhs l) (subst x rhs r)
  | .signedLt l r => .signedLt (subst x rhs l) (subst x rhs r)
  | .and l r => .and (substAssertion x rhs l) (substAssertion x rhs r)
  | .implies l r => .implies (substAssertion x rhs l) (substAssertion x rhs r)

theorem assertion_substitution (s : State) (x : Nat) (rhs : Expr) (p : Assertion) :
    holds s (substAssertion x rhs p) ↔ holds (update s x (eval s rhs)) p := by
  induction p with
  | equal l r => simp [substAssertion, holds, substitution_preserves_evaluation]
  | signedLt l r => simp [substAssertion, holds, substitution_preserves_evaluation]
  | and l r hl hr => simp [substAssertion, holds, hl, hr]
  | implies l r hl hr => simp [substAssertion, holds, hl, hr]

-- A proved VC certifies the postcondition of the assignment in THIS model.
-- Exporter correctness and Rust/native semantics correspondence remain open.
theorem assignment_vc_sound (pre post : Assertion) (x : Nat) (rhs : Expr)
    (checked : ∀ s, holds s (.implies pre (substAssertion x rhs post))) :
    ∀ s, holds s pre → holds (update s x (eval s rhs)) post := by
  intro s hp
  exact (assertion_substitution s x rhs post).mp (checked s hp)

-- Kernel-reduced examples pin signed overflow rather than mathematical Int.
example : (eval (fun _ => 0) (.add (.literal 9223372036854775807) (.literal 1))).toInt
    = -9223372036854775808 := by decide
example : eval (fun _ => 0) (.mul (.literal 9223372036854775807) (.literal 2))
    = 18446744073709551614 := by decide

#print axioms substitution_preserves_evaluation
#print axioms assertion_substitution
#print axioms assignment_vc_sound

end BrievIntegerVC
