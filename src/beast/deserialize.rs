// ── BEAST Deserializer ───────────────────────────────────────────────────
// 2026-07-14: Read .beast S-expression text → Vec<TopLevel> + TypeUniverse.
// Every function is max 2 levels. Extract helpers.

use std::collections::HashMap;
use crate::ast::*;
use crate::type_universe::{ResolvedType, TypeUniverse};
use super::sexpr::{Atom, SExpr};

/// Deserialize BEAST text back into a compiled program.
pub fn from_beast(text: &str) -> Result<(Vec<TopLevel>, TypeUniverse), String> {
    let tokens = super::sexpr::tokenize(text)?;
    let expr = super::sexpr::parse(&tokens)?;
    let list = match expr {
        SExpr::List(ref children) => children,
        _ => return Err("expected top-level list".into()),
    };
    let mut universe = TypeUniverse::new();
    let mut items = Vec::new();
    for child in list {
        match child {
            SExpr::List(parts) => {
                if parts.is_empty() { continue; }
                let tag = match sexpr_str(&parts[0]) { Ok(t) => t, Err(_) => continue };
                match tag {
                    "universe" => { let rt = parse_universe(parts)?; universe.types.insert(rt.name.clone(), rt); }
                    "typedef" => { items.push(TopLevel::TypeDef(parse_typedef(parts)?)); }
                    "defn" => { items.push(TopLevel::Definition(parse_definition(parts)?)); }
                    "txn" => { items.push(TopLevel::Transaction(parse_transaction(parts)?)); }
                    "state" => { items.push(parse_statedecl(parts)?); }
                    "trigger" => { items.push(parse_trigger(parts)?); }
                    "constant" => { items.push(parse_constant(parts)?); }
                    "init" => { items.push(parse_init(parts)?); }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    Ok((items, universe))
}

fn sexpr_str(expr: &SExpr) -> Result<&str, String> {
    match expr {
        SExpr::Atom(Atom::String(s)) => Ok(s.as_str()),
        _ => Err("expected string atom".into()),
    }
}

fn sexpr_int(expr: &SExpr) -> Result<i64, String> {
    match expr {
        SExpr::Atom(Atom::Int(n)) => Ok(*n),
        _ => Err("expected int atom".into()),
    }
}

fn tag(parts: &[SExpr], i: usize) -> Result<&str, String> {
    sexpr_str(parts.get(i).ok_or_else(|| format!("missing tag at index {}", i))?)
}

fn assert_tag(parts: &[SExpr], i: usize, expected: &str) -> Result<(), String> {
    let t = tag(parts, i)?;
    if t != expected { return Err(format!("expected '{}' at index {}, got '{}'", expected, i, t)); }
    Ok(())
}

// 2026-07-15: Extract tag from a child node that may be a bare atom (":reactive")
// or a tagged list like ("params" ...). Returns the tag string.
fn child_tag(expr: &SExpr) -> Result<&str, String> {
    match expr {
        SExpr::Atom(Atom::String(s)) => Ok(s.as_str()),
        SExpr::List(children) => {
            if let Some(first) = children.first() {
                sexpr_str(first)
            } else {
                Err("empty list — expected tagged child".into())
            }
        }
        _ => Err("expected string atom or tagged list".into()),
    }
}

fn parse_universe(parts: &[SExpr]) -> Result<ResolvedType, String> {
    let name = tag(parts, 1)?.to_string();
    let mut rt = ResolvedType {
        name: name.clone(),
        base: "Data".into(),
        bytes: 8,
        min_bits: 0,
        max_bits: 0,
        alignment: 8,
        properties: HashMap::new(),
        fields: vec![],
    };
    let mut i = 2;
    while i < parts.len() {
        let key = tag(parts, i)?;
        match key {
            "maxbits" => { rt.bytes = sexpr_int(&parts[i + 1])? as u64 / 8; i += 2; }
            "alignment" => { rt.alignment = sexpr_int(&parts[i + 1])? as u64; i += 2; }
            "properties" => {
                let mut j = i + 1;
                while j < parts.len() {
                    match &parts[j] {
                        SExpr::List(pair) if pair.len() == 2 => {
                            let k = sexpr_str(&pair[0])?.to_string();
                            let v = sexpr_to_pv(&pair[1])?;
                            rt.properties.insert(k, v);
                        }
                        _ => break,
                    }
                    j += 1;
                }
                i = j;
            }
            _ => { i += 1; }
        }
    }
    Ok(rt)
}

fn parse_typedef(parts: &[SExpr]) -> Result<Box<TypeDef>, String> {
    let name = tag(parts, 1)?.to_string();
    let mut slots = Vec::new();
    let mut metadata = HashMap::new();
    let mut i = 2;
    while i < parts.len() {
        let key = tag(parts, i)?;
        match key {
            "base" => { i += 2; }
            "slots" => {
                let mut j = i + 1;
                while j < parts.len() {
                    if let SExpr::List(slot_parts) = &parts[j] {
                        if slot_parts.len() >= 3 && sexpr_str(&slot_parts[0])? == "slot" {
                            let sn = sexpr_str(&slot_parts[1])?.to_string();
                            let st = parse_type(&slot_parts[2])?;
                            slots.push(TypeDefSlot { name: sn, ty: st, bit_range: None });
                        }
                    } else { break; }
                    j += 1;
                }
                i = j;
            }
            "metadata" => {
                if let SExpr::List(pair) = &parts[i + 1] {
                    if pair.len() == 2 {
                        let k = sexpr_str(&pair[0])?.to_string();
                        let v = sexpr_to_pv(&pair[1])?;
                        metadata.insert(k, v);
                    }
                }
                i += 2;
            }
            _ => { i += 1; }
        }
    }
    Ok(Box::new(TypeDef {
        name, type_params: vec![], parent: None, protocol: None,
        traits: vec![],
        bit_range: None, span: None, coll: false, seq: false,
        ports_in: Vec::new(),
        ports_out: Vec::new(),
        body: TypeDefBody { slots, pins: Vec::new(), metadata, projections: vec![], bindings: vec![],
            operators: vec![], op_bindings: vec![],
            constraints: vec![], members: vec![], span: None },
    }))
}

fn parse_definition(parts: &[SExpr]) -> Result<Definition, String> {
    let name = tag(parts, 1)?.to_string();
    let mut params = Vec::new();
    let mut outputs = Vec::new();
    let mut contract = Contract { pre_condition: Expr::Bool(true), post_condition: Expr::Bool(true),
        watchdog: None, span: None, explicit: false, post_authority: false };
    let mut body = Vec::new();
    let mut metadata = HashMap::new();
    let mut i = 2;
    // 2026-07-15: Use child_tag to handle both bare atoms and (tag ...) lists
    while i < parts.len() {
        let key = child_tag(&parts[i])?;
        match key {
            "params" => { params = parse_params(&parts[i])?; i += 1; }
            "outputs" => { outputs = parse_outputs(&parts[i])?; i += 1; }
            "contract" => { contract = parse_contract(&parts[i])?; i += 1; }
            "metadata" => {
                if let SExpr::List(pair) = &parts[i] {
                    if pair.len() >= 3 {
                        let k = sexpr_str(&pair[1])?.to_string();
                        let v = sexpr_to_pv(&pair[2])?;
                        metadata.insert(k, v);
                    }
                }
                i += 1;
            }
            "body" => { i += 1; }
            _ => {
                body.push(parse_statement(&parts[i])?);
                i += 1;
            }
        }
    }
    Ok(Definition { name, parameters: params, outputs, body, type_params: vec![],
        output_type: None, contract, metadata, annotations: vec![],
        derivation: None, modifiers: vec![], span: None, doc: None })
}

fn parse_transaction(parts: &[SExpr]) -> Result<Transaction, String> {
    let name = tag(parts, 1)?.to_string();
    let mut params = Vec::new();
    let mut contract = Contract { pre_condition: Expr::Bool(true), post_condition: Expr::Bool(true),
        watchdog: None, span: None, explicit: false, post_authority: false };
    let mut body = Vec::new();
    let mut metadata = HashMap::new();
    let mut is_reactive = false;
    let mut is_async = false;
    let mut i = 2;
    // 2026-07-15: Use child_tag to handle both bare atoms and (tag ...) lists
    while i < parts.len() {
        let key = child_tag(&parts[i])?;
        match key {
            ":reactive" => { is_reactive = true; i += 1; }
            ":async" => { is_async = true; i += 1; }
            "params" => { params = parse_params(&parts[i])?; i += 1; }
            "contract" => { contract = parse_contract(&parts[i])?; i += 1; }
            "metadata" => {
                if let SExpr::List(pair) = &parts[i] {
                    if pair.len() >= 3 {
                        let k = sexpr_str(&pair[1])?.to_string();
                        let v = sexpr_to_pv(&pair[2])?;
                        metadata.insert(k, v);
                    }
                }
                i += 1;
            }
            "body" => { i += 1; }
            _ => {
                body.push(parse_statement(&parts[i])?);
                i += 1;
            }
        }
    }
    Ok(Transaction { name, parameters: params, type_params: vec![], is_reactive, is_async,
        contract, body, outputs: vec![], output_type: None, metadata,
        derivation: None, modifiers: vec![], span: None, doc: None })
}

fn parse_contract(expr: &SExpr) -> Result<Contract, String> {
    let parts = match expr { SExpr::List(p) => p, _ => return Err("expected list for contract".into()) };
    let mut pre = Expr::Bool(true);
    let mut post = Expr::Bool(true);
    let mut post_authority = false;
    let mut i = 1;
    while i < parts.len() {
        let key = child_tag(&parts[i])?;
        match key {
            "pre" => {
                // (pre <expr>) — expr is at index 1 inside this sub-list
                let sub = match &parts[i] { SExpr::List(p) => p, _ => return Err("expected list for pre".into()) };
                pre = parse_expr(&sub[1])?;
                i += 1;
            }
            "post" => {
                let sub = match &parts[i] { SExpr::List(p) => p, _ => return Err("expected list for post".into()) };
                post = parse_expr(&sub[1])?;
                i += 1;
            }
            "post_authority" => {
                post_authority = true;
                i += 1;
            }
            _ => { i += 1; }
        }
    }
    Ok(Contract { pre_condition: pre, post_condition: post, watchdog: None, span: None, explicit: false, post_authority: false})
}

fn parse_statedecl(parts: &[SExpr]) -> Result<TopLevel, String> {
    let name = tag(parts, 1)?.to_string();
    let ty = parse_type(&parts[2])?;
    Ok(TopLevel::StateDecl(StateDecl { name, ty, span: None }))
}

fn parse_trigger(parts: &[SExpr]) -> Result<TopLevel, String> {
    let name = tag(parts, 1)?.to_string();
    let mut port = String::new();
    let mut i = 2;
    while i < parts.len() {
        let key = tag(parts, i)?;
        i += 1;
    }
    Ok(TopLevel::Trigger(Trigger { name, instance: Expr::Identifier("".into()), span: None }))
}

fn parse_constant(parts: &[SExpr]) -> Result<TopLevel, String> {
    let name = tag(parts, 1)?.to_string();
    let ty = parse_type(&parts[2])?;
    let expr = parse_expr(&parts[3])?;
    Ok(TopLevel::Constant(Constant { name, ty, expr, section: None }))
}

/// 2026-08-09: `(init NAME (:bound ...)? TYPE (=expr EXPR)? STMT*)`
fn parse_init(parts: &[SExpr]) -> Result<TopLevel, String> {
    let name = tag(parts, 1)?.to_string();
    let mut bound = None;
    let mut value = None;
    let mut body = Vec::new();
    let mut ty: Option<Type> = None;
    for i in 2..parts.len() {
        match classify_init_part(parts, i, ty.is_some())? {
            InitPart::Bound(b) => bound = Some(b),
            InitPart::Value(v) => value = Some(v),
            InitPart::Type(t) => ty = Some(t),
            InitPart::Body(s) => body.push(s),
        }
    }
    Ok(TopLevel::Init(crate::ast::InitDecl {
        name,
        bound,
        ty: ty.unwrap_or_else(Type::int),
        value,
        body,
        span: None,
        doc: None,
    }))
}

enum InitPart {
    Bound(crate::ast::BoundSpec),
    Value(Expr),
    Type(Type),
    Body(Statement),
}

/// Classify one trailing init decl part by its leading tag. The first untagged
/// part is the type (everything after is a body statement).
fn classify_init_part(parts: &[SExpr], i: usize, has_type: bool) -> Result<InitPart, String> {
    let t = child_tag(&parts[i])?;
    if t == ":bound" {
        let b = match parse_bound_field(parts, i)? {
            Some(b) => b,
            None => return Ok(InitPart::Body(parse_statement(&parts[i])?)),
        };
        return Ok(InitPart::Bound(b));
    }
    if t == "=expr" {
        let v = match parse_init_value(parts, i)? {
            Some(v) => v,
            None => return Ok(InitPart::Body(parse_statement(&parts[i])?)),
        };
        return Ok(InitPart::Value(v));
    }
    if has_type {
        return Ok(InitPart::Body(parse_statement(&parts[i])?));
    }
    Ok(InitPart::Type(parse_type(&parts[i])?))
}

/// Extract the `:bound` subtree of an init decl, if present at `parts[i]`.
fn parse_bound_field(parts: &[SExpr], i: usize) -> Result<Option<crate::ast::BoundSpec>, String> {
    let subtree = match &parts[i] {
        SExpr::List(l) => l,
        _ => return Ok(None),
    };
    parse_bound_nodes(subtree.get(1..).unwrap_or(&[]))
}

/// Extract the `(=expr EXPR)` value slot of an init decl, if present at `parts[i]`.
fn parse_init_value(parts: &[SExpr], i: usize) -> Result<Option<Expr>, String> {
    let p = match &parts[i] {
        SExpr::List(p) => p,
        _ => return Ok(None),
    };
    match p.get(1) {
        Some(e) => Ok(Some(parse_expr(e)?)),
        None => Ok(None),
    }
}

fn parse_bound_nodes(nodes: &[SExpr]) -> Result<Option<crate::ast::BoundSpec>, String> {
    let mut options = Vec::new();
    for node in nodes {
        if let Some(spec) = parse_bound_node(node)? {
            options.push(spec);
        }
    }
    Ok(match options.len() {
        1 => Some(options.pop().unwrap()),
        0 => None,
        _ => Some(crate::ast::BoundSpec::Choice(options)),
    })
}

fn parse_bound_node(node: &SExpr) -> Result<Option<crate::ast::BoundSpec>, String> {
    let p = match node {
        SExpr::List(p) => p,
        _ => return Ok(None),
    };
    let tag_name = sexpr_str(p.first().ok_or("empty bound node")?)?;
    match tag_name {
        ":single" => {
            let term = parse_bound_term(p.get(1))?;
            Ok(Some(crate::ast::BoundSpec::Single(term)))
        }
        ":range" => {
            let lo = parse_bound_term(p.get(1))?;
            let hi = parse_bound_term(p.get(2))?;
            Ok(Some(crate::ast::BoundSpec::Range(lo, hi)))
        }
        ":choice" => parse_bound_nodes(&p[1..]),
        _ => Ok(None),
    }
}

fn parse_bound_term(node: Option<&SExpr>) -> Result<crate::ast::BoundTerm, String> {
    let p = match node {
        Some(SExpr::List(p)) => p,
        _ => return Err("expected bound term list".into()),
    };
    let tag = sexpr_str(p.first().ok_or("empty bound term")?)?;
    match tag {
        ":lit" => {
            let raw = sexpr_str(p.get(1).ok_or("missing :lit value")?)?;
            let n = raw
                .parse::<i64>()
                .map_err(|e| format!("bad :lit bound value: {e}"))?;
            Ok(crate::ast::BoundTerm::Lit(n))
        }
        ":ref" => {
            let n = sexpr_str(p.get(1).ok_or("missing :ref value")?)?.to_string();
            Ok(crate::ast::BoundTerm::Ref(n))
        }
        other => Err(format!("unknown bound term tag '{other}'")),
    }
}

fn parse_params(expr: &SExpr) -> Result<Vec<(String, Type)>, String> {
    let parts = match expr { SExpr::List(p) => p, _ => return Ok(Vec::new()) };
    let mut params = Vec::new();
    for i in 1..parts.len() {
        if let SExpr::List(p) = &parts[i] {
            if p.len() >= 3 {
                let n = sexpr_str(&p[1])?.to_string();
                let t = parse_type(&p[2])?;
                params.push((n, t));
            }
        }
    }
    Ok(params)
}

fn parse_outputs(expr: &SExpr) -> Result<Vec<Type>, String> {
    let parts = match expr { SExpr::List(p) => p, _ => return Ok(Vec::new()) };
    let mut outputs = Vec::new();
    for i in 1..parts.len() {
        outputs.push(parse_type(&parts[i])?);
    }
    Ok(outputs)
}

fn parse_statement(expr: &SExpr) -> Result<Statement, String> {
    let parts = match expr { SExpr::List(p) => p, _ => return Err("expected list for statement".into()) };
    if parts.is_empty() { return Err("empty statement".into()); }
    let tag = sexpr_str(&parts[0])?;
    match tag {
        "assign" => Ok(Statement::Assign(parse_expr(&parts[1])?, parse_expr(&parts[2])?)),
        "let" => {
            let name = sexpr_str(&parts[1])?.to_string();
            let expr = if parts.len() > 2 { Some(parse_expr(&parts[2])?) } else { None };
             Ok(Statement::Let { names: vec![], name, ty: None, expr, modifiers: vec![] })
        }
        "term" => {
            let e = if parts.len() > 1 { Some(parse_expr(&parts[1])?) } else { None };
            Ok(Statement::Term(e))
        }
        "term!" => {
            let e = if parts.len() > 1 { Some(parse_expr(&parts[1])?) } else { None };
            Ok(Statement::EndProgram(e))
        }
        "yield" => Ok(Statement::Yield),
        "expr" => Ok(Statement::Expression(parse_expr(&parts[1])?)),
        "guarded" => {
            let cond = parse_expr(&parts[1])?;
            let mut body = Vec::new();
            for i in 2..parts.len() { body.push(parse_statement(&parts[i])?); }
            Ok(Statement::Guarded(cond, body))
        }
        "gate" => {
            let cond = parse_expr(&parts[1])?;
            Ok(Statement::Gate(cond))
        }
        "block" => {
            let mut body = Vec::new();
            for i in 1..parts.len() { body.push(parse_statement(&parts[i])?); }
            Ok(Statement::Block(body))
        }
        "metadata" => {
            let key = sexpr_str(&parts[1])?.to_string();
            let val = sexpr_to_pv(&parts[2])?;
            Ok(Statement::MetadataAssignment(key, val))
        }
        _ => Err(format!("unknown statement tag '{}'", tag)),
    }
}

pub(crate) fn parse_expr(expr: &SExpr) -> Result<Expr, String> {
    match expr {
        SExpr::Atom(a) => match a {
            Atom::Int(n) => Ok(Expr::Decimal(*n)),
            Atom::Float(f) => Ok(Expr::Float(*f)),
            Atom::Bool(b) => Ok(Expr::Bool(*b)),
            Atom::String(s) => Ok(Expr::Identifier(s.clone())),
        },
        SExpr::List(parts) => {
            if parts.is_empty() { return Err("empty expression list".into()); }
            let tag = sexpr_str(&parts[0])?;
            match tag {
                "ident" => Ok(Expr::Identifier(sexpr_str(&parts[1])?.to_string())),
                "call" => {
                    let name = sexpr_str(&parts[1])?.to_string();
                    let mut args = Vec::new();
                    for i in 2..parts.len() { args.push(parse_expr(&parts[i])?); }
                    Ok(Expr::Call(name, args, None))
                }
                "binop" => {
                    let kind = parse_binop(sexpr_str(&parts[1])?)?;
                    Ok(Expr::BinaryOp(kind, Box::new(parse_expr(&parts[2])?), Box::new(parse_expr(&parts[3])?)))
                }
                "unop" => {
                    let kind = parse_unop(sexpr_str(&parts[1])?)?;
                    Ok(Expr::UnaryOp(kind, Box::new(parse_expr(&parts[2])?)))
                }
                "field" => Ok(Expr::Field(Box::new(parse_expr(&parts[1])?), sexpr_str(&parts[2])?.to_string())),
                "index" => Ok(Expr::Index(Box::new(parse_expr(&parts[1])?), Box::new(parse_expr(&parts[2])?))),
                "tuple" => {
                    let mut items = Vec::new();
                    for i in 1..parts.len() { items.push(parse_expr(&parts[i])?); }
                    Ok(Expr::Tuple(items))
                }
                "list" => {
                    let mut items = Vec::new();
                    for i in 1..parts.len() { items.push(parse_expr(&parts[i])?); }
                    Ok(Expr::List(items))
                }
                "cast" => Ok(Expr::Cast(Box::new(parse_expr(&parts[1])?), parse_type(&parts[2])?)),
                "deref" => Ok(Expr::Deref(Box::new(parse_expr(&parts[1])?))),
                "addrof" => Ok(Expr::AddrOf(Box::new(parse_expr(&parts[1])?))),
                "string" => Ok(Expr::Quoted(sexpr_str(&parts[1])?.as_bytes().to_vec())),
                _ => Err(format!("unknown expression tag '{}'", tag)),
            }
        }
    }
}

fn parse_type(expr: &SExpr) -> Result<Type, String> {
    let s = sexpr_str(expr)?;
    // Simple heuristic: parse common type names
    Ok(match s {
        "Int" => Type::int(),
        "Float" => Type::float(),
        "Float64" => Type::float64(),
        "Bool" => Type::bool_(),
        "String" => Type::string(),
        "Char" => Type::char_(),
        "Void" => Type::void(),
        _ if s.starts_with('"') && s.ends_with('"') => {
            Type::Custom(s[1..s.len()-1].to_string())
        }
        _ => Type::Custom(s.to_string()),
    })
}

fn parse_binop(s: &str) -> Result<BinaryOpKind, String> {
    Ok(match s {
        "Add" => BinaryOpKind::Add, "Sub" => BinaryOpKind::Sub,
        "Mul" => BinaryOpKind::Mul, "Div" => BinaryOpKind::Div,
        "Mod" => BinaryOpKind::Mod, "Eq" => BinaryOpKind::Eq,
        "Neq" => BinaryOpKind::Neq, "Lt" => BinaryOpKind::Lt,
        "Le" => BinaryOpKind::Le, "Gt" => BinaryOpKind::Gt,
        "Ge" => BinaryOpKind::Ge, "And" => BinaryOpKind::And,
        "Or" => BinaryOpKind::Or, "BitAnd" => BinaryOpKind::BitAnd,
        "BitOr" => BinaryOpKind::BitOr, "BitXor" => BinaryOpKind::BitXor,
        "Shl" => BinaryOpKind::Shl, "Shr" => BinaryOpKind::Shr,
        "Concat" => BinaryOpKind::Concat,
        _ => return Err(format!("unknown BinaryOpKind '{}'", s)),
    })
}

fn parse_unop(s: &str) -> Result<UnaryOpKind, String> {
    Ok(match s {
        "Neg" => UnaryOpKind::Neg, "Not" => UnaryOpKind::Not,
        "BitNot" => UnaryOpKind::BitNot,
        _ => return Err(format!("unknown UnaryOpKind '{}'", s)),
    })
}

fn sexpr_to_pv(expr: &SExpr) -> Result<PropertyValue, String> {
    match expr {
        SExpr::Atom(Atom::String(s)) => {
            // 2026-07-18: Deserialize hash words from string tags
            Ok(match s.as_str() {
                "#Lh" => PropertyValue::HashL,
                "#Rh" => PropertyValue::HashR,
                "#T" => PropertyValue::HashT,
                _ => PropertyValue::String(s.clone()),
            })
        }
        SExpr::Atom(Atom::Int(n)) => Ok(PropertyValue::Int(*n)),
        SExpr::Atom(Atom::Float(f)) => Ok(PropertyValue::Float(*f)),
        SExpr::Atom(Atom::Bool(b)) => Ok(PropertyValue::Bool(*b)),
        SExpr::List(parts) => {
            if parts.is_empty() { return Err("empty list in property value".into()); }
            let tag = sexpr_str(&parts[0])?;
            if tag == "list" {
                let mut items = Vec::new();
                for i in 1..parts.len() { items.push(sexpr_to_pv(&parts[i])?); }
                Ok(PropertyValue::List(items))
            } else {
                Ok(PropertyValue::Identifier(sexpr_str(&parts[0])?.to_string()))
            }
        }
    }
}
