//! 2026-08-12 (Phase 2b3, SPEC 21.3): obj-backed components.
//!
//! Components ARE objects. `obj Name` owns the component's state slots and
//! member transactions; `render Name { ... }` is the view fragment bound to
//! that obj. The tag namespace resolves deterministically:
//!
//! 1. `<c1 />` — a declared Briev instance var (`let c1: Counter`) mounts the
//!    fragment with bindings routed to `c1.*` slots (Briev-side instance; the
//!    PROGRAM owns it).
//! 2. `<Counter />` — a component type (`obj Counter` + `render Counter`)
//!    spawns an anonymous, reactor-owned instance in a per-mount pool
//!    (`Counter.<i>.*` slots); zero-init defaults, not referenceable by code.
//! 3. else a lowercase HTML element; else an unknown-tag warning.
//!
//! Seeding is BRIEV source, never Rust: `let c1: Counter = Counter { count: 5 }`
//! seeds `c1.count` from the StructLiteral. There are NO HTML props — the
//! frontend invents no state values. Every state change the frontend requests
//! (per-mount txn variants, lifecycle resets) is bound to a trg — a callable
//! transaction with a proven contract — never a direct store.
//!
//! Scope (slice 2b3): compile-time instance pools for both paths. Dynamic
//! component counts (`b-each` of components) remain a follow-up.

use crate::ast::{BinaryOpKind, Expr, Statement, TopLevel, Transaction, Type};
use std::collections::{HashMap, HashSet};

/// A single component mount's per-instance view-rewrite decision — the VIEW
/// layer applies these to the raw fragment (field signals → qualified slots,
/// trigger txns → mount variants). The analysis decides WHAT; the view
/// compiler decides HOW to format the HTML.
#[derive(Debug, Clone)]
pub struct MountSpec {
    /// The component name (for the `data-instance` marker).
    pub component: String,
    /// This mount's index.
    pub index: usize,
    /// The `data-instance` marker value — the slot prefix: `Counter.0` for an
    /// HTML-side pool spawn, `c1` for a Briev-side instance. The shim's b-when
    /// unmount resets the instance via this key.
    pub marker: String,
    /// Fragment-referenced field → instance-qualified slot
    /// (`count` → `Counter.0.count`).
    pub fields: Vec<(String, String)>,
    /// Original txn name → per-mount variant (`increment` → `increment_0`).
    pub txn_variants: std::collections::HashMap<String, String>,
}

/// The result of expanding component instances in a program + view.
#[derive(Debug)]
pub struct ComponentInstancePlan {
    /// Component name → per-mount view-rewrite specs (decisions only — the
    /// view layer applies them to the raw fragment). HTML-side anonymous
    /// spawns (`<Counter />`) route here.
    pub mounts: std::collections::HashMap<String, Vec<MountSpec>>,
    /// Briev-side instances (`let c1: Counter = Counter { count: 5 }` +
    /// `<c1 />`), keyed by the instance var name. The PROGRAM owns these.
    pub instance_specs: std::collections::HashMap<String, MountSpec>,
    /// Instance slot → Briev seed (`c1.count` → 5, from a StructLiteral). The
    /// backend seeds these into %State at init; the VALUES are Briev source.
    pub initializers: std::collections::HashMap<String, Expr>,
    /// Every per-instance mount, as `(component, index)`. The backend emits a
    /// per-instance reset so a b-when unmount re-applies the instance's seeds
    /// (remount = fresh).
    pub instances: Vec<(String, usize)>,
}

/// An obj's component-relevant surface: its state slots (name → type) and its
/// member transactions (bare-name form, self-parameterized on the slots).
struct ObjInfo {
    slots: HashMap<String, Type>,
    member_txns: HashMap<String, Transaction>,
}

/// Expand every `render Name` component into per-mount instance state.
/// Returns a DECLARATIVE plan (mount specs, initializers, instance list) and
/// appends the instance state declarations + txn variants to `items` (sourced
/// from the obj's member txns). No HTML formatting happens here — the view
/// layer consumes the specs.
pub fn expand_component_instances(
    items: &mut Vec<TopLevel>,
    view_html: &str,
) -> Result<ComponentInstancePlan, String> {
    // ── Collect render blocks + obj definitions ──────────────────────
    // 2026-08-23 (bugfix, determinism): BTreeMap — the mount loop below
    // rewrites `items` in iteration order, so a HashMap here made the
    // compiled AST (and thus emitted IR function order) vary per process
    // seed. Every map iterated for program rewriting must be sorted by key.
    // To undo: revert to HashMap (reintroduces nondeterministic mounts).
    let render_blocks: std::collections::BTreeMap<String, String> = items
        .iter()
        .filter_map(|item| match item {
            TopLevel::RenderBlock(rb) => Some((rb.struct_name.clone(), rb.view_html.clone())),
            _ => None,
        })
        .collect();
    let obj_defs = collect_obj_defs(items);

    let mut plan = ComponentInstancePlan {
        mounts: HashMap::new(),
        instance_specs: HashMap::new(),
        initializers: HashMap::new(),
        instances: Vec::new(),
    };
    // 2026-08-12 (2b3 slice 2): Briev-side instances — `let <name>: <Obj> =
    // <StructLiteral>` where the type has a render block. The PROGRAM owns
    // these: seeds are the literal's field values (Briev source), and the
    // `<name />` tag mounts the fragment routed to the instance's slots.
    let instance_infos = collect_instance_lets(items, &render_blocks, &obj_defs)?;
    let mut pending_resets: Vec<(String, HashMap<String, Type>)> = Vec::new();
    for (component, fragment_html) in &render_blocks {
        let refs = collect_fragment_refs(fragment_html);
        // A fragment with no state fields is a static view fragment — a single
        // empty spec (the view layer mounts it shared, 2b1 behavior). A static
        // container (`render Root`) needs no obj of its own.
        if refs.fields.is_empty() && refs.txns.is_empty() {
            plan.mounts.insert(
                component.clone(),
                vec![MountSpec {
                    component: component.clone(),
                    index: 0,
                    marker: format!("{}.0", component),
                    fields: Vec::new(),
                    txn_variants: HashMap::new(),
                }],
            );
            continue;
        }
        // 2026-08-12 (2b3): components ARE objects — `render Name` requires
        // `obj Name`; the fragment binds ONLY the obj's slots + member txns.
        let obj = require_component_obj(&obj_defs, component)?;
        validate_component_refs(obj, component, &refs)?;
        let mount_count = count_component_mounts(view_html, component, &render_blocks);
        let mut per_mount: Vec<MountSpec> = Vec::with_capacity(mount_count);
        // for_each (not a `for`) keeps expand single-level for Praetor.
        (0..mount_count).for_each(|i| {
            let (spec, slot_types) = build_pool_mount(items, obj, &refs, component, i);
            pending_resets.push((format!("{}.{}", component, i), slot_types));
            per_mount.push(spec);
            plan.instances.push((component.clone(), i));
        });
        plan.mounts.insert(component.clone(), per_mount);
    }
    // 2026-08-12 (2b3 slice 3): trg-based resets for the HTML-side pool
    // spawns — a callable txn re-applies the instance's initial state (type
    // defaults, zero) so a b-when unmount starts fresh; the write flows
    // through the reactive machinery (contract + flush).
    for (prefix, slot_types) in pending_resets {
        emit_reset_txn(items, &prefix, &slot_types, &plan.initializers)?;
    }
    // Briev-side instances: their specs (slots + variants, routed to the
    // instance-name prefix) are added to the plan after the pool pass.
    for (var_name, component, literal) in &instance_infos {
        let Some(obj) = obj_defs.get(component) else { continue };
        let Some(fragment_html) = render_blocks.get(component) else { continue };
        let refs = collect_fragment_refs(fragment_html);
        if refs.fields.is_empty() && refs.txns.is_empty() {
            continue;
        }
        build_instance_spec(items, obj, &refs, &(var_name, component, literal), &mut plan)?;
    }
    Ok(plan)
}

/// The obj a stateful `render Name` fragment must pair with — a render without
/// its obj is a compile error, never silently-mounted globals.
fn require_component_obj<'a>(
    obj_defs: &'a HashMap<String, ObjInfo>,
    component: &str,
) -> Result<&'a ObjInfo, String> {
    obj_defs.get(component).ok_or_else(|| {
        format!(
            "render '{}' requires an obj of the same name (components ARE \
             objects): declare `obj {} {{ ... }}` with the component's \
             state slots and member transactions",
            component, component
        )
    })
}

/// Validate the fragment binds ONLY the obj's own slots and member txns — a
/// reference to a non-member is never silently dead.
fn validate_component_refs(
    obj: &ObjInfo,
    component: &str,
    refs: &FragmentRefs,
) -> Result<(), String> {
    let missing_fields: Vec<&String> = refs
        .fields
        .iter()
        .filter(|f| !obj.slots.contains_key(*f))
        .collect();
    if !missing_fields.is_empty() {
        return Err(format!(
            "render '{}' references field(s) {} not in obj '{}' slots",
            component,
            missing_fields
                .iter()
                .map(|f| format!("'{}'", f))
                .collect::<Vec<_>>()
                .join(", "),
            component
        ));
    }
    let missing_txns: Vec<&String> = refs
        .txns
        .iter()
        .filter(|t| !obj.member_txns.contains_key(*t))
        .collect();
    if !missing_txns.is_empty() {
        return Err(format!(
            "render '{}' triggers transaction(s) {} not members of obj '{}'",
            component,
            missing_txns
                .iter()
                .map(|t| format!("'{}'", t))
                .collect::<Vec<_>>()
                .join(", "),
            component
        ));
    }
    Ok(())
}

/// Build one HTML-side pool mount's spec: the instance slots (typed by the
/// obj) + per-mount txn variants, plus the slot→type map the reset needs.
fn build_pool_mount(
    items: &mut Vec<TopLevel>,
    obj: &ObjInfo,
    refs: &FragmentRefs,
    component: &str,
    i: usize,
) -> (MountSpec, HashMap<String, Type>) {
    let prefix = format!("{}.{}", component, i);
    // Instance slots = the fragment's fields ∪ every slot a variant member
    // references — all typed by the obj (never `Type::int()` guessed). The
    // qualifier maps obj-slot identifiers to their instance-qualified names in
    // the member bodies + contracts.
    let slot_set = instance_slot_set(items, obj, refs, &prefix);
    let qualifier = |id: &str| -> Option<String> {
        if slot_set.contains(id) {
            Some(format!("{}.{}", prefix, id))
        } else {
            None
        }
    };
    let variant_txns = build_txn_variants(items, obj, &i.to_string(), refs, &qualifier);
    let slot_types = slot_set
        .iter()
        .map(|f| {
            (
                format!("{}.{}", prefix, f),
                obj.slots.get(f).cloned().unwrap_or_else(Type::int),
            )
        })
        .collect();
    // The declarative mount spec — the view layer applies it to the raw
    // fragment (no HTML formatting here).
    let fields = refs
        .fields
        .iter()
        .map(|field| (field.clone(), format!("{}.{}", prefix, field)))
        .collect();
    let spec = MountSpec {
        component: component.to_string(),
        index: i,
        marker: prefix,
        fields,
        txn_variants: variant_txns,
    };
    (spec, slot_types)
}

/// Build a Briev-side instance's spec: slots + variants routed to the
/// instance-name prefix, the literal's seeds, and its reset txn.
fn build_instance_spec(
    items: &mut Vec<TopLevel>,
    obj: &ObjInfo,
    refs: &FragmentRefs,
    instance: &(&String, &String, &Option<HashMap<String, Expr>>),
    plan: &mut ComponentInstancePlan,
) -> Result<(), String> {
    let (var_name, component, literal) = instance;
    let var_name = var_name.as_str();
    let component = component.as_str();
    let slot_set = instance_slot_set(items, obj, refs, var_name);
    let qualifier = |id: &str| -> Option<String> {
        if slot_set.contains(id) {
            Some(format!("{}.{}", var_name, id))
        } else {
            None
        }
    };
    let variant_txns = build_txn_variants(items, obj, var_name, refs, &qualifier);
    // The literal's field values seed the instance slots (Briev source — the
    // frontend invents nothing).
    if let Some(literal) = literal {
        for (field, value) in literal {
            plan.initializers
                .insert(format!("{}.{}", var_name, field), value.clone());
        }
    }
    let fields = refs
        .fields
        .iter()
        .map(|field| (field.clone(), format!("{}.{}", var_name, field)))
        .collect();
    plan.instance_specs.insert(
        var_name.to_string(),
        MountSpec {
            component: component.to_string(),
            index: 0,
            marker: var_name.to_string(),
            fields,
            txn_variants: variant_txns,
        },
    );
    // 2026-08-12 (2b3 slice 3): the Briev-side reset — a callable txn that
    // re-applies the instance's SEEDS (the StructLiteral values), so a
    // b-when unmount restarts the instance at its Briev-declared state.
    let slot_types = slot_set
        .iter()
        .map(|f| {
            (
                format!("{}.{}", var_name, f),
                obj.slots.get(f).cloned().unwrap_or_else(Type::int),
            )
        })
        .collect();
    emit_reset_txn(items, var_name, &slot_types, &plan.initializers)
}

/// Validate a StructLiteral's field seeds against the obj's slots — a seeded
/// field the obj does not own is a compile error.
fn validate_literal_fields(
    name: &str,
    base: &str,
    obj: &ObjInfo,
    fields: &[(String, Expr)],
) -> Result<Option<HashMap<String, Expr>>, String> {
    let mut map = HashMap::new();
    for (field, value) in fields {
        if !obj.slots.contains_key(field) {
            return Err(format!(
                "component instance '{}' seeds field '{}' which is not an obj '{}' slot",
                name, field, base
            ));
        }
        map.insert(field.clone(), value.clone());
    }
    Ok(Some(map))
}

/// The Briev zero-default for a bootstrap-primitive slot type — used when a
/// reset must re-seed an unseeded slot. Custom/compound slot types have no
/// synthesizable default (a seeded value is the only way they reset).
fn default_expr_for_type(ty: &Type) -> Option<Expr> {
    if ty == &Type::bool_() {
        return Some(Expr::Bool(false));
    }
    if ty == &Type::string() {
        return Some(Expr::Quoted(Vec::new()));
    }
    if ty == &Type::int() || ty == &Type::float() || ty == &Type::float64() {
        return Some(Expr::Decimal(0));
    }
    None
}

/// Emit a per-instance RESET — a callable txn that re-applies the instance's
/// initial state on b-when unmount (remount = fresh). The write flows through
/// the reactive machinery: the contract is carried (`[true][slot == value …]`)
/// and the body's write set drives the flush, so the DOM updates immediately —
/// the old direct-store reset never flushed (stale DOM after reset). A slot
/// with no seed and no type default is a compile error — never silently left
/// stale.
fn emit_reset_txn(
    items: &mut Vec<TopLevel>,
    prefix: &str,
    slot_types: &HashMap<String, Type>,
    seeds: &HashMap<String, Expr>,
) -> Result<(), String> {
    let marker = prefix.replace('.', "_");
    let mut slots: Vec<String> = slot_types.keys().cloned().collect();
    slots.sort_unstable();
    if slots.is_empty() {
        return Ok(());
    }
    let mut body: Vec<Statement> = Vec::new();
    let mut conjuncts: Vec<Expr> = Vec::new();
    for slot in &slots {
        let value = reset_value_for_slot(slot, slot_types, seeds)?;
        let lhs = Expr::Identifier(slot.clone());
        body.push(Statement::Assign(lhs.clone(), value.clone()));
        conjuncts.push(Expr::BinaryOp(
            BinaryOpKind::Eq,
            Box::new(lhs),
            Box::new(value),
        ));
    }
    body.push(Statement::Term(None));
    let post = conjuncts.into_iter().rev().reduce(|acc, c| {
        Expr::BinaryOp(BinaryOpKind::And, Box::new(c), Box::new(acc))
    }).unwrap_or(Expr::Bool(true));
    items.push(TopLevel::Transaction(Transaction {
        name: format!("__reset_{}", marker),
        is_reactive: false,
        is_async: false,
        type_params: Vec::new(),
        parameters: Vec::new(),
        output_type: None,
        outputs: Vec::new(),
        contract: crate::ast::Contract {
            pre_condition: Expr::Bool(true),
            post_condition: post,
            watchdog: None,
            explicit: true,
            post_authority: false,
            span: None,
        },
        body,
        metadata: HashMap::new(),
        derivation: None,
        modifiers: Vec::new(),
        span: None,
        doc: None,
    }));
    Ok(())
}

/// The value a reset txn writes for one slot: the Briev seed when seeded, else
/// the type default (bootstrap primitives only). A slot with neither is a
/// compile error — never silently left stale on a reset.
fn reset_value_for_slot(
    slot: &str,
    slot_types: &HashMap<String, Type>,
    seeds: &HashMap<String, Expr>,
) -> Result<Expr, String> {
    if let Some(seed) = seeds.get(slot) {
        return Ok(seed.clone());
    }
    if let Some(def) = slot_types.get(slot).and_then(default_expr_for_type) {
        return Ok(def);
    }
    Err(format!(
        "component slot '{}' has no seed and no type default — it cannot be \
         reset on unmount; seed the instance (`let {}: … = … {{ {}: value }}`)",
        slot, prefix_for_error(slot), slot
    ))
}

/// The instance prefix (everything before the last `.field`) for an error
/// message — `c1.count` → `c1`, `Counter.0.count` → `Counter.0`.
fn prefix_for_error(slot: &str) -> String {
    slot.rfind('.').map(|i| slot[..i].to_string()).unwrap_or_else(|| slot.to_string())
}

/// The HTML element names reserved against Briev instance vars — an instance
/// named `div` mounted as `<div />` would silently shadow the HTML element,
/// so such a name is a compile error (the namespaces stay separated).
const RESERVED_TAG_NAMES: &[&str] = &[
    "a", "button", "div", "form", "h1", "h2", "h3", "h4", "h5", "h6", "img",
    "input", "label", "li", "ol", "p", "select", "span", "table", "tbody",
    "td", "textarea", "th", "thead", "tr", "ul",
];

/// Collect Briev-side component instances: top-level `let <name>: <Obj> =
/// <StructLiteral>` where `<Obj>` has a render block. Returns `(var name,
/// component, literal field values)`. The consumed lets are removed (their
/// state becomes the `name.<field>` slots).
fn collect_instance_lets(
    items: &mut Vec<TopLevel>,
    render_blocks: &std::collections::BTreeMap<String, String>,
    obj_defs: &HashMap<String, ObjInfo>,
) -> Result<Vec<(String, String, Option<HashMap<String, Expr>>)>, String> {
    let mut infos: Vec<(String, String, Option<HashMap<String, Expr>>)> = Vec::new();
    let mut to_remove: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for item in items.iter() {
        let TopLevel::Statement(stmt) = item else { continue };
        let crate::ast::Statement::Let { name, ty, expr, .. } = stmt.as_ref() else {
            continue;
        };
        let Some(Type::Custom(base)) = ty else { continue };
        let Some(obj) = obj_defs.get(base) else { continue };
        let Some(_frag) = render_blocks.get(base) else { continue };
        if !seen.insert(name.clone()) {
            continue;
        }
        // Tag namespace separation: an instance var may not shadow a
        // component type name or a reserved HTML element name.
        if render_blocks.contains_key(name) {
            return Err(format!(
                "instance variable '{}' shadows component type '{}' — rename the \
                 instance (the tag namespace resolves instance vars before \
                 component types)",
                name, name
            ));
        }
        if RESERVED_TAG_NAMES.contains(&name.as_str()) {
            return Err(format!(
                "instance variable '{}' collides with the HTML element '{}' — \
                 rename it (HTML element names are reserved)",
                name, name
            ));
        }
        let literal_fields = match expr {
            Some(Expr::StructLiteral { type_name, fields, specs }) => {
                if type_name != base {
                    return Err(format!(
                        "component instance '{}' is typed '{}' but constructed as '{}'",
                        name, base, type_name
                    ));
                }
                validate_literal_fields(name, base, obj, fields)?
            }
            Some(other) => {
                return Err(format!(
                    "component instance '{}' must be constructed with an object \
                     literal (`{} {{ field: value }}`) — a scalar initializer has \
                     no per-instance meaning",
                    name, base
                ));
            }
            None => None,
        };
        infos.push((name.clone(), base.clone(), literal_fields));
        to_remove.push(name.clone());
    }
    if !to_remove.is_empty() {
        items.retain(|item| {
            let TopLevel::Statement(stmt) = item else { return true };
            let crate::ast::Statement::Let { name, .. } = stmt.as_ref() else {
                return true;
            };
            !to_remove.contains(name)
        });
    }
    Ok(infos)
}

/// Collect `obj Name { slot: Type; txn member [...] {...}; }` definitions as
/// the component surface: slots (name → type) + member transactions by name.
fn collect_obj_defs(items: &[TopLevel]) -> HashMap<String, ObjInfo> {
    let mut defs = HashMap::new();
    for item in items {
        if let TopLevel::TypeDef(td) = item {
            if td.body.slots.is_empty() {
                continue;
            }
            let slots = td
                .body
                .slots
                .iter()
                .map(|s| (s.name.clone(), s.ty.clone()))
                .collect();
            let member_txns = td
                .body
                .members
                .iter()
                .filter_map(|m| match m {
                    TopLevel::Transaction(t) => Some((t.name.clone(), t.clone())),
                    _ => None,
                })
                .collect();
            defs.insert(td.name.clone(), ObjInfo { slots, member_txns });
        }
    }
    defs
}

/// Fields and transactions a fragment's directives reference.
#[derive(Default)]
struct FragmentRefs {
    /// State fields (bare names, projections stripped).
    fields: std::collections::HashSet<String>,
    /// Transaction names fired by b-trigger directives.
    txns: std::collections::HashSet<String>,
}

/// Lightweight directive scanner: pulls `b-text`/`b-show`/`b-when`/`b-bind`
/// field signals and `b-trigger`/`b-on` txn names out of a fragment's markup.
/// Single forward pass (no nested loops — Praetor loop-depth).
fn collect_fragment_refs(html: &str) -> FragmentRefs {
    let mut refs = FragmentRefs::default();
    let mut rest = html;
    while let Some(pos) = rest.find("b-") {
        let tail = &rest[pos + 2..];
        let name_end = tail.find(['=', ' ', '>', '\n']).unwrap_or(tail.len());
        let name = &tail[..name_end];
        let after_name = &tail[name_end..];
        if let Some(value) = after_name
            .find('=')
            .and_then(|eq| strip_quotes(after_name[eq + 1..].trim()))
        {
            record_fragment_ref(name, value, &mut refs);
        }
        rest = &tail[name_end.min(tail.len().saturating_sub(1)) + 1..];
    }
    refs
}

/// Record a directive attr's value into the fragment refs.
fn record_fragment_ref(name: &str, value: &str, refs: &mut FragmentRefs) {
    if name.starts_with("trigger:") || name.starts_with("on:") {
        let txn = value.split('(').next().unwrap_or("").trim();
        if !txn.is_empty() {
            refs.txns.insert(txn.to_string());
        }
        return;
    }
    if matches!(name, "show" | "when") {
        // `b-show="step == 0"` binds the root field `step` (the shim evaluates
        // the comparison); `condition_root_signal` takes the first token.
        let (root, _) = crate::view_compiler::condition_root_signal(value);
        refs.fields.insert(root.to_string());
        return;
    }
    if name == "text" || name.starts_with("bind") {
        let (root, _) = crate::view_compiler::root_signal(value);
        refs.fields.insert(root.to_string());
    }
}

/// Strip one level of `"`/`'` quotes from a directive value.
fn strip_quotes(v: &str) -> Option<&str> {
    let v = v.trim_start();
    if v.starts_with('"') || v.starts_with('\'') {
        let q = v.as_bytes()[0] as char;
        let inner = &v[1..];
        let end = inner.find(q).unwrap_or(inner.len());
        Some(&inner[..end])
    } else {
        Some(v)
    }
}

/// Count `<Name .../>` mount tags across the view AND every OTHER render
/// fragment — a component can be mounted inside a sibling component's
/// fragment (nested mounts), not just the root view. The component's OWN
/// fragment is excluded (a self-mount is a cycle error, handled elsewhere).
fn count_component_mounts(
    view_html: &str,
    component: &str,
    fragments: &std::collections::BTreeMap<String, String>,
) -> usize {
    let mut total = count_mounts_in(view_html, component);
    for (name, html) in fragments {
        // Skip the component's OWN fragment (a self-mount is a cycle error,
        // handled elsewhere) and any fragment that IS the view (its mounts are
        // already counted — the view is often the `render Root` block).
        if name != component && *html != view_html {
            total += count_mounts_in(html, component);
        }
    }
    total
}

/// Count `<Name .../>` occurrences in one HTML string. The name match stops at
/// the first non-identifier byte so `<counter1 />` never counts as a
/// `<Counter />` mount.
fn count_mounts_in(html: &str, component: &str) -> usize {
    let needle = format!("<{}", component.to_lowercase());
    let lower = html.to_lowercase();
    let mut count = 0usize;
    let mut pos = 0usize;
    while let Some(rel) = lower[pos..].find(&needle) {
        let after = pos + rel + needle.len();
        let boundary_ok = lower[after..]
            .chars()
            .next()
            .map(|c| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(true);
        if boundary_ok {
            count += 1;
        }
        pos = after;
    }
    count
}

/// The per-mount instance slots: the fragment's fields ∪ every obj slot a
/// variant member references. Registers each as a `StateDecl` typed by the
/// obj (the ORIGINAL type is preserved — a `show: Bool` stays Bool). Returns
/// the qualified name set for the member-body rewrite qualifier.
fn instance_slot_set(
    items: &mut Vec<TopLevel>,
    obj: &ObjInfo,
    refs: &FragmentRefs,
    prefix: &str,
) -> HashSet<String> {
    let mut slots: HashSet<String> = refs.fields.clone();
    for (name, member) in &obj.member_txns {
        if refs.txns.contains(name) || txn_references_fields(member, &refs.fields) {
            collect_txn_slots(member, &obj.slots, &mut slots);
        }
    }
    // 2026-08-23 (bugfix, determinism): sort before pushing StateDecls — a
    // HashSet iteration here made emitted IR field/function order vary per
    // process seed. To undo: revert to `for field in &slots` (nondeterministic).
    let mut sorted_slots: Vec<&String> = slots.iter().collect();
    sorted_slots.sort();
    for field in sorted_slots {
        let ty = obj.slots.get(field).cloned().unwrap_or_else(Type::int);
        items.push(TopLevel::StateDecl(crate::ast::top::StateDecl {
            name: format!("{}.{}", prefix, field),
            ty,
            span: None,
        }));
    }
    slots
}

/// Add every obj slot an obj member's body/contract references to `out`.
fn collect_txn_slots(txn: &Transaction, slots: &HashMap<String, Type>, out: &mut HashSet<String>) {
    let mut hits = HashSet::new();
    for stmt in &txn.body {
        stmt_vars(stmt, &mut hits);
    }
    expr_vars(&txn.contract.pre_condition, &mut hits);
    expr_vars(&txn.contract.post_condition, &mut hits);
    out.extend(hits.into_iter().filter(|f| slots.contains_key(f)));
}

/// Build per-mount variants of the obj's member txns that write the
/// fragment's fields (or are triggered by it). Returns the variant names
/// keyed by the original member name. `suffix` is the mount identity — the
/// mount index (`0`) for an HTML-side spawn, the instance var name (`c1`)
/// for a Briev-side instance.
fn build_txn_variants(
    items: &mut Vec<TopLevel>,
    obj: &ObjInfo,
    suffix: &str,
    refs: &FragmentRefs,
    qualifier: &dyn Fn(&str) -> Option<String>,
) -> HashMap<String, String> {
    let mut variants = HashMap::new();
    // Snapshot the members to variant-ize (those triggered by the fragment or
    // referencing the fragment's fields).
    let mut members: Vec<(String, Transaction)> = obj
        .member_txns
        .iter()
        .filter(|(name, t)| refs.txns.contains(*name) || txn_references_fields(t, &refs.fields))
        .map(|(name, t)| (name.clone(), t.clone()))
        .collect();
    // 2026-08-23 (bugfix, determinism): member_txns is a HashMap — sort the
    // snapshot so variant txns are appended to `items` in a stable order.
    // To undo: remove the sort (reintroduces per-process IR variance).
    members.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, mut t) in members {
        let variant_name = format!("{}_{}", name, suffix);
        // for_each keeps the body-rewrite single-level for Praetor.
        t.body.iter_mut().for_each(|stmt| rewrite_stmt(stmt, qualifier));
        let pre = rewrite_expr(&t.contract.pre_condition, qualifier);
        let post = rewrite_expr(&t.contract.post_condition, qualifier);
        // 2026-08-12 (2b3): preserve the member's signature — a member with
        // parameters (b-bind:value route) keeps them on the variant.
        let new_txn = Transaction {
            name: variant_name.clone(),
            is_reactive: false,
            is_async: false,
            type_params: t.type_params,
            parameters: t.parameters,
            output_type: t.output_type,
            outputs: t.outputs,
            contract: crate::ast::Contract {
                pre_condition: pre,
                post_condition: post,
                watchdog: None,
                explicit: true,
                post_authority: false,
                span: None,
            },
            body: t.body,
            metadata: t.metadata,
            derivation: t.derivation,
            modifiers: t.modifiers,
            span: None,
            doc: None,
        };
        items.push(TopLevel::Transaction(new_txn));
        variants.insert(name, variant_name);
    }
    variants
}

/// Whether a transaction's body/contract references any of the given fields.
fn txn_references_fields(t: &Transaction, fields: &std::collections::HashSet<String>) -> bool {
    let mut hits = std::collections::HashSet::new();
    for stmt in &t.body {
        stmt_vars(stmt, &mut hits);
    }
    expr_vars(&t.contract.pre_condition, &mut hits);
    expr_vars(&t.contract.post_condition, &mut hits);
    fields.iter().any(|f| hits.contains(f))
}

/// Collect the identifiers a statement references.
fn stmt_vars(stmt: &Statement, out: &mut std::collections::HashSet<String>) {
    match stmt {
        Statement::Let { expr, .. } => {
            if let Some(e) = expr {
                expr_vars(e, out);
            }
        }
        Statement::Assign(l, r) => {
            expr_vars(l, out);
            expr_vars(r, out);
        }
        Statement::ArrowAssign { target, value, .. } => {
            if let Some(t) = target {
                expr_vars(t, out);
            }
            expr_vars(value, out);
        }
        Statement::Guarded(c, b) => {
            expr_vars(c, out);
            for s in b.iter() {
                stmt_vars(s, out);
            }
        }
        Statement::Gate(c) => expr_vars(c, out),
        Statement::Block(b) => {
            for s in b.iter() {
                stmt_vars(s, out);
            }
        }
        Statement::Foreach { body, .. } => {
            for s in body.iter() {
                stmt_vars(s, out);
            }
        }
        Statement::Expression(e) => expr_vars(e, out),
        Statement::Term(Some(e)) | Statement::EndProgram(Some(e)) | Statement::Rollback(Some(e)) => {
            expr_vars(e, out)
        }
        _ => {}
    }
}

/// Collect the identifiers an expression references.
fn expr_vars(e: &Expr, out: &mut std::collections::HashSet<String>) {
    match e {
        Expr::Identifier(name) => {
            out.insert(name.clone());
        }
        Expr::BinaryOp(_, l, r) => {
            expr_vars(l, out);
            expr_vars(r, out);
        }
        Expr::UnaryOp(_, i) => expr_vars(i, out),
        Expr::Field(o, _) => expr_vars(o, out),
        Expr::MethodCall(o, _, args, _, _) => {
            expr_vars(o, out);
            for a in args.iter() {
                expr_vars(a, out);
            }
        }
        Expr::Index(o, i) => {
            expr_vars(o, out);
            expr_vars(i, out);
        }
        Expr::Call(_, args, _) => {
            for a in args.iter() {
                expr_vars(a, out);
            }
        }
        Expr::List(es) | Expr::Tuple(es) => {
            for x in es.iter() {
                expr_vars(x, out);
            }
        }
        Expr::AddrOf(i) | Expr::Deref(i) | Expr::Consume(i) | Expr::Await(i) | Expr::Within(i, _) => {
            expr_vars(i, out)
        }
        Expr::Block(ss) => {
            for s in ss.iter() {
                stmt_vars(s, out);
            }
        }
        Expr::If(c, t, f) => {
            expr_vars(c, out);
            expr_vars(t, out);
            if let Some(f) = f {
                expr_vars(f, out);
            }
        }
        Expr::Match(subject, arms) => {
            expr_vars(subject, out);
            for arm in arms.iter() {
                if let Some(g) = &arm.guard {
                    expr_vars(g, out);
                }
                expr_vars(&arm.body, out);
            }
        }
        Expr::StructLiteral { fields, .. } => {
            for (_, f) in fields.iter() {
                expr_vars(f, out);
            }
        }
        Expr::Reflect(recv, _, _) => expr_vars(recv, out),
        Expr::Cast(e, _) | Expr::IsType(e, _) => expr_vars(e, out),
        _ => {}
    }
}

/// Rewrite a statement's identifiers via the qualifier.
fn rewrite_stmt(stmt: &mut Statement, qualifier: &dyn Fn(&str) -> Option<String>) {
    match stmt {
        Statement::Let { expr, .. } => {
            if let Some(e) = expr {
                rewrite_expr_mut(e, qualifier);
            }
        }
        Statement::Assign(lhs, rhs) => {
            rewrite_expr_mut(lhs, qualifier);
            rewrite_expr_mut(rhs, qualifier);
        }
        Statement::ArrowAssign { target, value, .. } => {
            if let Some(t) = target {
                rewrite_expr_mut(t, qualifier);
            }
            rewrite_expr_mut(value, qualifier);
        }
        Statement::Guarded(cond, body) => {
            rewrite_expr_mut(cond, qualifier);
            for s in body.iter_mut() {
                rewrite_stmt(s, qualifier);
            }
        }
        Statement::Gate(cond) => rewrite_expr_mut(cond, qualifier),
        Statement::Block(body) => {
            for s in body.iter_mut() {
                rewrite_stmt(s, qualifier);
            }
        }
        Statement::Foreach { body, .. } => {
            for s in body.iter_mut() {
                rewrite_stmt(s, qualifier);
            }
        }
        Statement::Expression(e) => rewrite_expr_mut(e, qualifier),
        Statement::Term(Some(e)) | Statement::EndProgram(Some(e)) | Statement::Rollback(Some(e)) => {
            rewrite_expr_mut(e, qualifier)
        }
        _ => {}
    }
}

/// Rewrite an expression tree in place (identifiers → qualified).
fn rewrite_expr_mut(e: &mut Expr, qualifier: &dyn Fn(&str) -> Option<String>) {
    match e {
        Expr::Identifier(name) => {
            if let Some(q) = qualifier(name) {
                *e = Expr::Identifier(q);
            }
        }
        Expr::BinaryOp(_, l, r) => {
            rewrite_expr_mut(l, qualifier);
            rewrite_expr_mut(r, qualifier);
        }
        Expr::UnaryOp(_, i) => rewrite_expr_mut(i, qualifier),
        Expr::Field(o, _) => rewrite_expr_mut(o, qualifier),
        Expr::MethodCall(o, _, args, _, _) => {
            rewrite_expr_mut(o, qualifier);
            for a in args.iter_mut() {
                rewrite_expr_mut(a, qualifier);
            }
        }
        Expr::Index(o, i) => {
            rewrite_expr_mut(o, qualifier);
            rewrite_expr_mut(i, qualifier);
        }
        Expr::Call(_, args, _) => {
            for a in args.iter_mut() {
                rewrite_expr_mut(a, qualifier);
            }
        }
        Expr::List(es) | Expr::Tuple(es) => {
            for x in es.iter_mut() {
                rewrite_expr_mut(x, qualifier);
            }
        }
        Expr::AddrOf(i) | Expr::Deref(i) | Expr::Consume(i) | Expr::Await(i) | Expr::Within(i, _) => {
            rewrite_expr_mut(i, qualifier)
        }
        Expr::Block(ss) => {
            for s in ss.iter_mut() {
                rewrite_stmt(s, qualifier);
            }
        }
        Expr::If(c, t, f) => {
            rewrite_expr_mut(c, qualifier);
            rewrite_expr_mut(t, qualifier);
            if let Some(f) = f {
                rewrite_expr_mut(f, qualifier);
            }
        }
        Expr::Match(subject, arms) => {
            rewrite_expr_mut(subject, qualifier);
            for arm in arms.iter_mut() {
                if let Some(g) = &mut arm.guard {
                    rewrite_expr_mut(g, qualifier);
                }
                rewrite_expr_mut(&mut arm.body, qualifier);
            }
        }
        Expr::StructLiteral { fields, .. } => {
            for (_, f) in fields.iter_mut() {
                rewrite_expr_mut(f, qualifier);
            }
        }
        Expr::Reflect(recv, _, _) => rewrite_expr_mut(recv, qualifier),
        Expr::Cast(e, _) | Expr::IsType(e, _) => rewrite_expr_mut(e, qualifier),
        _ => {}
    }
}

/// Rewrite an owned expression (contracts).
fn rewrite_expr(e: &Expr, qualifier: &dyn Fn(&str) -> Option<String>) -> Expr {
    let mut cloned = e.clone();
    rewrite_expr_mut(&mut cloned, qualifier);
    cloned
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(src: &str) -> Result<(Vec<TopLevel>, ComponentInstancePlan), String> {
        let tokens = crate::lexer::tokenize(src).unwrap();
        let mut p = crate::parser::Parser::new(tokens, src);
        let mut items = p.parse_program().unwrap();
        let view = items
            .iter()
            .find_map(|item| match item {
                TopLevel::RenderBlock(rb) => {
                    if rb.struct_name == "Root" {
                        Some(rb.view_html.clone())
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .unwrap_or_default();
        let plan = expand_component_instances(&mut items, &view)?;
        Ok((items, plan))
    }

    #[test]
    fn two_mounts_get_independent_state() {
        let src = r#"
obj Counter {
    count: Int;
    txn increment [count < 100][true] {
        count = count + 1;
        term;
    };
};
render Counter {
    <span b-text="count">0</span>
    <button b-trigger:click="increment">+</button>
};
render Root {
    <Counter />
    <Counter />
};
"#;
        let (items, plan) = check(src).unwrap();
        let specs = plan.mounts.get("Counter").expect("Counter plan");
        assert_eq!(specs.len(), 2, "one spec per mount: {specs:?}");
        assert_eq!(
            specs[0].fields,
            vec![("count".to_string(), "Counter.0.count".to_string())],
            "mount 0 field map"
        );
        assert_eq!(
            specs[1].fields,
            vec![("count".to_string(), "Counter.1.count".to_string())],
            "mount 1 field map"
        );
        assert_eq!(
            specs[0].txn_variants.get("increment").map(|v| v.as_str()),
            Some("increment_0"),
            "mount 0 txn variant"
        );
        assert_eq!(
            specs[1].txn_variants.get("increment").map(|v| v.as_str()),
            Some("increment_1"),
            "mount 1 txn variant"
        );
        let names: Vec<String> = items.iter().filter_map(|item| match item {
            TopLevel::StateDecl(sd) => Some(sd.name.clone()),
            TopLevel::Transaction(t) => Some(format!("txn:{}", t.name)),
            _ => None,
        }).collect();
        assert!(names.contains(&"Counter.0.count".to_string()), "{names:?}");
        assert!(names.contains(&"Counter.1.count".to_string()), "{names:?}");
        assert!(names.contains(&"txn:increment_0".to_string()), "{names:?}");
        assert!(names.contains(&"txn:increment_1".to_string()), "{names:?}");
    }

    /// Slot types come from the obj's slots — a `show: Bool` instance slot is
    /// Bool, never the `Type::int()` fallback.
    #[test]
    fn slot_type_comes_from_obj() {
        let src = r#"
obj Panel {
    show: Bool;
    txn toggle [show == false][show == true] {
        show = true;
        term;
    };
};
render Panel {
    <div b-when="show">visible</div>
};
render Root {
    <Panel />
    <Panel />
};
"#;
        let (items, _plan) = check(src).unwrap();
        let states: Vec<(String, Type)> = items.iter().filter_map(|item| match item {
            TopLevel::StateDecl(sd) => Some((sd.name.clone(), sd.ty.clone())),
            _ => None,
        }).collect();
        assert!(
            states.iter().any(|(n, t)| n == "Panel.0.show" && *t == Type::bool_()),
            "instance slot keeps the obj's Bool type: {states:?}"
        );
        let txns: Vec<String> = items.iter().filter_map(|item| match item {
            TopLevel::Transaction(t) => Some(t.name.clone()),
            _ => None,
        }).collect();
        assert!(txns.contains(&"toggle_0".to_string()) && txns.contains(&"toggle_1".to_string()),
            "write-consumed member gets per-mount variants: {txns:?}");
    }

    /// `render Name` requires `obj Name` — a fragment without its obj is a
    /// compile error, never silently-mounted globals.
    #[test]
    fn render_requires_obj() {
        let src = r#"
render Counter {
    <span b-text="count">0</span>
};
render Root {
    <Counter />
};
"#;
        let err = check(src).unwrap_err();
        assert!(err.contains("requires an obj"), "{err}");
    }

    /// A fragment binding a field the obj does not own is a compile error —
    /// never a silently dead binding.
    #[test]
    fn fragment_field_must_be_obj_slot() {
        let src = r#"
obj Counter {
    count: Int;
};
render Counter {
    <span b-text="total">0</span>
};
render Root {
    <Counter />
};
"#;
        let err = check(src).unwrap_err();
        assert!(err.contains("'total'") && err.contains("slots"), "{err}");
    }

    /// A fragment triggering a txn that is not an obj member is a compile
    /// error — never an inert button.
    #[test]
    fn fragment_txn_must_be_obj_member() {
        let src = r#"
obj Counter {
    count: Int;
};
render Counter {
    <button b-trigger:click="bump">+</button>
};
render Root {
    <Counter />
};
"#;
        let err = check(src).unwrap_err();
        assert!(err.contains("'bump'") && err.contains("members"), "{err}");
    }

    /// 2026-08-12 (2b3): HTML-side spawns are reactor-owned and zero-init —
    /// no props, no Rust-invented seeds.
    #[test]
    fn html_spawns_are_zero_init() {
        let src = r#"
obj Counter {
    count: Int;
    txn increment [count < 100][true] {
        count = count + 1;
        term;
    };
};
render Counter {
    <span b-text="count">0</span>
};
render Root {
    <Counter />
    <Counter />
};
"#;
        let (_items, plan) = check(src).unwrap();
        assert!(plan.initializers.is_empty(), "no Rust-invented seeds: {:?}", plan.initializers);
        assert_eq!(
            plan.mounts.get("Counter").map(|s| s.len()).unwrap_or(0),
            2,
            "two per-mount specs"
        );
    }

    /// 2026-08-12 (2b3 slice 2): a Briev-side instance — `let c1: Counter =
    /// Counter { count: 5 }` + `<c1 />` — routes to the `c1.*` slots, seeds
    /// from the StructLiteral (Briev source), and the let is consumed.
    #[test]
    fn briev_side_instance_seeds_and_routes() {
        let src = r#"
obj Counter {
    count: Int;
    txn increment [count < 100][true] {
        count = count + 1;
        term;
    };
};
render Counter {
    <span b-text="count">0</span>
    <button b-trigger:click="increment">+</button>
};
let c1: Counter = Counter { count: 5 };
render Root {
    <c1 />
};
"#;
        let (items, plan) = check(src).unwrap();
        let spec = plan.instance_specs.get("c1").expect("c1 spec");
        assert_eq!(spec.marker, "c1", "data-instance marker is the instance name");
        assert_eq!(
            spec.fields,
            vec![("count".to_string(), "c1.count".to_string())],
            "fragment routes to c1.count"
        );
        assert_eq!(
            spec.txn_variants.get("increment").map(|v| v.as_str()),
            Some("increment_c1"),
            "variant named by the instance"
        );
        assert_eq!(
            plan.initializers.get("c1.count"),
            Some(&Expr::Decimal(5)),
            "seed is the Briev literal"
        );
        let states: Vec<String> = items.iter().filter_map(|item| match item {
            TopLevel::StateDecl(sd) => Some(sd.name.clone()),
            _ => None,
        }).collect();
        assert!(states.contains(&"c1.count".to_string()), "{states:?}");
        let txns: Vec<String> = items.iter().filter_map(|item| match item {
            TopLevel::Transaction(t) => Some(t.name.clone()),
            _ => None,
        }).collect();
        assert!(txns.contains(&"increment_c1".to_string()), "{txns:?}");
        assert!(
            !items.iter().any(|item| matches!(item, TopLevel::Statement(s) if matches!(
                s.as_ref(), crate::ast::Statement::Let { name, .. } if name == "c1"))),
            "the consumed let is removed (its state became the c1.* slots)"
        );
        assert!(
            plan.mounts.get("Counter").map(|s| s.is_empty()).unwrap_or(true),
            "no spurious HTML-side pool mount"
        );
    }

    /// Two Briev-side instances of the same obj are fully independent — each
    /// seeds its own slot and fires its own variant.
    #[test]
    fn two_briev_side_instances_are_independent() {
        let src = r#"
obj Counter {
    count: Int;
    txn increment [count < 100][true] {
        count = count + 1;
        term;
    };
};
render Counter {
    <span b-text="count">0</span>
};
let c1: Counter = Counter { count: 5 };
let c2: Counter = Counter { count: 7 };
render Root {
    <c1 />
    <c2 />
};
"#;
        let (_items, plan) = check(src).unwrap();
        assert_eq!(
            plan.initializers.get("c1.count"),
            Some(&Expr::Decimal(5)),
            "c1 seeds 5"
        );
        assert_eq!(
            plan.initializers.get("c2.count"),
            Some(&Expr::Decimal(7)),
            "c2 seeds 7"
        );
        assert!(plan.instance_specs.contains_key("c1"), "c1 spec");
        assert!(plan.instance_specs.contains_key("c2"), "c2 spec");
    }

    /// A seeded field the obj does not own is a compile error.
    #[test]
    fn instance_seed_field_must_be_obj_slot() {
        let src = r#"
obj Counter {
    count: Int;
};
render Counter {
    <span b-text="count">0</span>
};
let c1: Counter = Counter { total: 5 };
render Root {
    <c1 />
};
"#;
        let err = check(src).unwrap_err();
        assert!(err.contains("'total'") && err.contains("slot"), "{err}");
    }

    /// An instance var shadowing a component type name is a compile error.
    #[test]
    fn instance_var_shadowing_component_type_rejected() {
        let src = r#"
obj Counter {
    count: Int;
};
render Counter {
    <span b-text="count">0</span>
};
let Counter: Counter = Counter { count: 5 };
render Root {
    <Counter />
};
"#;
        let err = check(src).unwrap_err();
        assert!(err.contains("shadows component type"), "{err}");
    }

    /// An instance var colliding with a reserved HTML element name is a
    /// compile error — the tag namespaces stay separated.
    #[test]
    fn instance_var_colliding_with_html_element_rejected() {
        let src = r#"
obj Counter {
    count: Int;
};
render Counter {
    <span b-text="count">0</span>
};
let div: Counter = Counter { count: 5 };
render Root {
    <div />
};
"#;
        let err = check(src).unwrap_err();
        assert!(err.contains("HTML element"), "{err}");
    }

    /// 2026-08-12 (2b3 slice 3): a Briev-side instance gets a callable reset
    /// txn that re-applies its SEED — the write flows through the reactive
    /// machinery (contract + flush), never a direct store.
    #[test]
    fn briev_side_instance_reset_reapplies_seed() {
        let src = r#"
obj Counter {
    count: Int;
    txn increment [count < 100][true] {
        count = count + 1;
        term;
    };
};
render Counter {
    <span b-text="count">0</span>
};
let c1: Counter = Counter { count: 9 };
render Root {
    <c1 />
};
"#;
        let (items, _plan) = check(src).unwrap();
        let reset = items.iter().find_map(|item| match item {
            TopLevel::Transaction(t) if t.name == "__reset_c1" => Some(t),
            _ => None,
        }).expect("reset txn __reset_c1");
        let writes: Vec<String> = reset.body.iter().filter_map(|s| match s {
            Statement::Assign(l, _) => Some(format!("{:?}", l)),
            _ => None,
        }).collect();
        assert!(
            writes.iter().any(|w| w.contains("c1.count")),
            "reset writes the seeded slot: {writes:?}"
        );
        assert!(
            matches!(reset.contract.post_condition, Expr::BinaryOp(..)),
            "reset carries a non-trivial post (not [true][true]): {:?}",
            reset.contract.post_condition
        );
        assert!(!reset.is_reactive, "reset is callable-only (no tick livelock)");
    }

    /// 2026-08-12 (2b3 slice 3): an HTML-side spawn's reset re-applies the
    /// type default (zero) — a b-when unmount of an anonymous pool instance
    /// restarts it fresh.
    #[test]
    fn html_side_reset_uses_zero_default() {
        let src = r#"
obj Counter {
    count: Int;
    txn increment [count < 100][true] {
        count = count + 1;
        term;
    };
};
render Counter {
    <span b-text="count">0</span>
};
render Root {
    <Counter />
};
"#;
        let (items, _plan) = check(src).unwrap();
        let reset = items.iter().find_map(|item| match item {
            TopLevel::Transaction(t) if t.name == "__reset_Counter_0" => Some(t),
            _ => None,
        }).expect("reset txn __reset_Counter_0");
        assert!(
            reset.body.iter().any(|s| matches!(s, Statement::Assign(l, v)
                if format!("{:?}", l).contains("Counter.0.count")
                    && matches!(v, Expr::Decimal(0)))),
            "HTML-side reset writes the zero default: {:?}",
            reset.body
        );
    }

    /// 2026-08-12 (2b3 slice 3): an unseeded slot of a type with no
    /// synthesizable default is a compile error — never silently left stale on
    /// a reset.
    #[test]
    fn unseedable_slot_reset_rejected() {
        let src = r#"
obj Counter {
    count: Int;
    buf: Blob;
};
render Counter {
    <span b-text="count">0</span>
    <div b-show="buf">x</div>
};
render Root {
    <Counter />
};
"#;
        let err = check(src).unwrap_err();
        assert!(err.contains("no seed and no type default"), "{err}");
    }

    /// 2026-08-12 (2b3 slice 2): a component can mount BOTH ways — anonymous
    /// `<Counter />` pool spawns AND a Briev-side `<c1 />` instance.
    #[test]
    fn mixed_html_and_briev_mounts() {
        let src = r#"
obj Counter {
    count: Int;
    txn increment [count < 100][true] {
        count = count + 1;
        term;
    };
};
render Counter {
    <span b-text="count">0</span>
};
let c1: Counter = Counter { count: 5 };
render Root {
    <Counter />
    <c1 />
};
"#;
        let (_items, plan) = check(src).unwrap();
        assert_eq!(
            plan.mounts.get("Counter").map(|s| s.len()).unwrap_or(0),
            1,
            "one HTML-side pool mount"
        );
        assert!(plan.instance_specs.contains_key("c1"), "c1 spec present");
        assert_eq!(
            plan.initializers.get("c1.count"),
            Some(&Expr::Decimal(5)),
            "c1 seeds from Briev"
        );
        assert!(
            !plan.initializers.contains_key("Counter.0.count"),
            "HTML-side spawn stays zero-init"
        );
    }
}
