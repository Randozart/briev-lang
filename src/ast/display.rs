// ── Display Impls for AST Types ────────────────────────────────────────
// 2026-07-12: Phase 0.2 — Format AST types as valid Briev source text.
// 2026-07-22: Phase 8 — Round-trip tests verify Rust<->Briev pp parity.

use crate::ast::*;
use std::fmt;

// ═══════════════════════════════════════════════════════════════════════
// Display implementations
// ═══════════════════════════════════════════════════════════════════════

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Quoted(bytes) => write!(f, "\"{}\"", String::from_utf8_lossy(bytes)),
            // 2026-08-06 (Phase 7): `#b"..."` renders with its Data tag.
            Expr::TaggedQuotedLiteral(bytes, prefix) => {
                if prefix == "b" {
                    write!(f, "#b\"{}\"", String::from_utf8_lossy(bytes))
                } else {
                    write!(f, "{}\"{}\"", prefix, String::from_utf8_lossy(bytes))
                }
            }
            Expr::Decimal(n) | Expr::TaggedLiteral(n, _) => write!(f, "{}", n),
            Expr::Char(c) => write!(f, "'{}'", c),
            Expr::Bool(b) => write!(f, "{}", if *b { "true" } else { "false" }),
            Expr::BeginProgram => write!(f, "beginprogram"),
            Expr::Float(n) => write!(f, "{}", n),
            Expr::Identifier(name) => write!(f, "{}", name),
            Expr::Call(name, args, _) => {
                // 2026-09-22 (syntax-cleanup plan): the internal callee string
                // carries `Enum::Variant` (a stable registry contract); the
                // canonical spelling is member access `Enum.Variant`.
                write!(f, "{}(", name.replace("::", "."))?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", arg)?;
                }
                write!(f, ")")
            }
            // 2026-08-07 (object instance pools): `spawn Obj(args)`.
            // 2026-08-09 (Phase 5): `box`/`spill` storage-class prefixes.
            Expr::Spawn { type_name, args, storage } => {
                let kw = storage.keyword();
                if kw.is_empty() {
                    write!(f, "spawn {}(", type_name)?;
                } else {
                    write!(f, "{} spawn {}(", kw, type_name)?;
                }
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", a)?;
                }
                write!(f, ")")
            }
            Expr::BinaryOp(kind, lhs, rhs) => {
                write!(f, "({} {} {})", lhs, kind, rhs)
            }
            Expr::UnaryOp(kind, expr) => {
                write!(f, "({}{})", kind, expr)
            }
            Expr::Field(obj, name) => write!(f, "{}.{}", obj, name),
            Expr::Index(obj, index) => write!(f, "{}[{}]", obj, index),
            Expr::Slice { array, start, end, stride } => {
                write!(f, "{}[", array)?;
                if let Some(s) = start { write!(f, "{}", s)?; }
                write!(f, ":")?;
                if let Some(e) = end { write!(f, "{}", e)?; }
                if let Some(s) = stride { write!(f, ":{}", s)?; }
                write!(f, "]")
            }
            // 2026-08-07 (Phase 7): `start..end` / `start..=end` ranges.
            Expr::Range { start, end, inclusive } => write!(
                f,
                "{}..{}{}",
                start,
                if *inclusive { "=" } else { "" },
                end
            ),
            Expr::Exists(name) => write!(f, "{}?", name),
            Expr::Block(stmts) => {
                write!(f, "{{ ")?;
                for stmt in stmts {
                    write!(f, "{} ", stmt)?;
                }
                write!(f, "}}")
            }
            Expr::If(cond, then, else_) => {
                write!(f, "if {} then {}", cond, then)?;
                if let Some(else_) = else_ {
                    write!(f, " else {}", else_)?;
                }
                Ok(())
            }
            Expr::Match(expr, arms) => {
                write!(f, "match {} {{ ", expr)?;
                for (i, arm) in arms.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", arm)?;
                }
                write!(f, " }}")
            }
            Expr::Tuple(elems) => {
                write!(f, "(")?;
                for (i, elem) in elems.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", elem)?;
                }
                write!(f, ")")
            }
            Expr::List(elems) => {
                write!(f, "[")?;
                for (i, elem) in elems.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", elem)?;
                }
                write!(f, "]")
            }
            Expr::Lambda(params, body) => {
                write!(f, "(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", p)?;
                }
                write!(f, ") => {}", body)
            }
            Expr::Cast(expr, ty) => write!(f, "{} as {}", expr, ty),
            Expr::IsType(expr, ty) => write!(f, "{} is {}", expr, ty),
            Expr::Within(expr, scope) => write!(f, "{} within {}", expr, scope),
            Expr::DerivationBlock(block) => fmt::Display::fmt(block, f),
            Expr::Deref(inner) => write!(f, "*{}", inner),
            Expr::AddrOf(inner) => write!(f, "&{}", inner),
            Expr::Consume(inner) => write!(f, "~{}", inner),
            Expr::Await(inner) => write!(f, "await {}", inner),
            Expr::PluginIntercept { name, args, type_args: _, receiver: _, chain_refs: _ } => {
                write!(f, "{}!(", name)?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}", arg)?;
                }
                write!(f, ")")
            }
            Expr::Reflect(recv, name, kind) => match kind {
                ReflectKind::Runtime => write!(f, "{}.^{}", recv, name),
                ReflectKind::CompileTime => write!(f, "{}.^^{}", recv, name),
            },
            Expr::MethodCall(recv, name, args, _, _) => {
                write!(f, "{}.{}(", recv, name)?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}", arg)?;
                }
                write!(f, ")")
            }
            Expr::FormattingAnnotation(fmt_) => write!(f, "formatting <~ {}", fmt_.name()),
            Expr::StructLiteral { type_name, .. } => write!(f, "{} {{ ... }}", type_name),
            Expr::UnitLiteral { value, unit } => write!(f, "{}{}", value, unit),
            Expr::Capture { expr, name } => write!(f, "({}) >> {}", expr, name),
        }
    }
}

/// Round-trippable rendering of `!>` metadata values (inverse of
/// `Parser::parse_metadata_value`): identifiers are bare, strings quoted,
/// lists bracketed. Used by the canonical formatter for module-level `!>` and
/// by statement metadata display.
impl fmt::Display for MatchArm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.pattern)?;
        if let Some(guard) = &self.guard {
            write!(f, " when {}", guard)?;
        }
        write!(f, " => {}", self.body)
    }
}

/// Round-trippable rendering of `!>` metadata values (inverse of
/// `Parser::parse_metadata_value`): identifiers are bare, strings quoted,
/// lists bracketed. Used by the canonical formatter for module-level `!>` and
/// by statement metadata display.
impl fmt::Display for PropertyValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PropertyValue::Int(n) => write!(f, "{}", n),
            PropertyValue::Float(n) => write!(f, "{}", n),
            PropertyValue::Bool(b) => write!(f, "{}", if *b { "true" } else { "false" }),
            PropertyValue::String(s) => write!(f, "\"{}\"", s),
            PropertyValue::Identifier(s) => write!(f, "{}", s),
            PropertyValue::List(items) => {
                write!(f, "[")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", item)?;
                }
                write!(f, "]")
            }
            // 2026-09-23 (quantities plan): render back to bare notation.
            PropertyValue::Quantity { si, dimension } => {
                write!(f, "{}", quantity_str(*si, *dimension))
            }
            PropertyValue::HashL => write!(f, "#Lh"),
            PropertyValue::HashR => write!(f, "#Rh"),
            PropertyValue::HashT => write!(f, "#T"),
        }
    }
}

/// Render a quantity (SI + dimension) back to bare notation for display
/// and round-trip: pick a scaling prefix that keeps the magnitude in a
/// readable range (`100nF`, `2mA`, `3.3V`, `4.7kΩ`). The output is
/// re-parseable by the quantity grammar.
fn quantity_str(si: f64, dim: crate::ast::QuantityDim) -> String {
    let base = match dim {
        crate::ast::QuantityDim::Volt => "V",
        crate::ast::QuantityDim::Amp => "A",
        crate::ast::QuantityDim::Ohm => "Ω",
        crate::ast::QuantityDim::Farad => "F",
        crate::ast::QuantityDim::Henry => "H",
        crate::ast::QuantityDim::Hertz => "Hz",
        crate::ast::QuantityDim::Watt => "W",
        crate::ast::QuantityDim::Kelvin => "K",
        crate::ast::QuantityDim::Length => "m",
    };
    let prefixes: [(f64, &str); 8] = [
        (1e-12, "p"),
        (1e-9, "n"),
        (1e-6, "u"),
        (1e-3, "m"),
        (1.0, ""),
        (1e3, "k"),
        (1e6, "M"),
        (1e9, "G"),
    ];
    let (scale, pfx) = prefixes
        .iter()
        .rev()
        .find(|(sc, _)| si.abs() >= *sc)
        .unwrap_or(&(1.0, ""));
    let v = si / scale;
    let mut s = format!("{:.4}", v);
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    format!("{}{}{}", s, pfx, base)
}

impl fmt::Display for BinaryOpKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BinaryOpKind::Add => write!(f, "+"),
            BinaryOpKind::Sub => write!(f, "-"),
            BinaryOpKind::Mul => write!(f, "*"),
            BinaryOpKind::Div => write!(f, "/"),
            BinaryOpKind::Mod => write!(f, "%"),
            BinaryOpKind::Eq => write!(f, "=="),
            BinaryOpKind::Neq => write!(f, "!="),
            BinaryOpKind::Lt => write!(f, "<"),
            BinaryOpKind::Gt => write!(f, ">"),
            BinaryOpKind::Le => write!(f, "<="),
            BinaryOpKind::Ge => write!(f, ">="),
            BinaryOpKind::And => write!(f, "&&"),
            BinaryOpKind::Or => write!(f, "||"),
            BinaryOpKind::BitAnd => write!(f, "&"),
            BinaryOpKind::BitOr => write!(f, "|"),
            BinaryOpKind::BitXor => write!(f, "^"),
            BinaryOpKind::Shl => write!(f, "<<"),
            BinaryOpKind::Shr => write!(f, ">>"),
            BinaryOpKind::Concat => write!(f, "++"),
        }
    }
}

impl fmt::Display for UnaryOpKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnaryOpKind::Neg => write!(f, "-"),
            UnaryOpKind::Not => write!(f, "!"),
            UnaryOpKind::BitNot => write!(f, "~"),
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Bits(n) => write!(f, "Bit<{}>", n),
            Type::Void => write!(f, "void"),
            Type::Number(n) => write!(f, "{}", n),
            Type::Custom(name) => write!(f, "{}", name),
            Type::Generic(name, args) => {
                write!(f, "{}<", name)?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", arg)?;
                }
                write!(f, ">")
            }
            Type::Applied(name, args) => {
                write!(f, "{}<", name)?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", arg)?;
                }
                write!(f, ">")
            }
            Type::Union(types) => {
                for (i, t) in types.iter().enumerate() {
                    if i > 0 {
                        write!(f, " | ")?;
                    }
                    write!(f, "{}", t)?;
                }
                Ok(())
            }
            Type::Tuple(types) => {
                write!(f, "(")?;
                for (i, t) in types.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", t)?;
                }
                write!(f, ")")
            }
            Type::TypeVar(name) => write!(f, "{}", name),
            Type::Dyn(inner) => write!(f, "dyn {}", inner),
            Type::Task(inner) => write!(f, "Task<{}>", inner),
            Type::Ptr(inner) => write!(f, "Ptr<{}>", inner),
            Type::PtrConst(inner) => write!(f, "Ptr<const {}>", inner),
            Type::Function(params, ret) => {
                write!(f, "(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", p)?;
                }
                write!(f, ") -> {}", ret)
            }
            Type::Width(n) => write!(f, "{}", n),
            Type::Vector(ty, dims) => {
                write!(f, "{}", ty)?;
                for dim in dims {
                    write!(f, "[{}]", dim)?;
                }
                Ok(())
            }
            // 2026-08-25 (sized scalars): Single-width renders as the
            // source syntax `Int<8>`; exotic ranges keep the diagnostic form.
            Type::Constrained(ty, range) => match range {
                BitRange::Single(w) => write!(f, "{}<{}>", ty, w),
                other => write!(f, "{} @/ {:?}", ty, other),
            },
            Type::LayoutPtr(c) => write!(f, "Ptr<Bits @/{}>", c.bytes),
        }
    }
}

impl fmt::Display for Dimension {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Dimension::Anonymous(n) => write!(f, "{}", n),
            Dimension::Named(name, n) => write!(f, "{} = {}", name, n),
        }
    }
}

impl fmt::Display for Statement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Statement::Let { name, ty, expr, .. } => {
                if let Some(ty) = ty {
                    if let Some(expr) = expr {
                        write!(f, "let {}: {} = {};", name, ty, expr)
                    } else {
                        write!(f, "let {}: {};", name, ty)
                    }
                } else if let Some(expr) = expr {
                    write!(f, "let {} = {};", name, expr)
                } else {
                    write!(f, "let {};", name)
                }
            }
            Statement::Assign(lhs, rhs) => write!(f, "{} = {};", lhs, rhs),
            Statement::FreeHint(name) => write!(f, "free {};", name),
            Statement::KeepHint(name) => write!(f, "keep {};", name),
            Statement::ArrowAssign { target, value, consume } => {
                match target {
                    Some(t) => write!(f, "{} {} {}", t, if *consume { "~<-" } else { "<-" }, value),
                    None => write!(f, "{} {}", if *consume { "~<-" } else { "<-" }, value),
                }
            }
            Statement::Term(val) => {
                if let Some(val) = val {
                    write!(f, "term {};", val)
                } else {
                    write!(f, "term;")
                }
            }
            Statement::Trap => write!(f, "trap;"),
            Statement::Halt => write!(f, "halt;"),
            Statement::Yield => write!(f, "yield;"),
            Statement::Check(e) => write!(f, "check {};", e),
            Statement::Break => write!(f, "break;"),
            Statement::EndProgram(val) => {
                if let Some(val) = val {
                    write!(f, "endprogram {};", val)
                } else {
                    write!(f, "endprogram;")
                }
            }
            Statement::Guarded(cond, body) => {
                write!(f, "when {} {{ ", cond)?;
                for stmt in body {
                    write!(f, "{} ", stmt)?;
                }
                write!(f, "}}")
            }
            Statement::Gate(cond) => write!(f, "[{}];", cond),
            Statement::Expression(expr) => write!(f, "{};", expr),
            Statement::Block(stmts) => {
                write!(f, "{{ ")?;
                for stmt in stmts {
                    write!(f, "{} ", stmt)?;
                }
                write!(f, "}}")
            }
            // 2026-09-22 (D16 p3b): author-expressed disconnection.
            Statement::Open(lhs, rhs) => write!(f, "open {}, {};", lhs, rhs),
            Statement::MetadataAssignment(key, val) => {
                write!(f, "!> {}: {};", key, val)
            }
            Statement::Rollback(val) => {
                if let Some(val) = val {
                    write!(f, "escape {};", val)
                } else {
                    write!(f, "escape;")
                }
            }
            Statement::Foreach { item, list, body } => {
                write!(f, "foreach({} in {}) {{ ", item, list)?;
                for stmt in body {
                    write!(f, "{} ", stmt)?;
                }
                write!(f, "}}")
            }
            Statement::TrgBinding { name, instance } => {
                write!(f, "trg {} @ {};", name, instance)
            }
            Statement::InlineAsm { asm_string, .. } => {
                write!(f, "asm \"{}\";", asm_string)
            }
            Statement::SyncBlock(body) => {
                write!(f, "sync {{ ")?;
                for stmt in body {
                    write!(f, "{} ", stmt)?;
                }
                write!(f, "}}")
            }
            Statement::Defer(body) => {
                write!(f, "defer {{ ")?;
                for stmt in body {
                    write!(f, "{} ", stmt)?;
                }
                write!(f, "}}")
            }
            Statement::Mutex(body) => {
                write!(f, "mutex {{ ")?;
                for stmt in body {
                    write!(f, "{} ", stmt)?;
                }
                write!(f, "}}")
            }
            Statement::InlineDefn(d) => write!(f, "$defn {}", d.name),
            Statement::InlineTxn(t) => write!(f, "$txn {}", t.name),
            Statement::Match { .. } => write!(f, "match {{ ... }}"),
        }
    }
}

/// 2026-08-06 (Phase 11): render selective-import symbols — `a` for an
/// unrenamed symbol, `Local: Exported` for a rename.
fn import_symbols(symbols: &[(String, String)]) -> String {
    symbols
        .iter()
        .map(|(l, e)| if l == e { l.clone() } else { format!("{}: {}", l, e) })
        .collect::<Vec<_>>()
        .join(", ")
}

impl fmt::Display for TopLevel {    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TopLevel::Definition(defn) => {
                write!(f, "defn {}(", defn.name)?;
                for (i, (name, ty)) in defn.parameters.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}: {}", name, ty)?;
                }
                write!(f, ")")?;
                if let Some(oty) = &defn.output_type {
                    write!(f, " -> {}", oty)?;
                }
                write!(f, " {}", defn.contract)?;
                if let Some(deriv) = &defn.derivation {
                    write!(f, " {}", deriv)?;
                }
                write!(f, " {{ ")?;
                for stmt in &defn.body {
                    write!(f, "{} ", stmt)?;
                }
                write!(f, "}};")
            }
            TopLevel::Transaction(txn) => {
                let prefix = if txn.is_reactive { "node" } else { "txn" };
                write!(f, "{} {}", prefix, txn.name)?;
                if !txn.parameters.is_empty() {
                    write!(f, "(")?;
                    for (i, (name, ty)) in txn.parameters.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{}: {}", name, ty)?;
                    }
                    write!(f, ")")?;
                }
                write!(f, " {}", txn.contract)?;
                write!(f, " {{ ")?;
                for stmt in &txn.body {
                    write!(f, "{} ", stmt)?;
                }
                write!(f, "}};")
            }
            TopLevel::Cell(cell) => {
                write!(f, "cell {} {{ ... }};", cell.name)
            }
            TopLevel::Import(import) => {
                match &import.kind {
                    ImportKind::Literal(path) => {
                        if import.symbols.is_empty() {
                            write!(f, "import \"{}\";", path)
                        } else {
                            write!(f, "import {{ {} }} from \"{}\";", import_symbols(&import.symbols), path)
                        }
                    }
                    ImportKind::Registry(name) => {
                        if import.symbols.is_empty() {
                            write!(f, "import <{}>;", name)
                        } else {
                            write!(f, "import {{ {} }} from <{}>;", import_symbols(&import.symbols), name)
                        }
                    }
                }
            }
            TopLevel::Export(export) => {
                write!(f, "export {}", export.inner)
            }
            TopLevel::Trigger(trg) => {
                write!(f, "trg {} @ {};", trg.name, trg.instance)
            }
            TopLevel::CompileTimeLet(name, expr) => {
                write!(f, "$let {} = {};", name, expr)
            }
            TopLevel::CompileTimeConst(name, expr) => {
                write!(f, "$const {} = {};", name, expr)
            }
            TopLevel::Init(init) => {
                write!(f, "init {}", init.name)?;
                write!(f, ":")?;
                if let Some(bound) = &init.bound {
                    write!(f, " [{}]", display_bound_set(bound))?;
                }
                write!(f, " {}", init.ty)?;
                if let Some(value) = &init.value {
                    write!(f, " = {};", value)
                } else {
                    write!(f, " {{ ")?;
                    for stmt in &init.body {
                        write!(f, "{} ", stmt)?;
                    }
                    write!(f, "}};")
                }
            }
            _ => write!(f, "<definition>"),
        }
    }
}

impl fmt::Display for OutputType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OutputType::Single(ty) => write!(f, "{}", ty),
            OutputType::Union(types) => {
                for (i, t) in types.iter().enumerate() {
                    if i > 0 {
                        write!(f, " | ")?;
                    }
                    write!(f, "{}", t)?;
                }
                Ok(())
            }
            OutputType::Tuple(types) => {
                write!(f, "(")?;
                for (i, t) in types.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", t)?;
                }
                write!(f, ")")
            }
            OutputType::Array(inner) => write!(f, "[]{}", inner),
            OutputType::Named(name, inner) => write!(f, "{}: {}", name, inner),
        }
    }
}

impl fmt::Display for Contract {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}]", self.pre_condition)?;
        write!(f, "[{}]", self.post_condition)
    }
}


impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Pattern::Wildcard => write!(f, "_"),
            Pattern::Literal(expr) => write!(f, "{}", expr),
            Pattern::Binding(name) => write!(f, "{}", name),
            // 2026-08-22 (Phase 3): typed binding of a structural sum member.
            Pattern::TypedBinding(name, ty) => write!(f, "{}: {}", name, ty),
            Pattern::EnumVariant(name, fields) => {
                // 2026-09-22: canonical `.` spelling for qualified variants.
                write!(f, "{}", name.replace("::", "."))?;
                if !fields.is_empty() {
                    write!(f, "(")?;
                    for (i, field) in fields.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{}", field)?;
                    }
                    write!(f, ")")?;
                }
                Ok(())
            }
            Pattern::Tuple(elems) => {
                write!(f, "(")?;
                for (i, elem) in elems.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", elem)?;
                }
                write!(f, ")")
            }
            Pattern::Range(start, end) => write!(f, "{}..{}", start, end),
            Pattern::RangeInclusive(start, end) => write!(f, "{}..={}", start, end),
            Pattern::Multi(ps) => {
                for (i, p) in ps.iter().enumerate() {
                    if i > 0 { write!(f, " | ")?; }
                    write!(f, "{}", p)?;
                }
                Ok(())
            }
        }
    }
}

impl fmt::Display for DerivationBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, ":= {{ ")?;
        for example in &self.examples {
            write!(f, "{}", example)?;
        }
        write!(f, "}}")
    }
}

impl fmt::Display for DerivationExample {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, input) in self.inputs.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{}", input)?;
        }
        // 2026-07-28: Show tolerance when present: `input -> [tol] output`
        if let Some(tol) = self.tolerance {
            write!(f, " -> [{}] {}", tol, self.output)
        } else {
            write!(f, " -> {}", self.output)
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Round-trip tests (Rust Display vs Briev pretty-printer via GLUE bridge)
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    /// Snapshot test: verify Rust Display output for all Type variants.
    #[test]
    fn test_display_type_snapshots() {
        assert_eq!(format!("{}", Type::Bits(8)), "Bit<8>");
        assert_eq!(format!("{}", Type::Void), "void");
        assert_eq!(format!("{}", Type::Custom("Int".into())), "Int");
        assert_eq!(
            format!("{}", Type::Generic("List".into(), vec![Type::Custom("Int".into())])),
            "List<Int>"
        );
        assert_eq!(
            format!("{}", Type::Applied("Result".into(), vec![Type::Custom("Int".into()), Type::Custom("Error".into())])),
            "Result<Int, Error>"
        );
        assert_eq!(format!("{}", Type::Union(vec![Type::Custom("A".into()), Type::Custom("B".into())])), "A | B");
        assert_eq!(format!("{}", Type::Tuple(vec![Type::Custom("Int".into()), Type::Custom("Bool".into())])), "(Int, Bool)");
        assert_eq!(format!("{}", Type::TypeVar("T".into())), "T");
        assert_eq!(format!("{}", Type::Ptr(Box::new(Type::Custom("Int".into())))), "Ptr<Int>");
        assert_eq!(format!("{}", Type::PtrConst(Box::new(Type::Custom("Int".into())))), "Ptr<const Int>");
        assert_eq!(
            format!("{}", Type::Function(vec![Type::Custom("Int".into())], Box::new(Type::Custom("Bool".into())))),
            "(Int) -> Bool"
        );
        assert_eq!(format!("{}", Type::Width(64)), "64");
        assert_eq!(
            format!("{}", Type::Vector(Box::new(Type::Custom("Float".into())), vec![Dimension::Anonymous(4)])),
            "Float[4]"
        );
    }

    /// Snapshot test: verify Rust Display output for BinaryOpKind.
    #[test]
    fn test_display_binop_snapshots() {
        assert_eq!(format!("{}", BinaryOpKind::Add), "+");
        assert_eq!(format!("{}", BinaryOpKind::Sub), "-");
        assert_eq!(format!("{}", BinaryOpKind::Mul), "*");
        assert_eq!(format!("{}", BinaryOpKind::Div), "/");
        assert_eq!(format!("{}", BinaryOpKind::Eq), "==");
        assert_eq!(format!("{}", BinaryOpKind::Neq), "!=");
        assert_eq!(format!("{}", BinaryOpKind::Lt), "<");
        assert_eq!(format!("{}", BinaryOpKind::Gt), ">");
        assert_eq!(format!("{}", BinaryOpKind::Le), "<=");
        assert_eq!(format!("{}", BinaryOpKind::Ge), ">=");
        assert_eq!(format!("{}", BinaryOpKind::And), "&&");
        assert_eq!(format!("{}", BinaryOpKind::Or), "||");
        assert_eq!(format!("{}", BinaryOpKind::BitAnd), "&");
        assert_eq!(format!("{}", BinaryOpKind::BitOr), "|");
        assert_eq!(format!("{}", BinaryOpKind::BitXor), "^");
        assert_eq!(format!("{}", BinaryOpKind::Shl), "<<");
        assert_eq!(format!("{}", BinaryOpKind::Shr), ">>");
        assert_eq!(format!("{}", BinaryOpKind::Concat), "++");
    }

    /// Snapshot test: verify Rust Display output for UnaryOpKind.
    #[test]
    fn test_display_unary_op_snapshots() {
        assert_eq!(format!("{}", UnaryOpKind::Neg), "-");
        assert_eq!(format!("{}", UnaryOpKind::Not), "!");
        assert_eq!(format!("{}", UnaryOpKind::BitNot), "~");
    }

    /// Snapshot test: verify BoundSpec rendering for `init` expected value sets.
    #[test]
    fn test_display_bound_set_snapshots() {
        use crate::ast::{BoundSpec, BoundTerm};
        assert_eq!(
            display_bound_set(&BoundSpec::Single(BoundTerm::Lit(64))),
            "64"
        );
        assert_eq!(
            display_bound_set(&BoundSpec::Range(BoundTerm::Lit(10), BoundTerm::Lit(54))),
            "10..54"
        );
        assert_eq!(
            display_bound_set(&BoundSpec::Choice(vec![
                BoundSpec::Single(BoundTerm::Lit(16)),
                BoundSpec::Single(BoundTerm::Lit(32)),
                BoundSpec::Single(BoundTerm::Lit(64)),
            ])),
            "16 | 32 | 64"
        );
    }
}

/// Render a bound set in canonical source form, e.g. `[64 | lo..hi]` bodies.
/// The brackets are added by the callers; this helper renders the interior.
pub fn display_bound_set(bound: &BoundSpec) -> String {
    fn render_term(t: &BoundTerm) -> String {
        match t {
            BoundTerm::Lit(n) => n.to_string(),
            BoundTerm::Ref(name) => name.clone(),
        }
    }
    fn render(bound: &BoundSpec) -> String {
        match bound {
            BoundSpec::Single(t) => render_term(t),
            BoundSpec::Range(lo, hi) => format!("{}..{}", render_term(lo), render_term(hi)),
            BoundSpec::Choice(parts) => {
                let rendered: Vec<String> = parts.iter().map(render).collect();
                rendered.join(" | ")
            }
        }
    }
    render(bound)
}
