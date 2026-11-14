// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception

use crate::smt::SmtResult;

/// Read the first solver response without mistaking model/error text for a verdict.
///
/// 2026-11-14: a SAT model can legally contain a variable named `unsat`.
/// Only an exact leading status authorizes a verdict; errors before that status
/// leave the query unproved. Later output may include a model or the expected
/// "model is not available" error following an UNSAT answer to `get-model`.
pub(crate) fn parse_solver_output(stdout: &str) -> SmtResult {
    match stdout.lines().map(str::trim).find(|line| !line.is_empty()) {
        Some("unsat") => SmtResult::Unsat,
        Some("sat") => SmtResult::Sat(Vec::new()),
        _ => SmtResult::Unknown,
    }
}

#[cfg(kani)]
mod proofs {
    use super::*;

    /// Any eight-byte ASCII model suffix must preserve a leading SAT response.
    #[kani::proof]
    #[kani::unwind(16)]
    fn sat_model_cannot_authorize_unsat() {
        let suffix: [u8; 8] = kani::any();
        let mut output = *b"sat\n........";
        for index in 0..8 {
            output[4 + index] = suffix[index] & 0x7f;
        }
        let decoded = std::str::from_utf8(&output);
        assert!(decoded.is_ok());
        let stdout = decoded.unwrap_or("");
        assert!(matches!(parse_solver_output(stdout), SmtResult::Sat(_)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_exact_leading_status() {
        assert!(matches!(parse_solver_output("unsat\n"), SmtResult::Unsat));
        assert!(matches!(parse_solver_output("sat\n"), SmtResult::Sat(_)));
        assert!(matches!(
            parse_solver_output(" \n\tunsat\r\n"),
            SmtResult::Unsat
        ));
    }

    #[test]
    fn model_identifiers_cannot_change_sat_to_unsat() {
        let output = "sat\n(\n  (define-fun unsat () (_ BitVec 64) #x0000000000000001)\n)\n";
        assert!(matches!(parse_solver_output(output), SmtResult::Sat(_)));
    }

    #[test]
    fn errors_and_unknown_responses_cannot_authorize_proofs() {
        for output in [
            "",
            " \n\t",
            "unknown\n",
            "unknown\nunsat\n",
            "(error \"unknown constant unsat\")\n",
            "(error \"invalid assertion\")\nunsat\n",
            "(error \"invalid assertion\")\nsat\n",
            "not-unsat\n",
            "unsat trailing-data\n",
            "satisfiable\n",
        ] {
            assert!(
                matches!(parse_solver_output(output), SmtResult::Unknown),
                "{output:?}"
            );
        }
    }

    #[test]
    fn unavailable_model_does_not_erase_completed_unsat_check() {
        let output = "unsat\n(error \"line 5 column 10: model is not available\")\n";
        assert!(matches!(parse_solver_output(output), SmtResult::Unsat));
    }
}
