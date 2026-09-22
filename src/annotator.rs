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

use crate::ast::*;
use std::collections::{HashMap, HashSet};

pub struct Annotator {
    pub call_paths: HashMap<String, Vec<String>>,
}

impl Annotator {
    pub fn new() -> Self {
        Annotator {
            call_paths: HashMap::new(),
        }
    }

    pub fn analyze(&mut self, items: &[TopLevel]) {
        for item in items {
            if let TopLevel::Definition(defn) = item {
                let mut calls = Vec::new();
                self.collect_calls_from_body(&defn.body, &mut calls);
                self.call_paths.insert(defn.name.clone(), calls);
            }
            if let TopLevel::Transaction(txn) = item {
                let mut calls = Vec::new();
                self.collect_calls_from_body(&txn.body, &mut calls);
                self.call_paths.insert(txn.name.clone(), calls);
            }
        }
    }

    fn collect_calls_from_body(&self, body: &[Statement], calls: &mut Vec<String>) {
        for stmt in body {
            match stmt {
                Statement::Yield => {}
                Statement::Check(_) => {}
                Statement::Expression(expr) => self.collect_calls_from_expr(expr, calls),
                Statement::Assign(lhs, expr) => {
                    self.collect_calls_from_expr(lhs, calls);
                    self.collect_calls_from_expr(expr, calls);
                }
                Statement::ArrowAssign { target, value, .. } => {
                    if let Some(t) = target {
                        self.collect_calls_from_expr(t, calls);
                    }
                    self.collect_calls_from_expr(value, calls);
                }
                Statement::FreeHint(_) | Statement::KeepHint(_) => {}
                Statement::Guarded(condition, statements) => {
                    self.collect_calls_from_expr(condition, calls);
                    self.collect_calls_from_body(statements, calls);
                }
                Statement::Gate(cond) => self.collect_calls_from_expr(cond, calls),
                Statement::Trap | Statement::Halt => {}
                Statement::Break => {}
                Statement::Term(val) => {
                    if let Some(v) = val {
                        self.collect_calls_from_expr(v, calls);
                    }
                }
                Statement::EndProgram(val) => {
                    if let Some(v) = val {
                        self.collect_calls_from_expr(v, calls);
                    }
                }
                Statement::Rollback(val) => {
                    if let Some(v) = val {
                        self.collect_calls_from_expr(v, calls);
                    }
                }
                Statement::Let { expr, .. } => {
                    if let Some(v) = expr {
                        self.collect_calls_from_expr(v, calls);
                    }
                }
                Statement::Block(stmts) => {
                    self.collect_calls_from_body(stmts, calls);
                }
                Statement::Foreach { list, body, .. } => {
                    self.collect_calls_from_expr(list, calls);
                    self.collect_calls_from_body(body, calls);
                }
                Statement::InlineAsm { .. }
                | Statement::MetadataAssignment(..)
                | Statement::SyncBlock(..)
                | Statement::TrgBinding { .. }
                | Statement::InlineDefn(_)
                | Statement::InlineTxn(_)
                | Statement::Match { .. } => {}
                // 2026-08-09 (Phase 10): defer/mutex bodies may call
                // functions — collect them.
                Statement::Defer(body) | Statement::Mutex(body) => {
                    self.collect_calls_from_body(body, calls);
                }
            }
        }
    }

    fn collect_calls_from_expr(&self, expr: &Expr, calls: &mut Vec<String>) {
        match expr {
            Expr::Exists(_) => {},
            Expr::BeginProgram => {},
            Expr::Slice { array, start, end, stride } => {
                self.collect_calls_from_expr(array, calls);
                if let Some(e) = start.as_deref() { self.collect_calls_from_expr(e, calls); }
                if let Some(e) = end.as_deref() { self.collect_calls_from_expr(e, calls); }
                if let Some(e) = stride.as_deref() { self.collect_calls_from_expr(e, calls); }
            }
            Expr::Range { start, end, inclusive: _ } => {
                self.collect_calls_from_expr(start, calls);
                self.collect_calls_from_expr(end, calls);
            }
            Expr::Call(name, args, _) => {
                calls.push(name.clone());
                for arg in args {
                    self.collect_calls_from_expr(arg, calls);
                }
            }
            Expr::Spawn { args, .. } => {
                for arg in args {
                    self.collect_calls_from_expr(arg, calls);
                }
            }
            Expr::BinaryOp(_, l, r) => {
                self.collect_calls_from_expr(l, calls);
                self.collect_calls_from_expr(r, calls);
            }
            Expr::UnaryOp(_, e) => self.collect_calls_from_expr(e, calls),
            Expr::List(elems) => {
                for e in elems {
                    self.collect_calls_from_expr(e, calls);
                }
            }
            Expr::Index(list, index) => {
                self.collect_calls_from_expr(list, calls);
                self.collect_calls_from_expr(index, calls);
            }
            Expr::Deref(inner) | Expr::AddrOf(inner) | Expr::Consume(inner) | Expr::Await(inner) => self.collect_calls_from_expr(inner, calls),
            Expr::Field(obj, _) => self.collect_calls_from_expr(obj, calls),
            Expr::Tuple(elems) => {
                for e in elems {
                    self.collect_calls_from_expr(e, calls);
                }
            }
            Expr::If(cond, then, else_) => {
                self.collect_calls_from_expr(cond, calls);
                self.collect_calls_from_expr(then, calls);
                if let Some(el) = else_ {
                    self.collect_calls_from_expr(el, calls);
                }
            }
            Expr::Match(expr, arms) => {
                self.collect_calls_from_expr(expr, calls);
                for arm in arms {
                    self.collect_calls_from_expr(&arm.body, calls);
                    if let Some(g) = &arm.guard {
                        self.collect_calls_from_expr(g, calls);
                    }
                }
            }
            Expr::Block(stmts) => {
                self.collect_calls_from_body(stmts, calls);
            }
            Expr::Lambda(_, body) => {
                self.collect_calls_from_expr(body, calls);
            }
            Expr::Cast(expr, _) => self.collect_calls_from_expr(expr, calls),
            Expr::IsType(expr, _) => self.collect_calls_from_expr(expr, calls),
            Expr::Within(expr, scope) => {
                self.collect_calls_from_expr(expr, calls);
                self.collect_calls_from_expr(scope, calls);
            }
            Expr::DerivationBlock(block) => {
                for ex in &block.examples {
                    for input in &ex.inputs {
                        self.collect_calls_from_expr(input, calls);
                    }
                    self.collect_calls_from_expr(&ex.output, calls);
                }
            }
            Expr::FormattingAnnotation(..)
            | Expr::Quoted(..) | Expr::TaggedQuotedLiteral(..)
            | Expr::Decimal(..) | Expr::TaggedLiteral(..) | Expr::Char(..)
            | Expr::Bool(..)
            | Expr::Float(..)
            | Expr::Identifier(..) => {}
            Expr::Field(recv, _) | Expr::Reflect(recv, _, _) => {
                self.collect_calls_from_expr(recv, calls);
            }
            Expr::MethodCall(recv, _, args, _, _) => {
                self.collect_calls_from_expr(recv, calls);
                for a in args {
                    self.collect_calls_from_expr(a, calls);
                }
            }
            Expr::StructLiteral { .. } => {}
            Expr::PluginIntercept { args, .. } => {
                for a in args {
                    self.collect_calls_from_expr(a, calls);
                }
            }
            Expr::Named { inner, .. } => {
                self.collect_calls_from_expr(inner, calls);
            }
            Expr::UnitLiteral { .. } => {}
            Expr::Capture { expr, .. } => {
                self.collect_calls_from_expr(expr, calls);
            }
        }
    }

    pub fn annotate_program(&self, items: &[TopLevel]) -> String {
        let mut output = String::new();
        for item in items {
            match item {
                TopLevel::Definition(defn) => output.push_str(&self.format_definition(defn)),
                TopLevel::Transaction(txn) => output.push_str(&self.format_transaction(txn)),
                TopLevel::Signature(sig) => output.push_str(&self.format_signature(sig)),
                TopLevel::StateDecl(decl) => output.push_str(&self.format_state_decl(decl)),
                _ => {}
            }
        }
        output
    }

    fn type_to_string(&self, ty: &Type) -> String {
        match ty {
            Type::Dyn(inner) => format!("dyn {}", inner),
            Type::Task(inner) => format!("Task<{}>", inner),
            Type::Number(n) => n.to_string(),
            Type::Custom(__t) if __t == "Int" => "Int".to_string(),
            Type::Custom(__t) if __t == "Int8" => "Int8".to_string(),
            Type::Custom(__t) if __t == "Int16" => "Int16".to_string(),
            Type::Custom(__t) if __t == "Int32" => "Int32".to_string(),
            Type::Custom(__t) if __t == "Float" => "Float".to_string(),
            Type::Custom(__t) if __t == "Float64" => "Float64".to_string(),
            Type::Custom(__t) if __t == "String" => "String".to_string(),
            Type::Custom(__t) if __t == "Bool" => "Bool".to_string(),
            Type::Custom(__t) if __t == "Blob" => "Blob".to_string(),
            Type::Custom(__t) if __t == "UInt" => "UInt".to_string(),
            Type::Custom(__t) if __t == "UInt8" => "UInt8".to_string(),
            Type::Custom(__t) if __t == "UInt16" => "UInt16".to_string(),
            Type::Custom(__t) if __t == "UInt32" => "UInt32".to_string(),
            Type::Custom(__t) if __t == "Char" => "Char".to_string(),
            Type::Void => "Void".to_string(),
            Type::Bits(width) => format!("Bit<{}>", width),
            Type::Width(n) => format!("Width({})", n),
            Type::Custom(name) => name.clone(),
            Type::TypeVar(name) => name.clone(),
            Type::Union(types) => types
                .iter()
                .map(|t| self.type_to_string(t))
                .collect::<Vec<_>>()
                .join(" | "),
            Type::Tuple(types) => format!("({})", types
                .iter()
                .map(|t| self.type_to_string(t))
                .collect::<Vec<_>>()
                .join(", ")),
            Type::Generic(name, type_args) => {
                format!(
                    "{}<{}>",
                    name,
                    type_args
                        .iter()
                        .map(|t| self.type_to_string(t))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            Type::Applied(name, type_args) => {
                format!(
                    "{}<{}>",
                    name,
                    type_args
                        .iter()
                        .map(|t| self.type_to_string(t))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            Type::Vector(inner, dims) => {
                let dims_str: Vec<String> = dims.iter().map(|d| match d {
                    Dimension::Anonymous(s) => format!("{}", s),
                    Dimension::Named(n, s) => format!("{}:{}", n, s),
                }).collect();
                format!("Vector<{}, {}>", self.type_to_string(inner), dims_str.join(", "))
            }
            Type::Constrained(inner, _) => self.type_to_string(inner),
            Type::LayoutPtr(lc) => format!("Ptr<Bits @/0..{}>", lc.bytes * 8 - 1),
            Type::Ptr(inner) => format!("Ptr<{}>", self.type_to_string(inner)),
            Type::PtrConst(inner) => format!("Ptr<const {}>", self.type_to_string(inner)),
            Type::Function(params, ret) => {
                let params_str: Vec<String> = params.iter().map(|t| self.type_to_string(t)).collect();
                format!("({}) -> {}", params_str.join(", "), self.type_to_string(ret))
            }
        }
    }

    fn format_definition(&self, defn: &Definition) -> String {
        let params: Vec<String> = defn
            .parameters
            .iter()
            .map(|(n, t)| format!("{}: {}", n, self.type_to_string(t)))
            .collect();

        let params_str = if params.is_empty() {
            "()".to_string()
        } else {
            format!("({})", params.join(", "))
        };
        let outputs_str = if defn.outputs.is_empty() {
            String::new()
        } else {
            let outputs: Vec<String> = defn
                .outputs
                .iter()
                .map(|t| self.type_to_string(t))
                .collect();
            format!(": {}", outputs.join(", "))
        };

        let pre = self.format_expr(&defn.contract.pre_condition);
        let post = self.format_expr(&defn.contract.post_condition);

        let body = self.format_body(&defn.body);

        format!(
            "defn {}{}{} [{}][{}] {{\n{}}};\n",
            defn.name, params_str, outputs_str, pre, post, body
        )
    }

    fn format_transaction(&self, txn: &Transaction) -> String {
        let modifier = if txn.is_async { "async " } else { "" };
        let rct = if txn.is_reactive { "rct " } else { "" };

        let pre = self.format_expr(&txn.contract.pre_condition);
        let post = self.format_expr(&txn.contract.post_condition);

        let body = self.format_body(&txn.body);

        format!(
            "{}txn {}{} [{}][{}] {{\n{}}};\n",
            rct, modifier, txn.name, pre, post, body
        )
    }

    fn format_signature(&self, sig: &Signature) -> String {
        let inputs: Vec<String> = sig
            .params
            .iter()
            .map(|(_, t)| self.type_to_string(t))
            .collect();
        let results: Vec<String> = sig
            .outputs
            .iter()
            .map(|t| self.type_to_string(t))
            .collect();

        format!(
            "sig {}: ({}) -> ({});\n",
            sig.name,
            inputs.join(", "),
            results.join(", ")
        )
    }

    fn format_state_decl(&self, decl: &StateDecl) -> String {
        format!(
            "let {}: {};\n",
            decl.name,
            self.type_to_string(&decl.ty),
        )
    }

    fn format_body(&self, body: &[Statement]) -> String {
        let mut output = String::new();
        for stmt in body {
            output.push_str(&self.format_statement(stmt, 2));
        }
        output
    }

    fn format_statement(&self, stmt: &Statement, indent: usize) -> String {
        let spaces = " ".repeat(indent);
        match stmt {
            Statement::Yield => format!("{}yield;\n", spaces),
            Statement::Check(e) => format!("{}check {};\n", spaces, self.format_expr(e)),
            Statement::Expression(expr) => format!("{}{};\n", spaces, self.format_expr(expr)),
            Statement::Assign(lhs, expr) => {
                format!(
                    "{}{} = {};\n",
                    spaces,
                    self.format_expr(lhs),
                    self.format_expr(expr),
                )
            }
            Statement::ArrowAssign { target, value, consume } => {
                match target {
                    Some(t) => format!("{}{} {} {}{};\n", spaces, self.format_expr(t), if *consume { "~<-" } else { "<-" }, self.format_expr(value), ""),
                    None => format!("{}{} {};\n", spaces, if *consume { "~<-" } else { "<-" }, self.format_expr(value)),
                }
            }
            Statement::FreeHint(name) => format!("{}free {};\n", spaces, name),
            Statement::KeepHint(name) => format!("{}keep {};\n", spaces, name),
            Statement::Guarded(condition, statements) => {
                let mut output = format!("{}when {} {{\n", spaces, self.format_expr(condition));
                for s in statements {
                    output.push_str(&self.format_statement(s, indent + 2));
                }
                output.push_str(&format!("{}}}\n", spaces));
                output
            }
            Statement::Gate(cond) => {
                format!("{}[{}];\n", spaces, self.format_expr(cond))
            }
            Statement::Term(val) => {
                let val_str = val.as_ref().map(|v| self.format_expr(v)).unwrap_or_default();
                format!("{}term {};\n", spaces, val_str)
            }
            Statement::EndProgram(val) => {
                let val_str = val.as_ref().map(|v| self.format_expr(v)).unwrap_or_default();
                format!("{}term! {};\n", spaces, val_str)
            }
            Statement::Trap => format!("{}trap;\n", spaces),
            Statement::Halt => format!("{}halt;\n", spaces),
            Statement::Break => format!("{}break;\n", spaces),
            Statement::Rollback(val) => {
                let val_str = val
                    .as_ref()
                    .map(|e| format!(" {}", self.format_expr(e)))
                    .unwrap_or_default();
                format!("{}escape{};\n", spaces, val_str)
            }
            Statement::Let { name, ty, expr, .. } => {
                if let Some(t) = ty {
                    if let Some(e) = expr {
                        format!("{}let {}: {} = {};\n", spaces, name, self.type_to_string(t), self.format_expr(e))
                    } else {
                        format!("{}let {}: {};\n", spaces, name, self.type_to_string(t))
                    }
                } else if let Some(e) = expr {
                    format!("{}let {} = {};\n", spaces, name, self.format_expr(e))
                } else {
                    format!("{}let {};\n", spaces, name)
                }
            }
            Statement::Block(stmts) => {
                let mut output = format!("{} {{\n", spaces);
                for s in stmts {
                    output.push_str(&self.format_statement(s, indent + 2));
                }
                output.push_str(&format!("{}}}\n", spaces));
                output
            }
            Statement::Foreach { item, list, body } => {
                let mut output = format!("{}foreach({} in {}) {{\n", spaces, item, self.format_expr(list));
                for s in body {
                    output.push_str(&self.format_statement(s, indent + 2));
                }
                output.push_str(&format!("{}}}\n", spaces));
                output
            }
            Statement::SyncBlock(body) => {
                let mut output = format!("{}sync {{\n", spaces);
                for s in body {
                    output.push_str(&self.format_statement(s, indent + 2));
                }
                output.push_str(&format!("{}}}\n", spaces));
                output
            }
            Statement::Defer(body) => {
                let mut output = format!("{}defer {{\n", spaces);
                for s in body {
                    output.push_str(&self.format_statement(s, indent + 2));
                }
                output.push_str(&format!("{}}}\n", spaces));
                output
            }
            Statement::Mutex(body) => {
                let mut output = format!("{}mutex {{\n", spaces);
                for s in body {
                    output.push_str(&self.format_statement(s, indent + 2));
                }
                output.push_str(&format!("{}}}\n", spaces));
                output
            }
            Statement::MetadataAssignment(key, val) => {
                format!("{}{} <~ {:?};\n", spaces, key, val)
            }
            Statement::TrgBinding { name, instance } => {
                format!("{}trg {} @ {};\n", spaces, name, self.format_expr(instance))
            }
            Statement::InlineAsm { asm_string, .. } => {
                format!("{}asm \"{}\";\n", spaces, asm_string)
            }
            Statement::InlineDefn(d) => {
                format!("{}// compile-time defn {}\n", spaces, d.name)
            }
            Statement::InlineTxn(t) => {
                format!("{}// compile-time txn {}\n", spaces, t.name)
            }
            Statement::Match { .. } => {
                format!("{}// compile-time match\n", spaces)
            }
        }
    }

    fn format_expr(&self, expr: &Expr) -> String {
        match expr {
            Expr::Char(c) => format!("'{}'", c),
            Expr::Consume(inner) => format!("~{}", self.format_expr(inner)),
            Expr::Await(inner) => format!("await {}", self.format_expr(inner)),
            Expr::BeginProgram => "beginprogram".to_string(),
            Expr::Decimal(n) | Expr::TaggedLiteral(n, _) => n.to_string(),
            Expr::Float(f) => f.to_string(),
            Expr::Quoted(s) | Expr::TaggedQuotedLiteral(s, _) => format!("\"{}\"", String::from_utf8_lossy(s)),
            Expr::Bool(true) => "true".to_string(),
            Expr::Bool(false) => "false".to_string(),
            Expr::Identifier(n) => n.clone(),
            Expr::Call(name, args, _) => {
                let args_str = args
                    .iter()
                    .map(|a| self.format_expr(a))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}({})", name, args_str)
            }
            Expr::Spawn { type_name, args, storage } => {
                let args_str = args
                    .iter()
                    .map(|a| self.format_expr(a))
                    .collect::<Vec<_>>()
                    .join(", ");
                if storage.keyword().is_empty() {
                    format!("spawn {}({})", type_name, args_str)
                } else {
                    format!("{} spawn {}({})", storage.keyword(), type_name, args_str)
                }
            }
            Expr::BinaryOp(kind, l, r) => {
                let op_str = match kind {
                    BinaryOpKind::Add => " + ",
                    BinaryOpKind::Sub => " - ",
                    BinaryOpKind::Mul => " * ",
                    BinaryOpKind::Div => " / ",
                    BinaryOpKind::Mod => " % ",
                    BinaryOpKind::Eq => " == ",
                    BinaryOpKind::Neq => " != ",
                    BinaryOpKind::Lt => " < ",
                    BinaryOpKind::Le => " <= ",
                    BinaryOpKind::Gt => " > ",
                    BinaryOpKind::Ge => " >= ",
                    BinaryOpKind::And => " && ",
                    BinaryOpKind::Or => " || ",
                    BinaryOpKind::BitAnd => " & ",
                    BinaryOpKind::BitOr => " | ",
                    BinaryOpKind::BitXor => " ^ ",
                    BinaryOpKind::Shl => " << ",
                    BinaryOpKind::Shr => " >> ",
                    BinaryOpKind::Concat => " ++ ",
                };
                format!("({}{}{})", self.format_expr(l), op_str, self.format_expr(r))
            }
            Expr::UnaryOp(kind, e) => {
                let op_str = match kind {
                    UnaryOpKind::Neg => "-",
                    UnaryOpKind::Not => "!",
                    UnaryOpKind::BitNot => "~",
                };
                format!("{}{}", op_str, self.format_expr(e))
            }
            Expr::List(elements) => {
                let elements_str = elements
                    .iter()
                    .map(|e| self.format_expr(e))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{}]", elements_str)
            }
            Expr::Field(obj, field) => {
                format!("{}.{}", self.format_expr(obj), field)
            }
            Expr::Index(list, index) => {
                format!("{}[{}]", self.format_expr(list), self.format_expr(index))
            }
            Expr::Tuple(exprs) => {
                let exprs_str = exprs.iter().map(|e| self.format_expr(e)).collect::<Vec<_>>().join(", ");
                format!("({})", exprs_str)
            }
            Expr::If(cond, then, else_) => {
                let else_str = else_.as_ref().map(|e| format!(" else {}", self.format_expr(e))).unwrap_or_default();
                format!("(if {} then {}{})", self.format_expr(cond), self.format_expr(then), else_str)
            }
            Expr::Match(expr, arms) => {
                let arms_str: Vec<String> = arms.iter().map(|arm| {
                    format!("{} => {}", arm.pattern, self.format_expr(&arm.body))
                }).collect();
                format!("(match {} {{ {} }})", self.format_expr(expr), arms_str.join(", "))
            }
            Expr::Block(stmts) => {
                let body_str: String = stmts.iter().map(|s| self.format_statement(s, 0)).collect();
                format!("{{ {} }}", body_str.trim())
            }
            Expr::Lambda(params, body) => {
                let params_str = params.join(", ");
                format!("({}) => {}", params_str, self.format_expr(body))
            }
            Expr::Cast(expr, ty) => format!("({} as {})", self.format_expr(expr), self.type_to_string(ty)),
            Expr::IsType(expr, ty) => format!("({} is {})", self.format_expr(expr), self.type_to_string(ty)),
            Expr::Within(expr, scope) => format!("({} within {})", self.format_expr(expr), self.format_expr(scope)),
            Expr::DerivationBlock(block) => {
                let examples_str: Vec<String> = block.examples.iter().map(|ex| {
                    let inputs_str: Vec<String> = ex.inputs.iter().map(|i| self.format_expr(i)).collect();
                    format!("{} -> {}", inputs_str.join(", "), self.format_expr(&ex.output))
                }).collect();
                format!(":= {{ {} }}", examples_str.join("; "))
            }
            Expr::Deref(inner) => format!("*{}", self.format_expr(inner)),
            Expr::AddrOf(inner) => format!("&{}", self.format_expr(inner)),
            Expr::Field(recv, name) => format!("{}.{}", self.format_expr(recv), name),
            Expr::Reflect(recv, name, kind) => match kind {
                ReflectKind::Runtime => format!("{}.^{}", self.format_expr(recv), name),
                ReflectKind::CompileTime => format!("{}.^^{}", self.format_expr(recv), name),
            },
            Expr::MethodCall(recv, name, args, _, _) => {
                let args_str: Vec<String> = args.iter().map(|a| self.format_expr(a)).collect();
                format!("{}.{}({})", self.format_expr(recv), name, args_str.join(", "))
            }
            Expr::FormattingAnnotation(f) => format!("formatting <~ {}", f.name()),
            Expr::PluginIntercept { name, args, .. } => {
                let args_str: Vec<String> = args.iter().map(|a| self.format_expr(a)).collect();
                format!("{}!({})", name, args_str.join(", "))
            }
            Expr::StructLiteral { type_name, .. } => format!("{} {{ ... }}", type_name),
            Expr::Exists(_) => { unreachable!("fn? only in stage eval") },
            Expr::Slice { array, start, end, stride } => {
                let mut s = format!("{}[", self.format_expr(array));
                if let Some(v) = start { s.push_str(&self.format_expr(v)); }
                s.push_str(":");
                if let Some(v) = end { s.push_str(&self.format_expr(v)); }
                if let Some(v) = stride { s.push_str(&format!(":{}", self.format_expr(v))); }
                s.push_str("]");
                s
            }
            Expr::Range { start, end, inclusive } => format!(
                "{}..{}{}",
                self.format_expr(start),
                if *inclusive { "=" } else { "" },
                self.format_expr(end)
            ),
            Expr::Named { name, inner } => {
                format!("net {}: {}", name, self.format_expr(inner))
            }
            Expr::UnitLiteral { value, unit } => {
                format!("{}{}", value, unit)
            }
            Expr::Capture { expr, name } => {
                format!("({}) >> {}", self.format_expr(expr), name)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::ast::*;
    use super::*;

    fn make_defn(name: &str, body: Vec<Statement>) -> TopLevel {
        TopLevel::Definition(Definition {
            name: name.to_string(),
            type_params: vec![],
            parameters: vec![],
            outputs: vec![],
            output_type: None,
            contract: Contract::new(Expr::Bool(true), Expr::Bool(true)),
            body,
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            annotations: vec![],
            span: None,
            doc: None,
        })
    }

    fn make_call_expr(name: &str) -> Expr {
        Expr::Call(name.to_string(), vec![], None)
    }

    #[test]
    fn test_analyze_empty_program() {
        let mut ann = Annotator::new();
        let items: Vec<TopLevel> = vec![];
        ann.analyze(&items);
        assert!(ann.call_paths.is_empty());
    }

    #[test]
    fn test_analyze_definition_no_calls() {
        let mut ann = Annotator::new();
        let stmts = vec![Statement::Term(Some(Expr::Decimal(42)))];
        let items = vec![make_defn("foo", stmts)];
        ann.analyze(&items);
        assert_eq!(ann.call_paths.get("foo").map(|v| v.len()).unwrap_or(0), 0);
    }

    #[test]
    fn test_analyze_definition_with_call() {
        let mut ann = Annotator::new();
        let stmts = vec![Statement::Expression(make_call_expr("bar"))];
        let items = vec![make_defn("foo", stmts)];
        ann.analyze(&items);
        let calls = ann.call_paths.get("foo").unwrap();
        assert_eq!(calls, &vec!["bar".to_string()]);
    }

    #[test]
    fn test_analyze_nested_call() {
        let mut ann = Annotator::new();
        let inner = make_call_expr("inner");
        let outer = Expr::Call("outer".to_string(), vec![inner], None);
        let stmts = vec![Statement::Expression(outer)];
        let items = vec![make_defn("foo", stmts)];
        ann.analyze(&items);
        let calls = ann.call_paths.get("foo").unwrap();
        assert_eq!(calls, &vec!["outer".to_string(), "inner".to_string()]);
    }

    #[test]
    fn test_analyze_guarded_call() {
        let mut ann = Annotator::new();
        let guarded = Statement::Guarded(
            Expr::Bool(true),
            vec![Statement::Expression(make_call_expr("inside_guard"))],
        );
        let items = vec![make_defn("foo", vec![guarded])];
        ann.analyze(&items);
        let calls = ann.call_paths.get("foo").unwrap();
        assert_eq!(calls, &vec!["inside_guard".to_string()]);
    }

    #[test]
    fn test_analyze_guarded_assignment_call() {
        let mut ann = Annotator::new();
        let guarded = Statement::Guarded(
            Expr::Bool(true),
            vec![Statement::Assign(
                Expr::Identifier("x".to_string()),
                make_call_expr("calc"),
            )],
        );
        let items = vec![make_defn("foo", vec![guarded])];
        ann.analyze(&items);
        let calls = ann.call_paths.get("foo").unwrap();
        assert_eq!(calls, &vec!["calc".to_string()]);
    }
}

#[cfg(all(feature = "kani", feature = "kani_full"))]
mod kani_full_tests {
    use super::*;

    #[kani::proof]
    fn verify_annotator_format_expr_literal_integer() {
        let ann = Annotator::new();
        let expr = Expr::Decimal(42);
        let result = ann.format_expr(&expr);
        assert_eq!(result, "42");
    }

    #[kani::proof]
    fn verify_annotator_format_expr_literal_bool() {
        let ann = Annotator::new();
        let expr = Expr::Bool(true);
        let result = ann.format_expr(&expr);
        assert_eq!(result, "true");
    }

    #[kani::proof]
    fn verify_annotator_format_expr_literal_float() {
        let ann = Annotator::new();
        let expr = Expr::Float(3.14);
        let result = ann.format_expr(&expr);
        assert!(!result.is_empty());
    }

    #[kani::proof]
    fn verify_annotator_format_expr_literal_string() {
        let ann = Annotator::new();
        let expr = Expr::Quoted("hello".into());
        let result = ann.format_expr(&expr);
        assert_eq!(result, "\"hello\"");
    }
}
