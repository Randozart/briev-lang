// ── Destination ownership analysis ─────────────────────────────────────
// 2026-10-29 (Phase R1, plan
// docs/plans/2026-10-29-destination-ownership-reconciliation.md): build the
// frontend `ValueDestinationPlan` from semantic value IDs, origins, uses,
// sinks, call summaries, and conservative escape/overlap facts.
//
// This phase is ANALYSIS-ONLY. No backend lowering consumes it yet. The plan
// intentionally over-approximates: every unknown result is boxed/copy and
// every non-primitive call argument starts as a borrow obligation. Backend
// implementation (Phase R2+) must consume these facts instead of deriving
// ownership from register names.
//
// Undo: delete this module, its AnalysisResults field, and its tests; no
// other code depends on it.

use crate::ast::{Expr, OutputType, Statement, TopLevel, Type};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

mod destination_walk;

/// Stable identity of one semantic value inside one callable.
///
/// `path` is a deterministic source-path string (parameter index, statement
/// index chain, expression index chain, or synthetic external path). It is
/// not an LLVM register name and survives register renaming.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ValueId {
    pub owner: String,
    pub path: String,
}

impl ValueId {
    pub(super) fn new(owner: impl Into<String>, path: impl Into<String>) -> Self {
        ValueId {
            owner: owner.into(),
            path: path.into(),
        }
    }
}

/// Where a semantic value was produced. A binding overwritten by assignment
/// may legitimately contain several origins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ValueOrigin {
    Parameter,
    Literal,
    InternalCall,
    UnknownCall,
    StateOrGlobalLoad,
    FieldLoad,
    CollectionLoad,
    Constructor,
    MatchResult,
    BinaryResult,
    UnaryResult,
    CastResult,
    Composite,
    ForeignResult,
    TaskResult,
    Unknown,
}

/// Consumer category. Escape-forming sinks also produce `EscapeKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SinkKind {
    Return,
    StateStore,
    CollectionInsert,
    ObjectFieldStore,
    UnknownCallArgument,
    ClosureCapture,
    LoopCarrier,
    ExportReturn,
}

impl SinkKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SinkKind::Return => "return",
            SinkKind::StateStore => "state-store",
            SinkKind::CollectionInsert => "collection-insert",
            SinkKind::ObjectFieldStore => "object-field-store",
            SinkKind::UnknownCallArgument => "unknown-call-argument",
            SinkKind::ClosureCapture => "closure-capture",
            SinkKind::LoopCarrier => "loop-carrier",
            SinkKind::ExportReturn => "export-return",
        }
    }
}

/// Lifetime escape implied by a sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EscapeKind {
    Returned,
    StateLifetime,
    CollectionLifetime,
    ObjectLifetime,
    UnknownCallLifetime,
    ClosureLifetime,
    LoopLifetime,
    ExportLifetime,
}

impl EscapeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EscapeKind::Returned => "returned",
            EscapeKind::StateLifetime => "state-lifetime",
            EscapeKind::CollectionLifetime => "collection-lifetime",
            EscapeKind::ObjectLifetime => "object-lifetime",
            EscapeKind::UnknownCallLifetime => "unknown-call-lifetime",
            EscapeKind::ClosureLifetime => "closure-lifetime",
            EscapeKind::LoopLifetime => "loop-lifetime",
            EscapeKind::ExportLifetime => "export-lifetime",
        }
    }
}

/// Physical representation class selected by R1 facts. Unknown is the
/// conservative fallback; no backend lowering consumes this yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DestinationClass {
    ScalarValue,
    Borrowed,
    CallerDestination,
    ScalarReplacedObject,
    StableCarrier,
    PackedMemory,
    Boxed,
    Unknown,
}

impl DestinationClass {
    pub fn as_str(self) -> &'static str {
        match self {
            DestinationClass::ScalarValue => "scalar-value",
            DestinationClass::Borrowed => "borrowed",
            DestinationClass::CallerDestination => "caller-destination",
            DestinationClass::ScalarReplacedObject => "scalar-replaced-object",
            DestinationClass::StableCarrier => "stable-carrier",
            DestinationClass::PackedMemory => "packed-memory",
            DestinationClass::Boxed => "boxed",
            DestinationClass::Unknown => "unknown",
        }
    }
}

/// Fact for one semantic value. Sinks are keyed by category and contain the
/// stable source site so diagnostics/reports are reproducible.
#[derive(Debug, Clone, PartialEq)]
pub struct ValueFact {
    pub id: ValueId,
    pub source_name: Option<String>,
    pub origins: BTreeSet<ValueOrigin>,
    pub class: DestinationClass,
    pub uses: u64,
    pub sinks: BTreeMap<SinkKind, BTreeSet<String>>,
    pub escapes: BTreeSet<EscapeKind>,
    /// Conservative points where a copy is required before the named sink.
    pub copy_points: BTreeMap<String, String>,
    /// R1 over-approximation: true when the value may overlap another value.
    pub may_overlap: bool,
    /// Human-readable conservative fallback reasons.
    pub reasons: BTreeSet<String>,
}

impl ValueFact {
    fn new(
        id: ValueId,
        origin: ValueOrigin,
        class: DestinationClass,
        source_name: Option<String>,
    ) -> Self {
        let mut origins = BTreeSet::new();
        origins.insert(origin);
        ValueFact {
            id,
            source_name,
            origins,
            class,
            uses: 0,
            sinks: BTreeMap::new(),
            escapes: BTreeSet::new(),
            copy_points: BTreeMap::new(),
            may_overlap: !matches!(origin, ValueOrigin::Literal),
            reasons: BTreeSet::new(),
        }
    }
}

/// Parameter obligation for one call site. Phase 9 declarations will replace
/// the conservative inference; absent declarations default to Borrow for
/// non-primitive values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ParamObligation {
    Value,
    Borrow,
    Consume,
    Owned,
    Shared,
    Unknown,
}

impl ParamObligation {
    pub fn as_str(self) -> &'static str {
        match self {
            ParamObligation::Value => "value",
            ParamObligation::Borrow => "borrow",
            ParamObligation::Consume => "consume",
            ParamObligation::Owned => "owned",
            ParamObligation::Shared => "shared",
            ParamObligation::Unknown => "unknown",
        }
    }
}

/// Where a callee places its result. Unknown is conservative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResultDestination {
    Void,
    SsaValue,
    CallerDestination,
    Boxed,
    Unknown,
}

impl ResultDestination {
    pub fn as_str(self) -> &'static str {
        match self {
            ResultDestination::Void => "void",
            ResultDestination::SsaValue => "ssa-value",
            ResultDestination::CallerDestination => "caller-destination",
            ResultDestination::Boxed => "boxed",
            ResultDestination::Unknown => "unknown",
        }
    }
}

/// Kind of callable. Unknown includes intrinsics whose destination lowering
/// is not yet summarized here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CallableKind {
    InternalDefn,
    InternalTransaction,
    Member,
    Foreign,
    Unknown,
}

/// Stable call-site identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CallSiteId {
    pub owner: String,
    pub site: String,
}

/// Summary for one call site.
#[derive(Debug, Clone, PartialEq)]
pub struct CallSiteFact {
    pub id: CallSiteId,
    pub callee: String,
    pub kind: CallableKind,
    pub obligations: Vec<ParamObligation>,
    pub result: ResultDestination,
    pub may_alias_args: bool,
    pub argument_values: Vec<ValueId>,
}

/// Summary for one callable name. Call resolution shares the Briev name
/// namespace, so duplicate same-name definitions intentionally map to one
/// conservative summary.
#[derive(Debug, Clone, PartialEq)]
pub struct CallableSummary {
    pub name: String,
    pub kind: CallableKind,
    pub parameters: Vec<ParamObligation>,
    pub result: ResultDestination,
    pub exported: bool,
    pub direct_recursive: bool,
    pub may_alias_args: bool,
}

/// A conservative diagnostic attached to the plan. This is queryable data;
/// R1 does not print it during normal compilation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DestinationDiagnostic {
    pub owner: String,
    pub value_path: String,
    pub severity: DestinationSeverity,
    pub code: DestinationDiagnosticCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DestinationSeverity {
    Info,
    ConservativeFallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DestinationDiagnosticCode {
    UnknownCallResult,
    UnknownCallArgument,
    EscapeRequiresCopy,
    CompositeNotYetClassified,
}

impl DestinationDiagnosticCode {
    pub fn as_str(self) -> &'static str {
        match self {
            DestinationDiagnosticCode::UnknownCallResult => "unknown-call-result",
            DestinationDiagnosticCode::UnknownCallArgument => "unknown-call-argument",
            DestinationDiagnosticCode::EscapeRequiresCopy => "escape-requires-copy",
            DestinationDiagnosticCode::CompositeNotYetClassified => "composite-not-yet-classified",
        }
    }
}

/// Facts for one callable body.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CallableDestinationFacts {
    pub owner: String,
    pub exported: bool,
    pub param_values: Vec<ValueId>,
    pub values: BTreeMap<ValueId, ValueFact>,
    pub call_sites: BTreeMap<CallSiteId, CallSiteFact>,
    pub diagnostics: BTreeSet<DestinationDiagnostic>,
    pub direct_recursive: bool,
}

/// Full Phase R1 result.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DestinationOwnershipPlan {
    /// Callable summaries keyed by Briev call name.
    pub callables: BTreeMap<String, CallableSummary>,
    /// Body facts keyed by canonical owner. Same-name member bodies may later
    /// require additional keys; R1 analyzes top-level callable bodies.
    pub callables_by_owner: BTreeMap<String, CallableDestinationFacts>,
    /// Call-site facts keyed by owner/site.
    pub call_sites: BTreeMap<CallSiteId, CallSiteFact>,
    /// Deterministic diagnostics.
    pub diagnostics: BTreeSet<DestinationDiagnostic>,
    /// 2026-10-30 (Phase D1): generalized all-domain schema. Analysis-only;
    /// no backend consumes this yet.
    pub generalized:
        crate::analysis::destination_model::GeneralizedDestinationOwnershipPlan,
}

/// Internal callable signature/body. Bodies are cloned once for analysis.
#[derive(Debug, Clone)]
pub(super) struct CallableSpec {
    pub(super) owner: String,
    pub(super) name: String,
    pub(super) kind: CallableKind,
    pub(super) parameters: Vec<(String, Type)>,
    pub(super) result: Option<Type>,
    pub(super) body: Vec<Statement>,
    pub(super) exported: bool,
}

/// Compute the Phase R1 destination ownership plan.
pub fn analyze(items: &[TopLevel]) -> DestinationOwnershipPlan {
    let (mut specs, export_names) = index_callables(items);
    for name in &export_names {
        if let Some(spec) = specs.get_mut(name) {
            spec.exported = true;
        }
    }

    let recursive = recursive_callables(&specs);
    let summaries = summarize_callables(&specs, &recursive);
    let facts = analyze_callables(&specs, &summaries, &recursive);

    let mut plan = DestinationOwnershipPlan {
        callables: summaries,
        ..Default::default()
    };
    for fact_set in facts {
        plan.call_sites.extend(fact_set.call_sites.clone());
        plan.diagnostics.extend(fact_set.diagnostics.clone());
        let name = specs_by_owner(&specs, &fact_set.owner);
        plan.callables_by_owner.insert(name, fact_set);
    }
    plan.generalized = crate::analysis::destination_model::build(items, &plan);
    plan
}

fn index_callables(items: &[TopLevel]) -> (BTreeMap<String, CallableSpec>, BTreeSet<String>) {
    let mut specs = BTreeMap::new();
    let mut export_names = BTreeSet::new();
    for item in items {
        match item {
            TopLevel::Definition(d) => insert_spec(
                &mut specs,
                spec_from_defn(d, CallableKind::InternalDefn, false),
            ),
            TopLevel::TypeDefOperator(d) => {
                insert_spec(&mut specs, spec_from_defn(d, CallableKind::Member, false))
            }
            TopLevel::Transaction(t) => insert_spec(
                &mut specs,
                CallableSpec {
                    owner: format!("txn:{}", t.name),
                    name: t.name.clone(),
                    kind: CallableKind::InternalTransaction,
                    parameters: t.parameters.clone(),
                    result: t.output_type.as_ref().and_then(result_type),
                    body: t.body.clone(),
                    exported: false,
                },
            ),
            TopLevel::CompileTimeDefn(d) => insert_spec(
                &mut specs,
                spec_from_defn(d, CallableKind::InternalDefn, false),
            ),
            TopLevel::CompileTimeTxn(t) => insert_spec(
                &mut specs,
                CallableSpec {
                    owner: format!("ctxn:{}", t.name),
                    name: t.name.clone(),
                    kind: CallableKind::InternalTransaction,
                    parameters: t.parameters.clone(),
                    result: t.output_type.as_ref().and_then(result_type),
                    body: t.body.clone(),
                    exported: false,
                },
            ),
            TopLevel::Export(e) => {
                if let Some(spec) = exported_spec(e.inner.as_ref()) {
                    export_names.insert(spec.name.clone());
                    insert_spec(&mut specs, spec);
                }
            }
            _ => {}
        }
    }
    (specs, export_names)
}

fn exported_spec(item: &TopLevel) -> Option<CallableSpec> {
    match item {
        TopLevel::Definition(d) => Some(spec_from_defn(d, CallableKind::InternalDefn, true)),
        TopLevel::TypeDefOperator(d) => Some(spec_from_defn(d, CallableKind::Member, true)),
        TopLevel::Transaction(t) => Some(CallableSpec {
            owner: format!("txn:{}", t.name),
            name: t.name.clone(),
            kind: CallableKind::InternalTransaction,
            parameters: t.parameters.clone(),
            result: t.output_type.as_ref().and_then(result_type),
            body: t.body.clone(),
            exported: true,
        }),
        TopLevel::CompileTimeDefn(d) => {
            Some(spec_from_defn(d, CallableKind::InternalDefn, true))
        }
        TopLevel::CompileTimeTxn(t) => Some(CallableSpec {
            owner: format!("ctxn:{}", t.name),
            name: t.name.clone(),
            kind: CallableKind::InternalTransaction,
            parameters: t.parameters.clone(),
            result: t.output_type.as_ref().and_then(result_type),
            body: t.body.clone(),
            exported: true,
        }),
        _ => None,
    }
}

fn insert_spec(specs: &mut BTreeMap<String, CallableSpec>, spec: CallableSpec) {
    specs.insert(spec.name.clone(), spec);
}

fn recursive_callables(specs: &BTreeMap<String, CallableSpec>) -> BTreeSet<String> {
    specs
        .values()
        .filter(|spec| {
            let mut calls = BTreeSet::new();
            collect_calls(&spec.body, &mut calls);
            calls.contains(&spec.name)
        })
        .map(|spec| spec.name.clone())
        .collect()
}

fn summarize_callables(
    specs: &BTreeMap<String, CallableSpec>,
    recursive: &BTreeSet<String>,
) -> BTreeMap<String, CallableSummary> {
    specs
        .values()
        .map(|spec| {
            let direct_recursive = recursive.contains(&spec.name);
            (
                spec.name.clone(),
                summarize_callable(spec, direct_recursive),
            )
        })
        .collect()
}

fn analyze_callables(
    specs: &BTreeMap<String, CallableSpec>,
    summaries: &BTreeMap<String, CallableSummary>,
    recursive: &BTreeSet<String>,
) -> Vec<CallableDestinationFacts> {
    let mut exported_names = BTreeSet::new();
    for spec in specs.values() {
        if spec.exported {
            exported_names.insert(spec.owner.clone());
        }
    }
    specs
        .values()
        .map(|spec| {
            analyze_callable(
                spec,
                specs,
                summaries,
                exported_names.contains(&spec.owner),
                recursive.contains(&spec.name),
            )
        })
        .collect()
}

fn analyze_callable(
    spec: &CallableSpec,
    specs: &BTreeMap<String, CallableSpec>,
    summaries: &BTreeMap<String, CallableSummary>,
    exported_owner: bool,
    direct_recursive: bool,
) -> CallableDestinationFacts {
    let mut analyzer = BodyAnalyzer {
        owner: spec.owner.clone(),
        specs,
        summaries,
        exported_owner,
        facts: BTreeMap::new(),
        call_sites: BTreeMap::new(),
        diagnostics: BTreeSet::new(),
        temp_counter: 0,
        loop_depth: 0,
    };
    let mut scope = Scope {
        values: HashMap::new(),
    };
    let mut param_values = Vec::new();
    for (index, (name, ty)) in spec.parameters.iter().enumerate() {
        let class = if is_primitive(ty) {
            DestinationClass::ScalarValue
        } else {
            DestinationClass::Borrowed
        };
        let id = analyzer.fact(
            ValueId::new(
                spec.owner.clone(),
                format!("param/{}/{}", index, sanitize_path(name)),
            ),
            ValueOrigin::Parameter,
            class,
            Some(name.clone()),
        );
        param_values.push(id.clone());
        scope.define(name, id);
    }
    let body_path = format!("body/{}", sanitize_path(&spec.name));
    for (index, statement) in spec.body.iter().enumerate() {
        let mut path = vec![index];
        analyzer.stmt(statement, &body_path, &mut path, &mut scope);
    }
    CallableDestinationFacts {
        owner: spec.owner.clone(),
        exported: spec.exported,
        param_values,
        values: analyzer.facts,
        call_sites: analyzer.call_sites,
        diagnostics: analyzer.diagnostics,
        direct_recursive,
    }
}

fn specs_by_owner(specs: &BTreeMap<String, CallableSpec>, owner: &str) -> String {
    specs
        .values()
        .find(|spec| spec.owner == owner)
        .map(|spec| spec.name.clone())
        .unwrap_or_else(|| owner.to_string())
}

fn spec_from_defn(d: &crate::ast::Definition, kind: CallableKind, exported: bool) -> CallableSpec {
    CallableSpec {
        owner: format!("defn:{}", d.name),
        name: d.name.clone(),
        kind,
        parameters: d.parameters.clone(),
        result: d.output_type.as_ref().and_then(result_type),
        body: d.body.clone(),
        exported,
    }
}

fn result_type(output: &OutputType) -> Option<Type> {
    match output {
        OutputType::Single(ty) => Some(ty.clone()),
        other => other.all_types().first().cloned(),
    }
}

fn collect_calls(body: &[Statement], out: &mut BTreeSet<String>) {
    crate::ast::visit::walk_stmts(body, &mut |expression| {
        collect_calls_expr(expression, out);
    });
}

fn collect_calls_expr(expr: &Expr, out: &mut BTreeSet<String>) {
    match expr {
        Expr::Call(name, args, _) => {
            out.insert(name.clone());
            for arg in args {
                collect_calls_expr(arg, out);
            }
        }
        Expr::MethodCall(receiver, _, args, _, _) => {
            collect_calls_expr(receiver, out);
            for arg in args {
                collect_calls_expr(arg, out);
            }
        }
        Expr::BinaryOp(_, lhs, rhs) => {
            collect_calls_expr(lhs, out);
            collect_calls_expr(rhs, out);
        }
        Expr::UnaryOp(_, inner)
        | Expr::Field(inner, _)
        | Expr::Reflect(inner, _, _)
        | Expr::Index(inner, _)
        | Expr::Cast(inner, _)
        | Expr::IsType(inner, _)
        | Expr::Within(inner, _)
        | Expr::Deref(inner)
        | Expr::AddrOf(inner)
        | Expr::Consume(inner)
        | Expr::Await(inner) => collect_calls_expr(inner, out),
        Expr::Slice {
            array,
            start,
            end,
            stride,
        } => {
            collect_calls_expr(array, out);
            if let Some(expr) = start {
                collect_calls_expr(expr, out);
            }
            if let Some(expr) = end {
                collect_calls_expr(expr, out);
            }
            if let Some(expr) = stride {
                collect_calls_expr(expr, out);
            }
        }
        Expr::Range { start, end, .. } => {
            collect_calls_expr(start, out);
            collect_calls_expr(end, out);
        }
        Expr::Spawn { args, .. } => {
            for arg in args {
                collect_calls_expr(arg, out);
            }
        }
        Expr::Block(body) => collect_calls(body, out),
        Expr::If(condition, then_body, else_body) => {
            collect_calls_expr(condition, out);
            collect_calls_expr(then_body, out);
            if let Some(else_body) = else_body {
                collect_calls_expr(else_body, out);
            }
        }
        Expr::Match(scrut, arms) => {
            collect_calls_expr(scrut, out);
            for arm in arms {
                collect_calls_expr(&arm.body, out);
            }
        }
        Expr::Tuple(values) | Expr::List(values) => {
            for value in values {
                collect_calls_expr(value, out);
            }
        }
        Expr::StructLiteral { fields, specs, .. } => {
            for (_, expr) in fields.iter().chain(specs.iter()) {
                collect_calls_expr(expr, out);
            }
        }
        Expr::Lambda(_, body) => collect_calls_expr(body, out),
        Expr::PluginIntercept { args, receiver, .. } => {
            for arg in args {
                collect_calls_expr(arg, out);
            }
            if let Some(receiver) = receiver {
                collect_calls_expr(receiver, out);
            }
        }
        _ => {}
    }
}

fn is_primitive(ty: &Type) -> bool {
    match ty {
        Type::Bits(_) | Type::Void => true,
        Type::Custom(name) => {
            matches!(name.as_str(), "Int" | "Float" | "Float64" | "Bool" | "Char")
        }
        _ => false,
    }
}

fn summarize_callable(spec: &CallableSpec, direct_recursive: bool) -> CallableSummary {
    let parameters = spec
        .parameters
        .iter()
        .map(|(_, ty)| {
            if is_primitive(ty) {
                ParamObligation::Value
            } else {
                ParamObligation::Borrow
            }
        })
        .collect();

    let result = match spec.result.as_ref() {
        None => ResultDestination::Void,
        Some(ty) if is_primitive(ty) => ResultDestination::SsaValue,
        Some(_) => ResultDestination::Unknown,
    };

    CallableSummary {
        name: spec.name.clone(),
        kind: spec.kind,
        parameters,
        result,
        exported: spec.exported,
        direct_recursive,
        may_alias_args: spec.parameters.iter().any(|(_, ty)| !is_primitive(ty)),
    }
}

#[derive(Clone)]
pub(super) struct Scope {
    pub(super) values: HashMap<String, ValueId>,
}

impl Scope {
    pub(super) fn lookup(&self, name: &str) -> Option<ValueId> {
        self.values.get(name).cloned()
    }

    pub(super) fn define(&mut self, name: &str, id: ValueId) {
        self.values.insert(name.to_string(), id);
    }
}

pub(super) struct BodyAnalyzer<'a> {
    pub(super) owner: String,
    pub(super) specs: &'a BTreeMap<String, CallableSpec>,
    pub(super) summaries: &'a BTreeMap<String, CallableSummary>,
    pub(super) exported_owner: bool,
    pub(super) facts: BTreeMap<ValueId, ValueFact>,
    pub(super) call_sites: BTreeMap<CallSiteId, CallSiteFact>,
    pub(super) diagnostics: BTreeSet<DestinationDiagnostic>,
    pub(super) temp_counter: u64,
    /// Active foreach nesting depth. Loop-carried assignments are marked with
    /// a LoopCarrier sink so the generalized plan can build loop facts.
    pub(super) loop_depth: usize,
}

impl<'a> BodyAnalyzer<'a> {
    pub(super) fn fact(
        &mut self,
        id: ValueId,
        origin: ValueOrigin,
        class: DestinationClass,
        name: Option<String>,
    ) -> ValueId {
        let entry = self
            .facts
            .entry(id.clone())
            .or_insert_with(|| ValueFact::new(id.clone(), origin, class, name));
        entry.origins.insert(origin);
        id
    }

    pub(super) fn use_value(&mut self, id: &ValueId) {
        if let Some(fact) = self.facts.get_mut(id) {
            fact.uses += 1;
        }
    }

    pub(super) fn add_sink(
        &mut self,
        id: &ValueId,
        sink: SinkKind,
        site: &str,
        escape: EscapeKind,
    ) {
        let class = self
            .facts
            .get(id)
            .map(|fact| fact.class)
            .unwrap_or(DestinationClass::Unknown);
        if let Some(fact) = self.facts.get_mut(id) {
            fact.sinks.entry(sink).or_default().insert(site.to_string());
            fact.escapes.insert(escape);
            fact.copy_points
                .insert(site.to_string(), escape.as_str().to_string());
        }

        let fallback = matches!(class, DestinationClass::Boxed | DestinationClass::Unknown);
        if fallback
            && matches!(
                sink,
                SinkKind::Return | SinkKind::StateStore | SinkKind::CollectionInsert
            )
        {
            self.diagnostics.insert(DestinationDiagnostic {
                owner: self.owner.clone(),
                value_path: id.path.clone(),
                severity: DestinationSeverity::ConservativeFallback,
                code: DestinationDiagnosticCode::EscapeRequiresCopy,
                message: format!(
                    "{} at '{}' requires a copy because the value is {}",
                    sink.as_str(),
                    site,
                    class.as_str()
                ),
            });
        }
    }

    pub(super) fn diagnostic(
        &mut self,
        id: &ValueId,
        code: DestinationDiagnosticCode,
        message: impl Into<String>,
    ) {
        self.diagnostics.insert(DestinationDiagnostic {
            owner: self.owner.clone(),
            value_path: id.path.clone(),
            severity: DestinationSeverity::ConservativeFallback,
            code,
            message: message.into(),
        });
    }

    pub(super) fn call_site_id(&self, site: &str) -> CallSiteId {
        CallSiteId {
            owner: self.owner.clone(),
            site: site.to_string(),
        }
    }

    pub(super) fn summarize_site(
        &mut self,
        site: &str,
        callee: &str,
        args: &[ValueId],
    ) -> CallSiteFact {
        let (kind, obligations, result, may_alias_args) = match self.summaries.get(callee) {
            Some(summary) => (
                summary.kind,
                summary.parameters.clone(),
                summary.result,
                summary.may_alias_args,
            ),
            None => (
                CallableKind::Unknown,
                vec![ParamObligation::Unknown; args.len()],
                ResultDestination::Unknown,
                true,
            ),
        };

        CallSiteFact {
            id: self.call_site_id(site),
            callee: callee.to_string(),
            kind,
            obligations,
            result,
            may_alias_args,
            argument_values: args.to_vec(),
        }
    }
}

fn bind_pattern(pattern: &crate::ast::Pattern, scrut: &ValueId, scope: &mut Scope) {
    match pattern {
        crate::ast::Pattern::Binding(name) => {
            scope.define(name, scrut.clone());
        }
        crate::ast::Pattern::TypedBinding(name, _) => {
            scope.define(name, scrut.clone());
        }
        crate::ast::Pattern::EnumVariant(_, subpatterns) => {
            for subpattern in subpatterns {
                bind_pattern(subpattern, scrut, scope);
            }
        }
        crate::ast::Pattern::Tuple(subpatterns) => {
            for subpattern in subpatterns {
                bind_pattern(subpattern, scrut, scope);
            }
        }
        crate::ast::Pattern::Multi(subpatterns) => {
            for subpattern in subpatterns {
                bind_pattern(subpattern, scrut, scope);
            }
        }
        _ => {}
    }
}

fn collect_identifier_uses(body: &[Statement], out: &mut dyn FnMut(&str)) {
    crate::ast::visit::walk_stmts(body, &mut |expression| {
        walk_expression_identifiers(expression, out);
    });
}

fn walk_expression_identifiers(expr: &Expr, out: &mut dyn FnMut(&str)) {
    match expr {
        Expr::Identifier(name) => out(name),
        Expr::BinaryOp(_, lhs, rhs) => {
            walk_expression_identifiers(lhs, out);
            walk_expression_identifiers(rhs, out);
        }
        Expr::UnaryOp(_, inner)
        | Expr::Field(inner, _)
        | Expr::Cast(inner, _)
        | Expr::Index(inner, _)
        | Expr::Deref(inner)
        | Expr::AddrOf(inner)
        | Expr::Consume(inner)
        | Expr::Await(inner)
        | Expr::Reflect(inner, _, _) => walk_expression_identifiers(inner, out),
        Expr::Call(_, args, _) => {
            for arg in args {
                walk_expression_identifiers(arg, out);
            }
        }
        Expr::MethodCall(receiver, _, args, _, _) => {
            walk_expression_identifiers(receiver, out);
            for arg in args {
                walk_expression_identifiers(arg, out);
            }
        }
        Expr::Block(body) => crate::ast::visit::walk_stmts(body, &mut |expression| {
            walk_expression_identifiers(expression, out);
        }),
        _ => {}
    }
}

pub(super) fn format_path(path: &[usize]) -> String {
    path.iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join("_")
}

pub(super) fn sanitize_path(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analyze_src(src: &str) -> DestinationOwnershipPlan {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        let items = parser.parse_program().expect("parse");
        analyze(&items)
    }

    #[test]
    fn parameters_and_returns_have_semantic_ids() {
        let plan = analyze_src("defn identity(value: Int) -> Int { term value; };\n");
        let facts = plan
            .callables_by_owner
            .get("identity")
            .expect("identity facts");
        assert_eq!(facts.param_values.len(), 1);
        let param = &facts.param_values[0];
        let param_fact = facts.values.get(param).expect("param fact");
        assert_eq!(param_fact.class, DestinationClass::ScalarValue);
        assert_eq!(param_fact.origins, [ValueOrigin::Parameter].into());

        let return_fact = facts
            .values
            .values()
            .find(|fact| fact.sinks.contains_key(&SinkKind::Return))
            .expect("return sink");
        assert_eq!(return_fact.class, DestinationClass::ScalarValue);
    }

    #[test]
    fn unknown_call_results_are_boxed_with_diagnostics() {
        let plan = analyze_src(
            r#"
defn call_mystery() -> Int {
    let value: Int = Mystery#();
    term value;
};
"#,
        );
        let owner = plan
            .callables_by_owner
            .values()
            .find(|facts| {
                facts
                    .values
                    .values()
                    .any(|fact| fact.origins.contains(&ValueOrigin::UnknownCall))
            })
            .expect("calling callable");
        let call_fact = owner
            .values
            .values()
            .find(|fact| fact.origins.contains(&ValueOrigin::UnknownCall))
            .expect("unknown call result");
        assert!(
            matches!(
                call_fact.class,
                DestinationClass::Boxed | DestinationClass::Unknown
            ),
            "unknown call result must use a conservative destination class"
        );
        assert!(
            owner
                .diagnostics
                .iter()
                .any(|d| d.code == DestinationDiagnosticCode::UnknownCallResult)
        );
    }

    #[test]
    fn state_assignment_records_lifetime_escape() {
        let plan = analyze_src(
            r#"
let saved: Int = 0;
defn save(value: Int) { saved = value; };
"#,
        );
        let owner = plan.callables_by_owner.get("save").expect("save facts");
        let value_fact = owner
            .values
            .values()
            .find(|fact| fact.source_name.as_deref() == Some("value"))
            .expect("parameter value");
        assert!(value_fact.escapes.contains(&EscapeKind::StateLifetime));
        assert!(
            value_fact
                .sinks
                .get(&SinkKind::StateStore)
                .is_some_and(|sites| sites.iter().any(|site| site.contains("/stmt/")))
        );
        assert!(
            value_fact
                .copy_points
                .keys()
                .any(|point| point.contains("/stmt/")),
            "state escape must record a copy point"
        );
    }

    #[test]
    fn plan_is_deterministic_for_same_source() {
        let src = r#"
defn helper(v: Int) -> Int { term v; };
defn caller(v: Int) -> Int { term helper(v); };
"#;
        let first = analyze_src(src);
        let second = analyze_src(src);
        assert_eq!(first, second);
    }
}
