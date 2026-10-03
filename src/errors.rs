// Copyright 2026 Randy Smits-Schreuder Goedheijt
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
// Runtime Exception for Use as a Language:
// When the Work or any Derivative Work thereof is used to generate code
// ("generated code"), such generated code shall not be subject to the
// terms of this License, provided that the generated code itself is not
// a Derivative Work of the Work. This exception does not apply to code
// that is itself a compiler, interpreter, or similar tool that incorporates
// or embeds the Work.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub line: usize,
    pub column: usize,
}

impl Span {
    pub fn new(start: usize, end: usize, line: usize, column: usize) -> Self {
        Span {
            start,
            end,
            line,
            column,
        }
    }

    pub fn dummy() -> Self {
        Span {
            start: 0,
            end: 0,
            line: 0,
            column: 0,
        }
    }

    pub fn format(&self, source: &str) -> String {
        let lines: Vec<&str> = source.lines().collect();
        if self.line > 0 && self.line <= lines.len() {
            let line_content = lines[self.line - 1];
            let pointer = " ".repeat(self.column.saturating_sub(1)) + "^";
            format!(
                " --> {}:{}:{}\n  |\n{} | {}\n{} | {}",
                "file", self.line, self.column, self.line, line_content, self.line, pointer
            )
        } else {
            format!(" --> {}:{}:{}", "file", self.line, self.column)
        }
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub code: String,
    pub severity: Severity,
    pub title: String,
    pub span: Option<Span>,
    pub source_snippet: Option<String>,
    pub explanation: Vec<String>,
    pub proof_chain: Vec<String>,
    pub examples: Vec<String>,
    pub hints: Vec<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorMode {
    Verbose,
    Whisper,
}

impl Diagnostic {
    pub fn new(code: &str, severity: Severity, title: &str) -> Self {
        Diagnostic {
            code: code.to_string(),
            severity,
            title: title.to_string(),
            span: None,
            source_snippet: None,
            explanation: Vec::new(),
            proof_chain: Vec::new(),
            examples: Vec::new(),
            hints: Vec::new(),
            notes: Vec::new(),
        }
    }

    pub fn with_span(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }

    pub fn with_explanation(mut self, text: &str) -> Self {
        self.explanation.push(text.to_string());
        self
    }

    pub fn with_proof_step(mut self, step: &str) -> Self {
        self.proof_chain.push(step.to_string());
        self
    }

    pub fn with_example(mut self, example: &str) -> Self {
        self.examples.push(example.to_string());
        self
    }

    pub fn with_hint(mut self, hint: &str) -> Self {
        self.hints.push(hint.to_string());
        self
    }

    pub fn with_note(mut self, note: &str) -> Self {
        self.notes.push(note.to_string());
        self
    }

    pub fn format(&self, source: &str, file_name: &str) -> String {
        self.format_with_mode(source, file_name, ErrorMode::Verbose)
    }

    pub fn format_with_mode(&self, source: &str, file_name: &str, mode: ErrorMode) -> String {
        match mode {
            ErrorMode::Verbose => self.format_verbose(source, file_name),
            ErrorMode::Whisper => self.format_whisper(source, file_name),
        }
    }

    fn format_verbose(&self, source: &str, file_name: &str) -> String {
        let mut output = String::new();

        let severity_str = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
            Severity::Info => "info",
        };

        if let Some(span) = &self.span {
            output.push_str(&format!(
                "{}: {} [{}{}]\n --> {}:{}\n",
                severity_str,
                self.title,
                self.code,
                if self.code.is_empty() {
                    "".to_string()
                } else {
                    ", ".to_string()
                },
                file_name,
                span
            ));
        } else {
            output.push_str(&format!(
                "{}: {} [{}{}]\n",
                severity_str,
                self.title,
                self.code,
                if self.code.is_empty() {
                    "".to_string()
                } else {
                    ", ".to_string()
                },
            ));
        }

        if let Some(span) = &self.span {
            if self.severity == Severity::Error {
                let lines: Vec<&str> = source.lines().collect();
                if span.line > 0 && span.line <= lines.len() {
                    let line_content = lines[span.line - 1];
                    let line_str = format!("{}", span.line);
                    let padding = " ".repeat(line_str.len());

                    output.push_str(&format!("  |\n"));
                    output.push_str(&format!("{} | {}\n", line_str, line_content));
                    output.push_str(&format!(
                        "{} | {}\n",
                        padding,
                        " ".repeat(span.column.saturating_sub(1)) + "^"
                    ));
                }
            }
        }

        for line in &self.explanation {
            output.push_str(&format!("  |\n  = {}\n", line));
        }

        if !self.proof_chain.is_empty() {
            output.push_str("  |\n  = proof:\n");
            for (i, step) in self.proof_chain.iter().enumerate() {
                let prefix = if i == 0 {
                    "  =   ".to_string()
                } else {
                    "  =     ".to_string()
                };
                output.push_str(&format!("{}• {}\n", prefix, step));
            }
        }

        if !self.examples.is_empty() {
            output.push_str("  |\n  = example failure:\n");
            for example in &self.examples {
                output.push_str(&format!("  =   {}\n", example));
            }
        }

        if !self.hints.is_empty() {
            output.push_str("  |\n  = hint:");
            for hint in &self.hints {
                output.push_str(&format!(" {}\n", hint));
            }
        }

        if !self.notes.is_empty() {
            output.push_str("  |\n");
            for note in &self.notes {
                output.push_str(&format!("  = note: {}\n", note));
            }
        }

        output
    }

    fn format_whisper(&self, source: &str, file_name: &str) -> String {
        let severity_str = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
            Severity::Info => "info",
        };

        let mut parts = Vec::new();

        if let Some(span) = &self.span {
            parts.push(format!("{}:{}:{}", file_name, span.line, span.column));
        }

        parts.push(format!("[{}]", self.code));
        parts.push(self.title.clone());

        if !self.explanation.is_empty() {
            let hint = self
                .explanation
                .first()
                .map(|s| if s.len() > 50 { &s[..50] } else { s.as_str() })
                .unwrap_or("");
            if !hint.is_empty() {
                parts.push(format!("({})", hint));
            }
        }

        if !self.hints.is_empty() {
            if let Some(first_hint) = self.hints.first() {
                if first_hint.starts_with("did you mean") {
                    parts.push(
                        first_hint
                            .replace("did you mean ", "try: ")
                            .replace("'", "")
                            .replace("?", ""),
                    );
                } else {
                    parts.push(first_hint.chars().take(40).collect::<String>());
                }
            }
        }

        format!("{} {}\n", severity_str, parts.join(" "))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Note,
    Info,
}

#[derive(Debug, Clone)]
pub enum TypeError {
    UndefinedVariable {
        name: String,
        available: Vec<String>,
    },
    TypeMismatch {
        expected: String,
        found: String,
        context: String,
    },
    UninitializedSignal {
        name: String,
    },
    OwnershipViolation {
        var: String,
        reason: String,
    },
    InvalidOperation {
        operation: String,
        type_name: String,
    },
    FFIError {
        message: String,
    },
    /// 2026-07-08: Phase 5 — intrinsic was relocated to std/os/ module
    RemovedIntrinsic {
        name: String,
        module: String,
    },
    /// 2026-08-05 (Phase 6): an explicitly written `[true][true]` contract.
    /// Omitted contracts are fine; a written tautology is rejected (SPEC §10.1).
    TautologicalContract,
    /// 2026-08-05 (Phase 6): a `node`, `txn`, or `asm` declaration without a
    /// contract. These declarations must declare pre/post conditions so the
    /// compiler can prove and classify the transition.
    MissingContract {
        declaration: String,
    },
    /// 2026-08-17 (foreach break): `break;` written outside any `foreach` body.
    BreakOutsideLoop {
        span: Span,
    },
}

impl fmt::Display for TypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeError::UndefinedVariable { name, .. } => {
                write!(f, "undefined variable '{}'", name)
            }
            TypeError::TypeMismatch {
                expected,
                found,
                context,
                ..
            } => {
                write!(
                    f,
                    "type mismatch: expected {} for {}, found {}",
                    expected, context, found
                )
            }
            TypeError::UninitializedSignal { name, .. } => {
                write!(f, "signal '{}' has no initial value", name)
            }
            TypeError::OwnershipViolation { var, reason, .. } => {
                write!(f, "ownership violation on '{}': {}", var, reason)
            }
            TypeError::InvalidOperation {
                operation,
                type_name,
                ..
            } => {
                write!(f, "invalid operation '{}' on type {}", operation, type_name)
            }
            TypeError::FFIError { message, .. } => {
                write!(f, "FFI error: {}", message)
            }
            TypeError::RemovedIntrinsic { name, module, .. } => {
                write!(f, "intrinsic '{}' was moved to {} (auto-imported via prelude). If using --no-std, add: import \"{}\";", name, module, module)
            }
            TypeError::TautologicalContract => {
                write!(f, "the contract [true][true] asserts nothing (true ⇒ true is trivial), so it is indistinguishable from an omitted contract and the compiler records no obligation; omit the brackets, or state the conditions you want proven")
            }
            TypeError::MissingContract { declaration } => {
                write!(f, "'{}' must declare a contract with pre and post conditions so the compiler can prove and classify the transition", declaration)
            }
            TypeError::BreakOutsideLoop { .. } => {
                write!(f, "'break' may only appear inside a 'foreach' body (it exits the innermost enclosing 'foreach'); at top level there is no loop to break out of")
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum ProofError {
    UnreachableState {
        transaction: String,
        precondition: String,
        reason: String,
        proof_trace: Vec<String>,
        span: Span,
    },
    PostconditionUnsatisfiable {
        transaction: String,
        postcondition: String,
        reason: String,
        example_values: Vec<String>,
        suggestion: String,
        span: Span,
    },
    NoAcceptingPath {
        transaction: String,
        reason: String,
        rollback_count: usize,
        span: Span,
    },
    MutualExclusionViolation {
        txn1: String,
        txn2: String,
        shared_vars: Vec<String>,
        conflict_description: String,
        span: Span,
    },
    UnhandledOutcome {
        signature: String,
        union_type: String,
        missing_variants: Vec<String>,
        span: Span,
    },
    TrueAssertionFailure {
        signature: String,
        reason: String,
        proof_steps: Vec<String>,
        span: Span,
    },
    CircularDependency {
        transactions: Vec<String>,
        call_chain: Vec<String>,
        span: Span,
    },
    ImpossiblePrecondition {
        condition: String,
        contradiction: String,
        span: Span,
    },
    PostconditionMutationViolation {
        transaction: String,
        postcondition: String,
        mutation: String,
        explanation: String,
        span: Span,
    },
    TrivialPrecondition {
        item_name: String,
        item_type: String,
        span: Span,
    },
    TrivialPostcondition {
        item_name: String,
        item_type: String,
        span: Span,
    },
}

impl fmt::Display for ProofError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProofError::UnreachableState { transaction, .. } => {
                write!(
                    f,
                    "transaction '{}' has unreachable precondition",
                    transaction
                )
            }
            ProofError::PostconditionUnsatisfiable { transaction, .. } => {
                write!(
                    f,
                    "transaction '{}' postcondition cannot be satisfied",
                    transaction
                )
            }
            ProofError::NoAcceptingPath { transaction, .. } => {
                write!(f, "transaction '{}' has no valid termination", transaction)
            }
            ProofError::MutualExclusionViolation { txn1, txn2, .. } => {
                write!(
                    f,
                    "transactions '{}' and '{}' have unsafe concurrent access",
                    txn1, txn2
                )
            }
            ProofError::UnhandledOutcome { signature, .. } => {
                write!(f, "unhandled outcome for signature '{}'", signature)
            }
            ProofError::TrueAssertionFailure { signature, .. } => {
                write!(f, "true assertion failed for signature '{}'", signature)
            }
            ProofError::CircularDependency { .. } => {
                write!(f, "circular transaction dependency detected")
            }
            ProofError::ImpossiblePrecondition { condition, .. } => {
                write!(f, "precondition '{}' is impossible to satisfy", condition)
            }
            ProofError::PostconditionMutationViolation { transaction, .. } => {
                write!(
                    f,
                    "transaction '{}' postcondition references mutated state incorrectly",
                    transaction
                )
            }
            ProofError::TrivialPrecondition {
                item_name,
                item_type,
                ..
            } => {
                write!(
                    f,
                    "{} '{}' has a trivial precondition '[true]'",
                    item_type, item_name
                )
            }
            ProofError::TrivialPostcondition {
                item_name,
                item_type,
                ..
            } => {
                write!(
                    f,
                    "{} '{}' has a trivial postcondition '[true]'",
                    item_type, item_name
                )
            }
        }
    }
}

/// Errors from fuzz case verification.
#[derive(Debug, Clone)]
pub enum FuzzError {
    /// Expected output does not match actual.
    Mismatch {
        function: String,
        case_index: usize,
        inputs: String,
        expected: String,
        actual: String,
        span: Span,
    },
    /// Fuzz inputs violate the item's precondition.
    InvalidInput {
        function: String,
        case_index: usize,
        detail: String,
        span: Span,
    },
    /// BILD simulation encountered an unrecoverable opaque instruction.
    Unverifiable {
        function: String,
        case_index: usize,
        detail: String,
        span: Span,
    },
    /// A required parameter was not bound in the fuzz case.
    MissingBinding {
        function: String,
        case_index: usize,
        param: String,
        span: Span,
    },
    /// Cell fuzzing is not yet supported.
    Skipped {
        function: String,
        reason: String,
        span: Span,
    },
    /// The fuzz case could not be evaluated (runtime error).
    EvaluationError {
        function: String,
        case_index: usize,
        message: String,
        span: Span,
    },
}

impl fmt::Display for FuzzError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FuzzError::Mismatch {
                function,
                case_index,
                inputs,
                expected,
                actual,
                ..
            } => {
                write!(
                    f,
                    "fuzz case {} of '{}' failed: expected {}, got {}",
                    case_index, function, expected, actual
                )?;
                if !inputs.is_empty() {
                    write!(f, " (inputs: {})", inputs)?;
                }
                Ok(())
            }
            FuzzError::InvalidInput {
                function,
                case_index,
                detail,
                ..
            } => {
                write!(
                    f,
                    "fuzz case {} of '{}': precondition not satisfied: {}",
                    case_index, function, detail
                )
            }
            FuzzError::Unverifiable {
                function,
                case_index,
                detail,
                ..
            } => {
                write!(
                    f,
                    "fuzz case {} of '{}': cannot verify — {}",
                    case_index, function, detail
                )
            }
            FuzzError::MissingBinding {
                function,
                case_index,
                param,
                ..
            } => {
                write!(
                    f,
                    "fuzz case {} of '{}': missing binding for parameter '{}'",
                    case_index, function, param
                )
            }
            FuzzError::Skipped {
                function, reason, ..
            } => {
                write!(f, "fuzz check skipped for '{}': {}", function, reason)
            }
            FuzzError::EvaluationError {
                function,
                case_index,
                message,
                ..
            } => {
                write!(
                    f,
                    "fuzz case {} of '{}' raised error: {}",
                    case_index, function, message
                )
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum SyntaxError {
    /// 2026-08-23 (F3): multiple parse errors collected during recovery.
    ParseErrors(Vec<String>),
    UnexpectedToken {
        expected: String,
        found: String,
        span: Span,
    },
    UnexpectedEOF {
        expected: String,
        span: Span,
    },
    InvalidExpression {
        reason: String,
        span: Span,
    },
    InvalidStatement {
        reason: String,
        span: Span,
    },
    InvalidType {
        type_name: String,
        span: Span,
    },
    /// 2026-08-05 (normative spec Phase 0): a construct is normative in
    /// spec/SPEC.md but is not yet implemented. The compiler must reject it
    /// explicitly instead of accepting a placeholder/subset semantics. See
    /// SPEC §25 and docs/plans/2026-08-05-implement-normative-language-spec.md.
    StagedFeature {
        feature: String,
        span: Span,
    },
    /// 2026-08-17: the token is a reserved KEYWORD that cannot be used as an
    /// identifier in this position (e.g. the `out`/`vol`/`seq` modifiers).
    /// A specific message beats the generic "expected identifier, found 'out'".
    ReservedKeyword {
        keyword: String,
        span: Span,
    },
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 2026-08-23 (F3): multi-error variant formats all collected errors.
        if let SyntaxError::ParseErrors(errors) = self {
            writeln!(f, "{} parse error(s):", errors.len())?;
            for e in errors {
                writeln!(f, "  - {}", e)?;
            }
            return Ok(());
        }

        let span_str = match self {
            SyntaxError::UnexpectedToken { span, .. } => format!(" at {}", span),
            SyntaxError::UnexpectedEOF { span, .. } => format!(" at {}", span),
            SyntaxError::InvalidExpression { span, .. } => format!(" at {}", span),
            SyntaxError::InvalidStatement { span, .. } => format!(" at {}", span),
            SyntaxError::InvalidType { span, .. } => format!(" at {}", span),
            SyntaxError::StagedFeature { span, .. } => format!(" at {}", span),
            SyntaxError::ReservedKeyword { span, .. } => format!(" at {}", span),
            _ => String::new(),
        };

        match self {
            SyntaxError::UnexpectedToken {
                expected, found, ..
            } => {
                write!(f, "expected {}, found '{}'{}", expected, found, span_str)
            }
            SyntaxError::UnexpectedEOF { expected, .. } => {
                write!(f, "expected {}, found end of file{}", expected, span_str)
            }
            SyntaxError::InvalidExpression { reason, .. } => {
                write!(f, "invalid expression: {}{}", reason, span_str)
            }
            SyntaxError::ReservedKeyword { keyword, .. } => {
                write!(f, "'{}' is a reserved keyword and cannot be used as an identifier{}", keyword, span_str)
            }
            SyntaxError::InvalidStatement { reason, .. } => {
                write!(f, "invalid statement: {}{}", reason, span_str)
            }
            SyntaxError::InvalidType { type_name, .. } => {
                write!(f, "invalid type: '{}'{}", type_name, span_str)
            }
            SyntaxError::StagedFeature { feature, .. } => {
                write!(
                    f,
                    "feature '{}' is specified in the language but not yet implemented in this build{}",
                    feature, span_str
                )
            }
            _ => Ok(()),
        }
    }
}

impl From<String> for SyntaxError {
    fn from(s: String) -> Self {
        SyntaxError::InvalidStatement {
            reason: s,
            span: Span::dummy(),
        }
    }
}

impl std::error::Error for SyntaxError {}

#[derive(Debug, Clone)]
pub enum ImportError {
    ModuleNotFound {
        module: String,
        search_paths: Vec<String>,
        span: Span,
    },
    CircularImport {
        module: String,
        import_chain: Vec<String>,
        span: Span,
    },
    InvalidImport {
        reason: String,
        span: Span,
    },
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImportError::ModuleNotFound { module, .. } => {
                write!(f, "module '{}' not found", module)
            }
            ImportError::CircularImport { module, .. } => {
                write!(f, "circular import detected for module '{}'", module)
            }
            ImportError::InvalidImport { reason, .. } => {
                write!(f, "invalid import: {}", reason)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum ContractError {
    PreconditionUnsatisfiable {
        condition: String,
        proof: String,
        span: Span,
    },
    PostconditionUnsatisfiable {
        condition: String,
        proof: String,
        span: Span,
    },
    GuardViolation {
        guard: String,
        explanation: String,
        span: Span,
    },
}

impl fmt::Display for ContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ContractError::PreconditionUnsatisfiable { condition, .. } => {
                write!(f, "precondition '{}' can never be true", condition)
            }
            ContractError::PostconditionUnsatisfiable { condition, .. } => {
                write!(f, "postcondition '{}' can never be true", condition)
            }
            ContractError::GuardViolation { guard, .. } => {
                write!(f, "guard '{}' violation", guard)
            }
        }
    }
}

// ── New Architecture Error Types (2026-07-12 rewrite) ────────────────────
//
// These error types are for the new compiler architecture. They are additive —
// existing error types are untouched. As phases progress, older error types
// will be consolidated into CompilerError variants.
//
// 2026-07-12: Phase 0.0 — AllocError, DeriveError, BackendError, CompilerError

/// Errors from alloc annotation validation (Phase A/A.1 of the rewrite).
///
/// Validated at type-check time: alloc("Stack") must pass escape analysis,
/// alloc(0x...) must be a compile-time constant, etc.
#[derive(Debug, Clone)]
pub enum AllocError {
    /// Variable with alloc("Stack") escapes the current scope.
    Escape { name: String, span: Option<Span> },
    /// alloc(0x...) requires a compile-time constant address.
    AddressNotConstant { name: String, span: Option<Span> },
    /// Physical address is outside the target's memory map (backend-validated,
    /// but caught at type-check time if the constant is clearly out of range).
    AddressOutOfRange {
        name: String,
        address: i64,
        span: Option<Span>,
    },
}

impl fmt::Display for AllocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AllocError::Escape { name, .. } => {
                write!(
                    f,
                    "variable '{}' is annotated alloc(\"Stack\") but escapes the current scope",
                    name
                )
            }
            AllocError::AddressNotConstant { name, .. } => {
                write!(
                    f,
                    "alloc annotation on '{}' requires a compile-time constant address",
                    name
                )
            }
            AllocError::AddressOutOfRange { name, address, .. } => {
                write!(
                    f,
                    "alloc address 0x{:x} on '{}' is out of range",
                    address, name
                )
            }
        }
    }
}

/// Errors from the derivation/synthesis engine (Phase 6 of the rewrite).
#[derive(Debug, Clone)]
pub enum DeriveError {
    /// Derivation block has no examples.
    NoExamples { function: String },
    /// Example input types don't match the function signature.
    ExampleTypeMismatch {
        function: String,
        example_index: usize,
        expected: String,
        found: String,
    },
    /// Synthesis found no valid expression within the depth bound.
    SynthesisFailed {
        function: String,
        max_depth: usize,
        reason: String,
    },
    /// SMT solver returned an error.
    SolverError { function: String, message: String },
    /// Derivation requires SMT but --no-smt or solver unavailable.
    SolverUnavailable { function: String },
}

impl fmt::Display for DeriveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeriveError::NoExamples { function } => {
                write!(f, "derivation block for '{}' has no examples", function)
            }
            DeriveError::ExampleTypeMismatch {
                function,
                example_index,
                expected,
                found,
            } => {
                write!(
                    f,
                    "derivation example {} for '{}': expected type {}, got {}",
                    example_index, function, expected, found
                )
            }
            DeriveError::SynthesisFailed {
                function,
                max_depth,
                reason,
            } => {
                write!(
                    f,
                    "synthesis of '{}' failed at depth {}: {}",
                    function, max_depth, reason
                )
            }
            DeriveError::SolverError { function, message } => {
                write!(f, "SMT solver error for '{}': {}", function, message)
            }
            DeriveError::SolverUnavailable { function } => {
                write!(
                    f,
                    "SMT solver is not available; derivation of '{}' requires it",
                    function
                )
            }
        }
    }
}

/// Errors from backend code generation (Phase 4 of the rewrite).
///
/// Contains backend-specific variants for LLVM, CIRCT, and Webstack.
/// Each variant carries enough context for a backend-specific diagnostic.
#[derive(Debug, Clone)]
pub enum BackendError {
    /// Generic codegen failure with message.
    CodegenFailed { message: String, span: Option<Span> },
    /// LLVM-specific error.
    Llvm(LlvmError),
    /// CIRCT-specific error.
    Circt(CirctError),
    /// Webstack-specific error.
    Webstack(WebstackError),
    /// Unknown or unhandled target.
    UnsupportedTarget { target: String, span: Option<Span> },
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BackendError::CodegenFailed { message, .. } => {
                write!(f, "codegen failed: {}", message)
            }
            BackendError::Llvm(err) => fmt::Display::fmt(err, f),
            BackendError::Circt(err) => fmt::Display::fmt(err, f),
            BackendError::Webstack(err) => fmt::Display::fmt(err, f),
            BackendError::UnsupportedTarget { target, .. } => {
                write!(f, "unsupported target: {}", target)
            }
        }
    }
}

/// LLVM-specific codegen error.
#[derive(Debug, Clone)]
pub enum LlvmError {
    /// LLVM instruction emission failed.
    InstructionFailed {
        instruction: String,
        reason: String,
        span: Option<Span>,
    },
    /// Unknown or unsupported alloc strategy for LLVM target.
    UnknownAllocTarget {
        target: String,
        binding: String,
        span: Option<Span>,
    },
    /// Arena allocation missing base pointer.
    MissingArenaPointer { binding: String, span: Option<Span> },
    /// Physical address not in target memory map.
    AddressNotInMemoryMap {
        address: i64,
        target: String,
        span: Option<Span>,
    },
}

impl fmt::Display for LlvmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LlvmError::InstructionFailed {
                instruction,
                reason,
                ..
            } => {
                write!(f, "LLVM instruction '{}' failed: {}", instruction, reason)
            }
            LlvmError::UnknownAllocTarget {
                target, binding, ..
            } => {
                write!(
                    f,
                    "unknown alloc target '{}' for binding '{}'",
                    target, binding
                )
            }
            LlvmError::MissingArenaPointer { binding, .. } => {
                write!(
                    f,
                    "alloc(\"Arena\") on '{}' requires a pointer argument",
                    binding
                )
            }
            LlvmError::AddressNotInMemoryMap {
                address, target, ..
            } => {
                write!(
                    f,
                    "address 0x{:x} is not in the memory map for target '{}'",
                    address, target
                )
            }
        }
    }
}

/// CIRCT-specific codegen error.
#[derive(Debug, Clone)]
pub enum CirctError {
    /// Physical address not mapped on the target device.
    AddressNotMapped {
        address: i64,
        device: String,
        available_range: String,
        span: Option<Span>,
    },
    /// Unknown alloc strategy for hardware target.
    UnknownAlloc {
        strategy: String,
        binding: String,
        span: Option<Span>,
    },
}

impl fmt::Display for CirctError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CirctError::AddressNotMapped {
                address,
                device,
                available_range,
                ..
            } => {
                write!(
                    f,
                    "address 0x{:x} not mapped on device '{}' (available: {})",
                    address, device, available_range
                )
            }
            CirctError::UnknownAlloc {
                strategy, binding, ..
            } => {
                write!(
                    f,
                    "unknown alloc strategy '{}' for binding '{}'",
                    strategy, binding
                )
            }
        }
    }
}

/// Webstack-specific codegen error.
#[derive(Debug, Clone)]
pub enum WebstackError {
    /// WASM emission failed.
    WasmEmitFailed { reason: String, span: Option<Span> },
}

impl fmt::Display for WebstackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WebstackError::WasmEmitFailed { reason, .. } => {
                write!(f, "WASM emission failed: {}", reason)
            }
        }
    }
}

/// Runtime error during compile-time evaluation (interpreter).
#[derive(Debug, Clone)]
pub enum RuntimeError {
    UndefinedVariable { name: String },
    UndefinedFunction(String),
    DivisionByZero,
    HeapError(String),
    UnsupportedIntrinsic(String),
    TypeError { expected: String, found: String },
    UndefinedForeignFunction { name: String, source: String },
    ContractViolation(String),
    /// 2026-07-28: Term statement evaluated — early return with value.
    /// Used by the interpreter's call_function to detect termination.
    TermReturn(crate::interpreter::Value),
    /// 2026-08-17 (foreach break): `break;` evaluated — exit the innermost
    /// enclosing `foreach`. The foreach's own evaluator intercepts this and
    /// stops iterating (does NOT propagate outward), so it is internally
    /// swallowed; the variant exists as the escape signal.
    Break,
    /// 2026-08-06 (endprogram plan): `endprogram` evaluated — the process
    /// boundary (SPEC §11.5). Carries the exit code value. The reactor stops
    /// on this, unlike TermReturn (which ends the transaction only).
    ProgramExit(crate::interpreter::Value),
    /// 2026-08-06 (Slice C): match expression where no arm's pattern matched
    /// (or its `when` guard failed). Carries the scrutinee value description.
    NonExhaustiveMatch(String),
    /// 2026-08-09 (init kind, Phase 2): an attempt to re-seed or reassign an
    /// `init` — the value is set once before beginprogram and is immutable for
    /// the run. The interpreter is the reference: codegen must match.
    ImmutableInit(String),
    /// 2026-08-13 (layout-keywords plan Phase 4): `trap;` evaluated — a
    /// hardware abort (SPEC §8.8). The reference interpreter reports the abort
    /// like the LLVM `llvm.trap` + `unreachable` sequence.
    Trap,
    /// 2026-09-06 (Halt# slice): `Halt#()` evaluated — the bare-metal halt.
    /// Embedded targets enter the low-power wait state (`wfi`) in a loop;
    /// the reference interpreter reports the halt (the program stops here,
    /// no further transactions fire).
    Halt,
    /// 2026-08-26 (async Phase B, docs/plans/2026-08-26-async-phase-b.md):
    /// a task read an unready event port — the scheduler signal to suspend.
    /// NEVER a user-facing error: the segment executors catch it, having
    /// already registered the task as a slot waiter (status `Waiting`). If
    /// this escapes to the surface, a segment executor forgot its catch arm.
    TaskBlocked,
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuntimeError::UndefinedVariable { name } => {
                write!(f, "undefined variable '{}'", name)
            }
            RuntimeError::UndefinedFunction(name) => {
                write!(f, "undefined function '{}'", name)
            }
            RuntimeError::DivisionByZero => write!(f, "division by zero"),
            RuntimeError::HeapError(msg) => write!(f, "heap error: {}", msg),
            RuntimeError::UnsupportedIntrinsic(name) => {
                write!(f, "unsupported intrinsic '{}'", name)
            }
            RuntimeError::TypeError { expected, found } => {
                write!(f, "type error: expected {}, got {}", expected, found)
            }
            RuntimeError::UndefinedForeignFunction { name, source } => {
                write!(f, "undefined foreign function '{}' from {}", name, source)
            }
            RuntimeError::ContractViolation(msg) => write!(f, "contract violation: {}", msg),
            RuntimeError::TermReturn(_) => write!(f, "term return"),
            RuntimeError::Break => write!(f, "break (exit foreach)"),
            RuntimeError::ProgramExit(_) => write!(f, "endprogram (process exit)"),
            RuntimeError::NonExhaustiveMatch(desc) => {
                write!(f, "non-exhaustive match: no arm matched {}", desc)
            }
            RuntimeError::ImmutableInit(name) => {
                write!(f, "cannot modify '{}': an init is seeded once before beginprogram and is immutable for the run", name)
            }
            RuntimeError::Trap => {
                write!(f, "trap: the program executed a hardware abort (SPEC §8.8)")
            }
            RuntimeError::Halt => {
                write!(f, "halt: the program entered the wait-for-interrupt state (Halt#) — no further code runs")
            }
            RuntimeError::TaskBlocked => {
                write!(f, "internal: task blocked on an unready event port but no scheduler caught the suspension")
            }
        }
    }
}

impl From<RuntimeError> for CompilerError {
    fn from(err: RuntimeError) -> Self {
        CompilerError::Runtime(err)
    }
}

// ── Into impls: convert specific error types into CompilerError ──

/// Top-level compiler error, wrapping all specific error types.
///
/// This provides a common error type for the compilation pipeline.
/// Each specific error type can be converted into CompilerError via From.
#[derive(Debug, Clone)]
pub enum CompilerError {
    /// Invalid file path or filename format.
    InvalidPath,
    /// Malformed filename (expected [name].[bv] or [name].[flags].[bv]).
    MalformedFilename(String),
    /// Syntax/parse error.
    Syntax(SyntaxError),
    /// Type-checking error.
    Type(TypeError),
    /// Import resolution error.
    Import(ImportError),
    /// Contract verification error.
    Contract(ContractError),
    /// Alloc annotation validation error.
    Alloc(AllocError),
    /// Derivation/synthesis error.
    Derive(DeriveError),
    /// Backend codegen error.
    Backend(BackendError),
    /// Fuzz verification error.
    Fuzz(FuzzError),
    /// Proof engine error.
    Proof(ProofError),
    /// I/O error.
    Io(String),
    /// Runtime/interpreter error.
    Runtime(RuntimeError),
}

impl fmt::Display for CompilerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompilerError::InvalidPath => write!(f, "invalid file path"),
            CompilerError::MalformedFilename(msg) => write!(f, "malformed filename: {}", msg),
            CompilerError::Syntax(err) => fmt::Display::fmt(err, f),
            CompilerError::Type(err) => fmt::Display::fmt(err, f),
            CompilerError::Import(err) => fmt::Display::fmt(err, f),
            CompilerError::Contract(err) => fmt::Display::fmt(err, f),
            CompilerError::Alloc(err) => fmt::Display::fmt(err, f),
            CompilerError::Derive(err) => fmt::Display::fmt(err, f),
            CompilerError::Backend(err) => fmt::Display::fmt(err, f),
            CompilerError::Fuzz(err) => fmt::Display::fmt(err, f),
            CompilerError::Proof(err) => fmt::Display::fmt(err, f),
            CompilerError::Io(msg) => write!(f, "I/O error: {}", msg),
            CompilerError::Runtime(err) => fmt::Display::fmt(err, f),
        }
    }
}

impl From<SyntaxError> for CompilerError {
    fn from(err: SyntaxError) -> Self {
        CompilerError::Syntax(err)
    }
}

impl From<TypeError> for CompilerError {
    fn from(err: TypeError) -> Self {
        CompilerError::Type(err)
    }
}

impl From<ImportError> for CompilerError {
    fn from(err: ImportError) -> Self {
        CompilerError::Import(err)
    }
}

impl From<ContractError> for CompilerError {
    fn from(err: ContractError) -> Self {
        CompilerError::Contract(err)
    }
}

impl From<AllocError> for CompilerError {
    fn from(err: AllocError) -> Self {
        CompilerError::Alloc(err)
    }
}

impl From<DeriveError> for CompilerError {
    fn from(err: DeriveError) -> Self {
        CompilerError::Derive(err)
    }
}

impl From<BackendError> for CompilerError {
    fn from(err: BackendError) -> Self {
        CompilerError::Backend(err)
    }
}

impl From<FuzzError> for CompilerError {
    fn from(err: FuzzError) -> Self {
        CompilerError::Fuzz(err)
    }
}

impl From<ProofError> for CompilerError {
    fn from(err: ProofError) -> Self {
        CompilerError::Proof(err)
    }
}

impl From<LlvmError> for CompilerError {
    fn from(err: LlvmError) -> Self {
        CompilerError::Backend(BackendError::Llvm(err))
    }
}

impl From<CirctError> for CompilerError {
    fn from(err: CirctError) -> Self {
        CompilerError::Backend(BackendError::Circt(err))
    }
}

impl From<WebstackError> for CompilerError {
    fn from(err: WebstackError) -> Self {
        CompilerError::Backend(BackendError::Webstack(err))
    }
}

impl std::error::Error for CompilerError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_staged_feature_diagnostic_display() {
        let err = SyntaxError::StagedFeature {
            feature: "dyn Trait".into(),
            span: Span::dummy(),
        };
        let text = format!("{}", err);
        assert!(text.contains("feature 'dyn Trait'"));
        assert!(text.contains("not yet implemented"));
    }

    #[test]
    fn test_span_new_and_dummy() {
        let s = Span::new(10, 20, 5, 3);
        assert_eq!(s.start, 10);
        assert_eq!(s.end, 20);
        assert_eq!(s.line, 5);
        assert_eq!(s.column, 3);
        let d = Span::dummy();
        assert_eq!(d.start, 0);
        assert_eq!(d.end, 0);
        assert_eq!(d.line, 0);
        assert_eq!(d.column, 0);
    }

    #[test]
    fn test_span_display() {
        let s = Span::new(0, 0, 7, 12);
        assert_eq!(format!("{}", s), "7:12");
    }

    #[test]
    fn test_span_format_with_source() {
        let s = Span::new(6, 7, 1, 7);
        let source = "let x = 42;";
        let formatted = s.format(source);
        assert!(formatted.contains("1 | let x = 42;"));
        assert!(formatted.contains("file:1:7"));
    }

    #[test]
    fn test_diagnostic_verbose_format() {
        let diag = Diagnostic::new("E001", Severity::Error, "test error")
            .with_span(Span::new(0, 1, 1, 1))
            .with_explanation("something went wrong")
            .with_hint("try this instead")
            .with_note("note here");
        let output = diag.format_verbose("x=1", "test.bv");
        assert!(output.contains("error:"));
        assert!(output.contains("E001"));
        assert!(output.contains("something went wrong"));
        assert!(output.contains("try this instead"));
        assert!(output.contains("note here"));
    }

    #[test]
    fn test_diagnostic_whisper_format() {
        let diag = Diagnostic::new("W001", Severity::Warning, "warning title")
            .with_span(Span::new(0, 1, 2, 3));
        let output = diag.format_whisper("x=1", "test.bv");
        assert!(output.len() < 200);
        assert!(output.contains("warning"));
        assert!(output.contains("W001"));
    }

    #[test]
    fn test_diagnostic_builder_methods() {
        let d = Diagnostic::new("C001", Severity::Note, "info")
            .with_proof_step("step 1")
            .with_proof_step("step 2")
            .with_example("example 1")
            .with_span(Span::dummy());
        assert_eq!(d.proof_chain.len(), 2);
        assert_eq!(d.examples.len(), 1);
        assert!(d.span.is_some());
    }

    #[test]
    fn test_diagnostic_empty_code_format() {
        let diag = Diagnostic::new("", Severity::Info, "no code");
        let output = diag.format_verbose("", "test.bv");
        assert!(output.contains("info:"));
        assert!(output.contains("no code"));
    }

    #[test]
    fn test_error_mode_enum() {
        assert_ne!(ErrorMode::Verbose, ErrorMode::Whisper);
        assert_eq!(ErrorMode::Verbose, ErrorMode::Verbose);
    }

    // ── New Architecture Error Type Tests ──

    #[test]
    fn test_alloc_error_escape() {
        let err = AllocError::Escape {
            name: "buffer".into(),
            span: None,
        };
        let msg = format!("{}", err);
        assert!(msg.contains("buffer"));
        assert!(msg.contains("Stack"));
        assert!(msg.contains("escape"));
    }

    #[test]
    fn test_alloc_error_address_not_constant() {
        let err = AllocError::AddressNotConstant {
            name: "reg".into(),
            span: None,
        };
        let msg = format!("{}", err);
        assert!(msg.contains("reg"));
        assert!(msg.contains("constant"));
    }

    #[test]
    fn test_alloc_error_out_of_range() {
        let err = AllocError::AddressOutOfRange {
            name: "mmio".into(),
            address: 0xFFFF_FFFF,
            span: None,
        };
        let msg = format!("{}", err);
        assert!(msg.contains("mmio"));
        assert!(msg.contains("ffffffff"));
    }

    #[test]
    fn test_derive_error_no_examples() {
        let err = DeriveError::NoExamples {
            function: "add".into(),
        };
        let msg = format!("{}", err);
        assert!(msg.contains("add"));
        assert!(msg.contains("no examples"));
    }

    #[test]
    fn test_derive_error_type_mismatch() {
        let err = DeriveError::ExampleTypeMismatch {
            function: "add".into(),
            example_index: 0,
            expected: "Int".into(),
            found: "String".into(),
        };
        let msg = format!("{}", err);
        assert!(msg.contains("add"));
        assert!(msg.contains("Int"));
        assert!(msg.contains("String"));
    }

    #[test]
    fn test_derive_error_synthesis_failed() {
        let err = DeriveError::SynthesisFailed {
            function: "fib".into(),
            max_depth: 5,
            reason: "no valid expression found".into(),
        };
        let msg = format!("{}", err);
        assert!(msg.contains("fib"));
        assert!(msg.contains("5"));
    }

    #[test]
    fn test_llvm_error_instruction_failed() {
        let err = LlvmError::InstructionFailed {
            instruction: "add nsw i64".into(),
            reason: "operand type mismatch".into(),
            span: None,
        };
        let msg = format!("{}", err);
        assert!(msg.contains("add nsw i64"));
        assert!(msg.contains("operand"));
    }

    #[test]
    fn test_llvm_error_unknown_alloc() {
        let err = LlvmError::UnknownAllocTarget {
            target: "QuantumGravityZone".into(),
            binding: "x".into(),
            span: None,
        };
        let msg = format!("{}", err);
        assert!(msg.contains("QuantumGravityZone"));
    }

    #[test]
    fn test_circt_error_address_not_mapped() {
        let err = CirctError::AddressNotMapped {
            address: 0x4000_0000,
            device: "xc7z020".into(),
            available_range: "0x0000-0x3FFF".into(),
            span: None,
        };
        let msg = format!("{}", err);
        assert!(msg.contains("40000000"));
        assert!(msg.contains("xc7z020"));
    }

    #[test]
    fn test_webstack_error_emit_failed() {
        let err = WebstackError::WasmEmitFailed {
            reason: "unsupported opcode".into(),
            span: None,
        };
        let msg = format!("{}", err);
        assert!(msg.contains("WASM"));
        assert!(msg.contains("unsupported"));
    }

    #[test]
    fn test_backend_error_variants() {
        let llvm = BackendError::Llvm(LlvmError::MissingArenaPointer {
            binding: "buf".into(),
            span: None,
        });
        assert!(format!("{}", llvm).contains("buf"));

        let circt = BackendError::Circt(CirctError::UnknownAlloc {
            strategy: "BRAM".into(),
            binding: "x".into(),
            span: None,
        });
        assert!(format!("{}", circt).contains("BRAM"));

        let web = BackendError::Webstack(WebstackError::WasmEmitFailed {
            reason: "timeout".into(),
            span: None,
        });
        assert!(format!("{}", web).contains("timeout"));
    }

    #[test]
    fn test_compiler_error_wraps_errors() {
        let alloc = AllocError::Escape {
            name: "x".into(),
            span: None,
        };
        let ce: CompilerError = alloc.into();
        assert!(matches!(ce, CompilerError::Alloc(..)));
        assert!(format!("{}", ce).contains("x"));

        let derive = DeriveError::NoExamples {
            function: "f".into(),
        };
        let ce: CompilerError = derive.into();
        assert!(matches!(ce, CompilerError::Derive(..)));

        let llvm = LlvmError::MissingArenaPointer {
            binding: "b".into(),
            span: None,
        };
        let ce: CompilerError = llvm.into();
        assert!(matches!(ce, CompilerError::Backend(BackendError::Llvm(..))));
    }

    #[test]
    fn test_compiler_error_display() {
        let cases: Vec<(CompilerError, &str)> = vec![
            (CompilerError::InvalidPath, "invalid file path"),
            (
                CompilerError::MalformedFilename("bad".into()),
                "malformed filename",
            ),
            (CompilerError::Io("disk full".into()), "I/O error"),
        ];
        for (err, substr) in cases {
            assert!(format!("{}", err).contains(substr), "failed for variant");
        }
    }
}
