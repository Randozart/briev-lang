// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception

use crate::smt::SmtResult;

/// Read the first solver response without mistaking model/error text for a verdict.
///
/// 2026-11-14: a SAT model can legally contain a variable named `unsat`.
/// Only an exact leading status authorizes a verdict; errors before that status
/// leave the query unproved. Later output may include a model or the expected
/// "model is not available" error following an UNSAT answer to `get-model`.
pub(crate) fn parse_solver_output(stdout: &str) -> SmtResult {
    let line = stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    classify_status_line(line.as_bytes())
}

/// Classify a normalized response token; no substring can authorize a proof.
///
/// 2026-11-14: keep this production kernel independent of UTF-8/line iteration
/// so bounded verification checks the actual classifier without modeling those
/// library routines. The adapter's normalization remains a separate obligation.
fn classify_status_line(line: &[u8]) -> SmtResult {
    match line {
        b"unsat" => SmtResult::Unsat,
        b"sat" => SmtResult::Sat(Vec::new()),
        _ => SmtResult::Unknown,
    }
}

#[cfg(kani)]
mod proofs {
    use super::*;

    /// Every byte token of length zero through six has exactly its stated status.
    #[kani::proof]
    #[kani::unwind(16)]
    fn normalized_status_is_exact() {
        let bytes: [u8; 6] = kani::any();
        let length: u8 = kani::any();
        kani::assume(length <= 6);
        let token = &bytes[..usize::from(length)];
        let result = classify_status_line(token);
        let unsat = matches!(&result, SmtResult::Unsat);
        let sat = matches!(&result, SmtResult::Sat(_));
        let unknown = matches!(&result, SmtResult::Unknown);
        kani::cover!(unsat);
        kani::cover!(sat);
        kani::cover!(unknown);
        kani::cover!(length == 0);
        kani::cover!(length == 6);
        assert!(unsat == (token == b"unsat"));
        assert!(sat == (token == b"sat"));
        assert!(unknown == (token != b"unsat" && token != b"sat"));
    }

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
            "wunsat\n",
            "ssat\n",
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

    #[test]
    fn normalization_preserves_unicode_whitespace_and_exact_token_boundaries() {
        let output = "\u{2003}\r\n\u{00a0}sat\u{2003}\r\nunsat\n";
        assert!(matches!(parse_solver_output(output), SmtResult::Sat(_)));
        assert!(matches!(
            parse_solver_output("\u{2028}unsat\u{2028}"),
            SmtResult::Unsat
        ));
        for output in ["sat\u{2028}unsat", "unsat\0", "sat\0"] {
            assert!(matches!(parse_solver_output(output), SmtResult::Unknown));
        }
    }
}
