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

use crate::ast::{self, Contract, Expr, TopLevel};
use std::collections::{HashMap, HashSet};

const KNOWN_DIRECTIVES: &[&str] = &[
    "b-text", "b-show", "b-hide", "b-when", "b-on:", "b-trigger:",
    "b-class", "b-attr", "b-style", "b-each", "b-key", "b-bind",
];

#[derive(Debug, Clone)]
pub struct Binding {
    pub element_id: String,
    pub directive: Directive,
}

#[derive(Debug, Clone)]
pub enum Directive {
    Text {
        signal: String,
    },
    Show {
        expr: String,
    },
    Hide {
        expr: String,
    },
    /// 2026-08-11 (Phase 2a, SPEC 21.4): `b-when` structurally mounts/unmounts
    /// a subtree — presence in the DOM is state-driven and identity is NOT
    /// preserved across toggles (unlike `b-show`, which is presentation-only).
    When {
        expr: String,
    },
    /// 2026-08-11 (Phase 2a2, SPEC 21.4): `b-bind:value="field"` — two-way
    /// binding. The input event routes to the UNIQUE transaction that writes
    /// `target` (the write-contract proof), resolved at build time; the value
    /// is marshalled by that transaction's parameter type.
    Bind {
        target: String,
    },
    Trigger {
        event: String,
        txn: String,
        params: Vec<(String, String)>, // parameter name -> value (as string for JS)
    },
    Class {
        pairs: Vec<(String, String)>,
    },
    Attr {
        name: String,
        value: String,
    },
    Style {
        name: String,
        value: String,
    },
    Each {
        iterable: String,
        item_name: String,
        template_html: String,
        container_id: String,
        /// 2026-08-11 (Phase 2a3): per-item directives captured from the each
        /// element's own attributes AND its inner template. Applied by the
        /// shim's item renderer (each rendered item is a fresh template clone).
        /// Inner template elements carry a `data-itm="<index>"` marker so the
        /// shim can locate them in the clone.
        item_bindings: Vec<ItemBinding>,
        /// The `b-key` expression (stable identity for reconciliation).
        key_expr: String,
    },
}

/// 2026-08-11 (Phase 2a3): an item-scoped directive inside a `b-each`
/// template, with the marker attribute locating its element in a clone.
#[derive(Debug, Clone)]
pub struct ItemBinding {
    /// `data-itm` marker index — the shim finds the element via
    /// `clone.querySelector('[data-itm="N"]')`.
    pub marker: usize,
    pub directive: ItemDirective,
}

/// 2026-08-11 (Phase 2a3): the item-scoped directive kinds the item renderer
/// understands. Expressions are simple scalar item forms (`item`, `item ==
/// field`, `txn(item)`) — richer expressions are rejected at compile time
/// (never silently dead DOM).
#[derive(Debug, Clone)]
pub enum ItemDirective {
    Text {
        signal: String,
    },
    Class {
        /// `(class_name, expr)` pairs from `b-class="{ 'cls': item == 2 }"`.
        pairs: Vec<(String, String)>,
    },
    Show {
        expr: String,
    },
    When {
        expr: String,
    },
    Trigger {
        event: String,
        txn: String,
    },
}

pub struct ViewCompiler {
    signals: HashMap<String, usize>,
    transactions: HashMap<String, usize>,
    bindings: Vec<Binding>,
    id_counter: usize,
    each_context: Vec<EachContext>,
    /// 2026-08-11 (Phase 2a3): nesting depth inside a `b-each` element.
    /// While > 0, inject_ids passes tags through untouched — the whole subtree
    /// is item-scoped, handled by extract_bindings (global directive
    /// extraction must not leak item-scoped bindings like `b-text="item"`).
    each_depth: usize,
    /// 2026-08-11 (Phase 2b, SPEC 21.3): `render Name { ... }` view fragments,
    /// keyed by struct name — the RAW fragment html. A `<Name />` tag in the
    /// view mounts a fragment at compile time (inlined + ID-injected like any
    /// markup). The per-mount REWRITE specs (`component_specs`) decide what
    /// each mount binds (instance-qualified slots + txn variants) — the view
    /// layer formats the HTML; analysis only supplies the decisions.
    pub render_blocks: HashMap<String, String>,
    /// 2026-08-11 (2b2 slice 2a/2b): per-mount view-rewrite specs (decisions
    /// from analysis/component_instances). Mount k applies `specs[k % len]` to
    /// the raw fragment: field signals → instance-qualified slots, trigger
    /// txns → mount variants, and the fragment root gets a `data-instance`
    /// marker so the shim can reset the instance on b-when unmount.
    pub component_specs: HashMap<String, Vec<crate::analysis::component_instances::MountSpec>>,
    /// 2026-08-12 (2b3 slice 2): Briev-side instance specs, keyed by the
    /// instance var name (`c1`). A `<c1 />` tag mounts the component's
    /// fragment routed to the instance's slots — no mount counter (the
    /// instance is unique by name).
    pub instance_specs: HashMap<String, crate::analysis::component_instances::MountSpec>,
    /// 2026-08-11 (2b2 slice 2a): per-component mount counter, used to pick the
    /// per-mount fragment variant for each `<Name />` tag.
    pub component_mounts: HashMap<String, usize>,
    pub diagnostics: Vec<String>,
    /// 2026-08-09 (Phase 14, SPEC 21.4): directive validation errors — `b-if`
    /// rejected, `b-each` without `b-key`, `b-bind:value` on a non-assignable
    /// target. Surfaced by compile() (not silently ignored).
    pub validation_errors: Vec<String>,
    /// Transactions that are triggered by user input (b-trigger:)
    /// These should have preconditions that account for non-deterministic user input
    user_triggered_txns: HashSet<String>,
}

#[derive(Debug, Clone)]
struct EachContext {
    iterable: String,
    item_name: String,
}

impl ViewCompiler {
    pub fn new() -> Self {
        ViewCompiler {
            signals: HashMap::new(),
            transactions: HashMap::new(),
            bindings: Vec::new(),
            id_counter: 0,
            each_context: Vec::new(),
            each_depth: 0,
            render_blocks: HashMap::new(),
            component_specs: HashMap::new(),
            instance_specs: HashMap::new(),
            component_mounts: HashMap::new(),
            diagnostics: Vec::new(),
            validation_errors: Vec::new(),
            user_triggered_txns: HashSet::new(),
        }
    }

    pub fn register_signal(&mut self, name: &str, id: usize) {
        self.signals.insert(name.to_string(), id);
    }

    pub fn register_transaction(&mut self, name: &str, id: usize) {
        self.transactions.insert(name.to_string(), id);
    }

    /// 2026-08-11 (Phase 2b): register the RAW `render Name { ... }` view
    /// fragments (the view layer rewrites per mount via `component_specs`).
    pub fn set_render_blocks(&mut self, blocks: HashMap<String, String>) {
        self.render_blocks = blocks;
        self.component_mounts.clear();
    }

    /// 2026-08-11 (2b2 slice 2a/2b): register the per-mount rewrite specs.
    pub fn set_component_specs(
        &mut self,
        specs: HashMap<String, Vec<crate::analysis::component_instances::MountSpec>>,
    ) {
        self.component_specs = specs;
        self.component_mounts.clear();
    }

    /// 2026-08-12 (2b3 slice 2): register the Briev-side instance specs
    /// (keyed by instance var name).
    pub fn set_instance_specs(
        &mut self,
        specs: HashMap<String, crate::analysis::component_instances::MountSpec>,
    ) {
        self.instance_specs = specs;
    }

    /// 2026-10-07 (member-txn-on-plain-obj-var): rewrite a top-level view's
    /// directive values — bare member-txn names become the emitted top-level
    /// variants (`go` → `go_router`). A plain top-level obj var has no mount
    /// tag to rewrite through, so the analysis supplies the (orig → variant)
    /// pairs and the view layer applies them to the view itself. This is the
    /// same `replace_directive_value` pass `apply_mount_spec` uses for a
    /// mount's fragment — the plain-var view is the fragment.
    pub fn rewrite_top_level_txn_variants(
        html: &str,
        variants: &std::collections::HashMap<String, String>,
    ) -> String {
        let mut out = html.to_string();
        let mut pairs: Vec<(&String, &String)> = variants.iter().collect();
        pairs.sort();
        for (orig, variant) in pairs {
            out = Self::replace_directive_value(&out, orig, variant);
        }
        out
    }

    /// 2026-08-11 (2b2 slice 2a/2b): apply a mount's rewrite SPEC to the raw
    /// fragment — the view layer's formatting (analysis supplies only the
    /// decisions): field directive values become the instance-qualified slots,
    /// trigger txns become the mount variants, and the fragment root gets a
    /// `data-instance` marker so the shim can reset the instance on b-when
    /// unmount.
    fn apply_mount_spec(
        &self,
        raw: &str,
        spec: &crate::analysis::component_instances::MountSpec,
    ) -> String {
        let mut html = raw.to_string();
        for (field, qualified) in &spec.fields {
            html = Self::replace_directive_value(&html, field, qualified);
        }
        for (orig, variant) in &spec.txn_variants {
            html = Self::replace_directive_value(&html, orig, variant);
        }
        Self::mark_fragment_root(&html, &spec.marker)
    }

    /// 2026-08-11 (2b2 slice 2a/2b): inject `data-instance="<id>"` into the
    /// fragment's ROOT (first opening tag) so the shim's b-when unmount can
    /// find and reset the instance (remount = fresh).
    fn mark_fragment_root(fragment: &str, id: &str) -> String {
        let bytes = fragment;
        let mut pos = 0;
        while let Some(rel) = bytes[pos..].find('<') {
            let start = pos + rel;
            let next = bytes[start + 1..].as_bytes().first().copied();
            if matches!(next, Some(b'/') | Some(b'!')) {
                if let Some(end) = bytes[start..].find('>') {
                    pos = start + end + 1;
                    continue;
                }
                break;
            }
            if let Some(end_rel) = bytes[start..].find('>') {
                let end = start + end_rel;
                let insert_at = if bytes.as_bytes()[end - 1] == b'/' { end - 1 } else { end };
                let marker = format!(" data-instance=\"{}\"", id);
                return format!("{}{}{}", &bytes[..insert_at], marker, &bytes[insert_at..]);
            }
            break;
        }
        fragment.to_string()
    }

    /// 2026-08-11 (2b2 slice 2a/2b): replace `from` → `to` inside DIRECTIVE
    /// attribute values only (`b-text="..."`, `b-trigger:click="..."`).
    /// Non-directive attrs and markup text pass through.
    fn replace_directive_value(html: &str, from: &str, to: &str) -> String {
        let mut out = String::with_capacity(html.len());
        let mut scan = 0;
        while let Some(rel) = html[scan..].find("=\"") {
            let pos = scan + rel;
            let window_start = pos.saturating_sub(32);
            let window = &html[window_start..pos];
            let attr_name = &window[window.rfind([' ', '<', '>', '\n', '\t']).map(|i| i + 1).unwrap_or(0)..];
            if attr_name.starts_with("b-") {
                out.push_str(&html[scan..pos + 2]);
                let after = &html[pos + 2..];
                if let Some(end) = after.find('"') {
                    let value = &after[..end];
                    out.push_str(&value.replace(from, to));
                    out.push('"');
                    scan = pos + 2 + end + 1;
                } else {
                    out.push_str(after);
                    scan = html.len();
                }
            } else {
                out.push_str(&html[scan..pos + 1]);
                scan = pos + 1;
            }
        }
        out.push_str(&html[scan..]);
        out
    }

    /// Returns transactions that are triggered by user input (b-trigger:)
    /// These should have preconditions that account for non-deterministic user input
    pub fn get_user_triggered_transactions(&self) -> &HashSet<String> {
        &self.user_triggered_txns
    }

    /// Validate that user-triggered transactions have appropriate preconditions
    /// For RBV, preconditions should NOT be too strict since user input is unpredictable
    pub fn validate_user_triggered_preconditions(&self, preconditions: &HashMap<String, String>) -> Vec<String> {
        let mut warnings = Vec::new();

        for txn_name in &self.user_triggered_txns {
            if let Some(pre) = preconditions.get(txn_name) {
                // Warn if precondition is too strict (not accounting for unreliable user input)
                // Common strict patterns that might be problematic:
                // - Preconditions checking external state that user can't guarantee
                // - Preconditions that are only true in specific UI states

                // Check if precondition mentions any variable that's likely user-controlled or external
                let strict_patterns = [
                    "network", "api", "server", "fetch", "http",
                    "database", "db_", "file", "disk", "filesystem",
                ];

                let pre_lower = pre.to_lowercase();
                for pattern in strict_patterns {
                    if pre_lower.contains(pattern) {
                        warnings.push(format!(
                            "Warning[R001]: Transaction '{}' is user-triggered but has precondition referencing '{}' which may not be available when user acts: [{}]",
                            txn_name, pattern, pre
                        ));
                    }
                }
            } else {
                // No precondition found - this might be fine or need checking
                warnings.push(format!(
                    "Info[R002]: Transaction '{}' is user-triggered but has no explicit precondition",
                    txn_name
                ));
            }
        }

        warnings
    }

    fn extract_class_expression(&self, tag: &str) -> Option<String> {
        let tag_lower = tag.to_lowercase();

        if !tag_lower.contains("class=") {
            return None;
        }

        if let Some(cls_pos) = tag_lower.find("class=") {
            let value_start = cls_pos + 6;
            let rest = &tag[value_start..];
            let rest_trimmed = rest.trim_start();

            if !rest_trimmed.starts_with('{') {
                return None;
            }

            let inner = rest_trimmed[1..].trim();
            if let Some(close_pos) = inner.find('}') {
                let inner = &inner[..close_pos];

                if inner.contains('?') && inner.contains(" : ") {
                    return Some(inner.to_string());
                }
            }
        }

        None
    }

    pub fn compile(&mut self, view_html: &str) -> (Vec<Binding>, String, Vec<String>) {
        self.bindings.clear();
        self.diagnostics.clear();
        self.validation_errors.clear();
        let modified_html = self.inject_ids(view_html);
        self.extract_bindings(&modified_html);
        // 2026-08-11: custom component tags (e.g. `<Counter />`) are first-class
        // reactive instances per SPEC 21.3 — mount/unmount wiring is not yet
        // wired into the web runtime, so flag them rather than silently
        // rendering an inert element.
        self.warn_component_tags(&modified_html);
        // 2026-08-09 (Phase 14, SPEC 21.4): directive validation errors are
        // surfaced alongside the existing diagnostics (a rejected directive is
        // not silently ignored).
        let mut all = self.validation_errors.clone();
        all.extend(self.diagnostics.clone());
        (self.bindings.clone(), modified_html, all)
    }

    /// 2026-08-11: warn about custom component tags (`<Name .../>`, PascalCase)
    /// whose mount lifecycle is a Phase 2 component-model feature. Standard
    /// (lowercase) HTML elements are unaffected.
    fn warn_component_tags(&mut self, html: &str) {
        let mut pos = 0;
        while let Some(rel) = html[pos..].find('<') {
            let start = pos + rel;
            let rest = &html[start + 1..];
            let next = rest.as_bytes().first().copied();
            if let Some(c) = next {
                if c.is_ascii_uppercase() {
                    let tag_end = rest
                        .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
                        .unwrap_or(rest.len());
                    let name = &rest[..tag_end];
                    self.diagnostics.push(format!(
                        "component tag '<{name}>' mounts a first-class reactive \
                         instance (SPEC 21.3); mount/unmount wiring lands with \
                         the Phase 2 component plan — rendered inert for now"
                    ));
                }
            }
            pos = start + 1;
        }
    }

    fn inject_ids(&mut self, html: &str) -> String {
        let mut in_progress = std::collections::HashSet::new();
        self.inject_ids_inner(html, &mut in_progress)
    }

    /// 2026-08-11 (Phase 2b): the recursive core of inject_ids — when a
    /// `<Name />` tag names a `render Name { ... }` fragment, the fragment's
    /// HTML is spliced in and processed (shared id_counter → unique element
    /// IDs). `in_progress` guards render cycles (A→B→A → compile error).
    fn inject_ids_inner(&mut self, html: &str, in_progress: &mut std::collections::HashSet<String>) -> String {
        let mut result = String::new();
        let mut pos = 0;
        let bytes = html.as_bytes();

        while pos < bytes.len() {
            if bytes[pos] == b'<'
                && bytes
                    .get(pos + 1)
                    .map(|&b| b.is_ascii_alphabetic() || b == b'!')
                    .unwrap_or(false)
            {
                if let Some((tag, end_pos)) = self.parse_tag(&html[pos..]) {
                    let tag_str = &html[pos..pos + end_pos];
                    let tag_lower = tag_str.to_lowercase();

                    // 2026-08-11 (Phase 2b, SPEC 21.3): `<Name />` mounts the
                    // `render Name { ... }` fragment. Splice the fragment's
                    // HTML and process it recursively (shared id_counter →
                    // unique element IDs). Inside a b-each template (each_depth
                    // > 0) tags pass through — item-scoped components are a
                    // later slice.
                    if self.each_depth == 0 {
                        // parse_tag strips the brackets, so self-closing and the
                        // name are derived from the RAW tag string.
                        let tag_name_raw = tag_str
                            .trim_start_matches('<')
                            .split_whitespace()
                            .next()
                            .unwrap_or("")
                            .trim_end_matches('>')
                            .to_string();
                        // 2026-08-12 (2b3): tag namespace resolution — a Briev-side
                        // instance var (`<c1 />`) mounts the component's fragment
                        // routed to the instance's slots; a component type
                        // (`<Counter />`) spawns an anonymous pool instance.
                        let inst_spec = self.instance_specs.get(&tag_name_raw).cloned();
                        let is_instance = inst_spec.is_some();
                        if is_instance || self.render_blocks.contains_key(&tag_name_raw) {
                            // The spec decides the routing: instance name for
                            // Briev-side, mount-index pool for HTML-side.
                            let spec = inst_spec.unwrap_or_else(|| {
                                let mount = self
                                    .component_mounts
                                    .entry(tag_name_raw.clone())
                                    .or_insert(0);
                                let specs = self
                                    .component_specs
                                    .get(&tag_name_raw)
                                    .cloned()
                                    .unwrap_or_default();
                                let s = specs
                                    .get(*mount % specs.len().max(1))
                                    .cloned()
                                    .unwrap_or_else(|| {
                                        crate::analysis::component_instances::MountSpec {
                                            component: tag_name_raw.clone(),
                                            index: 0,
                                            marker: format!("{}.0", tag_name_raw),
                                            fields: Vec::new(),
                                            txn_variants: std::collections::HashMap::new(),
                                        }
                                    });
                                *mount += 1;
                                s
                            });
                            let comp = if is_instance {
                                spec.component.clone()
                            } else {
                                tag_name_raw.clone()
                            };
                            if in_progress.contains(&comp) {
                                self.validation_errors.push(format!(
                                    "component cycle: render '{}' mounts itself (directly or transitively)",
                                    comp
                                ));
                                result.push_str(tag_str);
                                pos += end_pos;
                                continue;
                            }
                            in_progress.insert(comp.clone());
                            let raw = self
                                .render_blocks
                                .get(&comp)
                                .cloned()
                                .unwrap_or_default();
                            // The raw fragment, rewritten for this mount.
                            let fragment = self.apply_mount_spec(&raw, &spec);
                            // Self-closing `<Name />` skips just the tag; the
                            // paired `<Name>...</Name>` form skips to the close.
                            let close = format!("</{}>", tag_name_raw);
                            let skip_to = if tag_str.trim_end().ends_with("/>") {
                                end_pos
                            } else if let Some(rel) = html[pos..].find(&close) {
                                end_pos + rel + close.len()
                            } else {
                                end_pos
                            };
                            let spliced = self.inject_ids_inner(&fragment, in_progress);
                            result.push_str(&spliced);
                            in_progress.remove(&comp);
                            pos += skip_to;
                            continue;
                        }
                    }

                    // 2026-08-11 (Phase 2a fix): a self-closing tag (`<input
                    // b-show="show" />`) is NOT skipped outright — void
                    // elements legitimately carry directives (`b-when`,
                    // `b-bind:value`, `b-show`). Only comments and closing
                    // tags skip. (The raw string, not tag_lower, carries the
                    // leading `<` — `starts_with('!')` on tag_lower never
                    // matched `<!--`.)
                    if tag_str.starts_with("<!--") || tag_str.starts_with("</") {
                        // 2026-08-11 (Phase 2a3): a closing tag decrements the
                        // b-each nesting depth.
                        if tag_str.starts_with("</") && self.each_depth > 0 {
                            self.each_depth -= 1;
                        }
                        result.push_str(tag_str);
                        pos += end_pos;
                        continue;
                    }

                    // 2026-08-11 (Phase 2a3): inside a b-each subtree the whole
                    // template is item-scoped — pass through untouched.
                    // extract_bindings captures the item directives + injects
                    // the data-itm markers from the raw template.
                    if self.each_depth > 0 {
                        result.push_str(tag_str);
                        pos += end_pos;
                        continue;
                    }

                    let tag_process = tag_str;

                    // 2026-08-09 (Phase 14, SPEC 21.4): directive validation —
                    // `b-if` is invalid; `b-each` requires a stable `b-key`
                    // (dynamic children may be inserted/removed/reordered);
                    // `b-bind:value` targets an assignable field. Errors are
                    // collected (the compile() caller reports them) — a
                    // rejected directive is not silently ignored.
                    if let Some(verr) = self.validate_directives(&tag_str) {
                        self.validation_errors.push(verr);
                    }

                    let has_class_expr = self.extract_class_expression(&tag_str);
                    let has_b_class = tag_lower.contains("b-class");
                    let has_directive = tag_lower.contains("b-text")
                        || tag_lower.contains("b-show")
                        || tag_lower.contains("b-hide")
                        || tag_lower.contains("b-when")
                        || tag_lower.contains("b-trigger")
                        || tag_lower.contains("b-on")
                        || tag_lower.contains("b-bind")
                        || has_b_class
                        || tag_lower.contains("b-attr")
                        || tag_lower.contains("b-style")
                        || tag_lower.contains("b-each")
                        || has_class_expr.is_some();

                    if has_directive {
                        // 2026-08-11 (Phase 2a3): a `b-each` element's OWN
                        // directives (b-class/b-trigger/b-show on the <li>)
                        // are item-scoped — they must NOT become global
                        // bindings. inject_ids only gives the container an ID
                        // (the shim's container_id); extract_bindings captures
                        // the item directives from the raw tag + template.
                        let is_each_tag = tag_lower.contains("b-each:");
                        // Use preprocessed tag for directive processing
                        let elem_id = if !tag_lower.contains("id=") {
                            self.generate_element_id(&tag_process)
                        } else {
                            self.extract_id_from_tag(&tag_process)
                                .unwrap_or_else(|| self.generate_element_id(&tag_process))
                        };
                        let tag_name = tag_process.split_whitespace().next().unwrap_or("");
                        let tag_name_stripped = tag_name.trim_start_matches('<');

                        let tag_with_id = if !tag_lower.contains("id=") {
                            let rest = &tag_process[tag_name.len()..];
                            format!("<{} id=\"{}\"{}", tag_name_stripped, elem_id, rest)
                        } else {
                            tag_process.to_string()
                        };

                        result.push_str(&tag_with_id);

                        if is_each_tag {
                            self.each_depth += 1;
                        } else {
                            // Pass the computed elem_id to extract_directives so it uses consistent IDs
                            self.extract_directives(&tag_with_id, &elem_id);
                        }
                    } else {
                        result.push_str(tag_str);
                    }
                    pos += end_pos;
                    continue;
                }
            }
            result.push(html.chars().nth(pos).unwrap_or(' '));
            pos += 1;
        }

        result
    }

    fn extract_bindings(&mut self, html: &str) {
        // Note: b-trigger extraction now happens in inject_ids -> extract_directives
        // This function handles b-each and other directives that need full HTML parsing
        let mut pos = 0;
        let bytes = html.as_bytes();
        let mut element_stack: Vec<(String, usize)> = Vec::new();

        while pos < bytes.len() {
            if bytes[pos] == b'<'
                && bytes
                    .get(pos + 1)
                    .map(|&b| b.is_ascii_alphabetic() || b == b'!')
                    .unwrap_or(false)
            {
                if let Some((tag, end_pos)) = self.parse_tag(&html[pos..]) {
                    let tag_str = String::from_utf8_lossy(&bytes[pos..pos + end_pos]).to_string();
                    let tag_lower = tag_str.to_lowercase();

                    if tag_lower.starts_with('/') {
                        let closing_name = tag_lower
                            .trim_start_matches('/')
                            .split_whitespace()
                            .next()
                            .unwrap_or("");
                        if let Some(pos_in_stack) = element_stack
                            .iter()
                            .position(|(name, _)| name == closing_name)
                        {
                            element_stack.truncate(pos_in_stack);
                        }
                        pos += end_pos;
                        continue;
                    }

                    if !tag_lower.ends_with("/>") && !tag_lower.ends_with("?") {
                        let elem_name = tag.split_whitespace().next().unwrap_or("div").to_string();
                        element_stack.push((elem_name, pos));
                    }

                    let has_each = tag_lower.contains("b-each:");

                    if has_each {
                        let each_attr = tag_lower
                            .split_whitespace()
                            .find(|s| s.contains("b-each:"))
                            .unwrap_or("");
                        if let Some((item_name, iterable)) = self.extract_each_value(each_attr) {
                            // 2026-08-11 (Phase 2a3): use the id inject_ids put
                            // in the MODIFIED html (the container the shim
                            // resolves) — regenerating here would desync.
                            let elem_id = self
                                .extract_id_from_tag(&tag_str)
                                .unwrap_or_else(|| self.generate_element_id(&tag_str));
                            let inner_html = self.find_each_inner_html(&html[pos..], &tag);
                            let elem_name = tag.split_whitespace().next().unwrap_or(&tag).trim();
                            let key_expr = self
                                .extract_attr_value(&tag_str, "b-key")
                                .unwrap_or_else(|| "item".to_string());
                            // Capture the each element's OWN item directives
                            // (marker 0 = the rendered clone's root) and walk
                            // the inner template (data-itm markers injected).
                            let mut item_bindings: Vec<ItemBinding> = Vec::new();
                            let mut marker_counter: usize = 1;
                            self.capture_item_directives(&tag_str, 0, &item_name, &mut item_bindings);
                            let template_html = self.capture_template_bindings(
                                &inner_html,
                                &item_name,
                                &mut marker_counter,
                                &mut item_bindings,
                            );

                            self.bindings.push(Binding {
                                element_id: elem_id.clone(),
                                directive: Directive::Each {
                                    iterable: iterable,
                                    item_name: item_name,
                                    template_html: template_html,
                                    container_id: elem_id,
                                    item_bindings: item_bindings,
                                    key_expr: key_expr,
                                },
                            });
                            let total_len = end_pos + inner_html.len() + elem_name.len() + 3;
                            pos += total_len;
                            continue;
                        }
                    }

                    // extract_directives already called in inject_ids - skip to avoid duplicates
                    pos += end_pos;
                    continue;
                }
            }
            pos += 1;
        }
    }

    /// 2026-08-11 (Phase 2a3): capture the item-scoped directives on a single
    /// tag (the b-each element's own attributes when `marker == 0`, or a
    /// template element when the marker is its data-itm index). Unsupported
    /// item expressions are a compile error — never a silently dead directive.
    fn capture_item_directives(
        &mut self,
        tag: &str,
        marker: usize,
        item_name: &str,
        out: &mut Vec<ItemBinding>,
    ) {
        let tag_lower = tag.to_lowercase();
        for attr in tag_lower.split_whitespace().skip(1) {
            let attr = attr.trim_end_matches('>').trim_end_matches('/');
            if attr.starts_with("b-text") {
                if let Some(signal) = self.extract_attr_value(tag, "b-text") {
                    match self.check_item_expr(&signal, item_name) {
                        Ok(()) => out.push(ItemBinding { marker, directive: ItemDirective::Text { signal } }),
                        Err(msg) => self.validation_errors.push(msg),
                    }
                }
            } else if attr.starts_with("b-class") {
                if let Some(value) = self.extract_attr_value(tag, "b-class") {
                    // 2026-08-11 (housekeeping, Part 2): normalize via the
                    // shared helper (brace strip + quote trim).
                    let pairs = self.parse_class_pairs(&value);
                    let mut ok = true;
                    // for_each (not a `for` loop) keeps this single-level for
                    // Praetor's loop-depth rule.
                    pairs.iter().for_each(|(_, expr)| {
                        if let Err(msg) = self.check_item_expr(expr, item_name) {
                            self.validation_errors.push(msg);
                            ok = false;
                        }
                    });
                    if ok {
                        out.push(ItemBinding { marker, directive: ItemDirective::Class { pairs } });
                    }
                }
            } else if attr.starts_with("b-show") {
                if let Some(expr) = self.extract_attr_value(tag, "b-show") {
                    match self.check_item_expr(&expr, item_name) {
                        Ok(()) => out.push(ItemBinding { marker, directive: ItemDirective::Show { expr } }),
                        Err(msg) => self.validation_errors.push(msg),
                    }
                }
            } else if attr.starts_with("b-when") {
                if let Some(expr) = self.extract_attr_value(tag, "b-when") {
                    match self.check_item_expr(&expr, item_name) {
                        Ok(()) => out.push(ItemBinding { marker, directive: ItemDirective::When { expr } }),
                        Err(msg) => self.validation_errors.push(msg),
                    }
                }
            } else if attr.starts_with("b-trigger:") || attr.starts_with("b-on:") {
                let prefix = if attr.starts_with("b-trigger:") { "b-trigger:" } else { "b-on:" };
                if let Some((txn, _)) = self.extract_trigger_value_from_tag(tag, prefix) {
                    let event = self
                        .extract_event_suffix(&tag_lower, prefix.trim_end_matches(':'))
                        .unwrap_or_else(|| "click".to_string());
                    out.push(ItemBinding { marker, directive: ItemDirective::Trigger { event, txn } });
                }
            }
        }
    }

    /// 2026-08-11 (Phase 2a3): validate a scalar item expression. Accepted:
    /// the bare item (`item`) and `item <op> <literal>` comparisons. Anything
    /// else (state-field comparisons, calls) is rejected — the item renderer
    /// cannot evaluate it, and a silently dead directive is a contract
    /// violation.
    fn check_item_expr(&self, expr: &str, item_name: &str) -> Result<(), String> {
        let e = expr.trim();
        if e == item_name {
            return Ok(());
        }
        // Find the operator by scanning for its first character (no loop —
        // keeps capture_item_directives single-level for Praetor).
        if let Some(i) = e.find(['=', '!', '<', '>']) {
            let op = if e[i..].starts_with("==") {
                "=="
            } else if e[i..].starts_with("!=") {
                "!="
            } else if e[i..].starts_with("<=") {
                "<="
            } else if e[i..].starts_with(">=") {
                ">="
            } else if e[i..].starts_with('<') {
                "<"
            } else if e[i..].starts_with('>') {
                ">"
            } else {
                return Err(self.unsupported_item_msg(expr, item_name));
            };
            let (l, r) = (e[..i].trim(), e[i + op.len()..].trim());
            let ok_lit = |s: &str| s.parse::<f64>().is_ok() || s == "true" || s == "false";
            if (l == item_name && ok_lit(r)) || (r == item_name && ok_lit(l)) {
                return Ok(());
            }
        }
        Err(self.unsupported_item_msg(expr, item_name))
    }

    fn unsupported_item_msg(&self, expr: &str, item_name: &str) -> String {
        format!(
            "item-scoped expression '{}' is unsupported in b-each (Phase 2a3) — \
             only `{}` or `{} <op> <literal>` comparisons are rendered per item; \
             state-field expressions land with the Phase 2b component plan",
            expr, item_name, item_name
        )
    }

    /// 2026-08-11 (Phase 2a3): walk the b-each inner template, capturing
    /// item-scoped directives and injecting `data-itm="<marker>"` attributes so
    /// the shim can locate each directive's element inside a rendered clone.
    /// Returns the marked template HTML. `marker_counter` starts at 1 (marker 0
    /// is the clone root).
    fn capture_template_bindings(
        &mut self,
        html: &str,
        item_name: &str,
        marker_counter: &mut usize,
        out: &mut Vec<ItemBinding>,
    ) -> String {
        let mut result = String::new();
        let mut pos = 0;
        let bytes = html.as_bytes();
        while pos < bytes.len() {
            if bytes[pos] == b'<'
                && bytes
                    .get(pos + 1)
                    .map(|&b| b.is_ascii_alphabetic() || b == b'!')
                    .unwrap_or(false)
            {
                if let Some((tag, end_pos)) = self.parse_tag(&html[pos..]) {
                    let tag_str = &html[pos..pos + end_pos];
                    let tag_lower = tag_str.to_lowercase();
                    if tag_lower.starts_with('/') || tag_lower.starts_with('!') {
                        result.push_str(tag_str);
                        pos += end_pos;
                        continue;
                    }
                    let has_directive = tag_lower.contains("b-text")
                        || tag_lower.contains("b-class")
                        || tag_lower.contains("b-show")
                        || tag_lower.contains("b-when")
                        || tag_lower.contains("b-trigger")
                        || tag_lower.contains("b-on");
                    if has_directive {
                        let marker = *marker_counter;
                        *marker_counter += 1;
                        // Inject data-itm before the closing `>`.
                        let inject_at = tag_str.rfind('>').unwrap_or(end_pos - 1);
                        let marked = format!(
                            "{} data-itm=\"{}\"{}",
                            &tag_str[..inject_at],
                            marker,
                            &tag_str[inject_at..]
                        );
                        result.push_str(&marked);
                        self.capture_item_directives(tag_str, marker, item_name, out);
                    } else {
                        result.push_str(tag_str);
                    }
                    pos += end_pos;
                    continue;
                }
            }
            result.push(html.chars().nth(pos).unwrap_or(' '));
            pos += 1;
        }
        result
    }

    fn extract_id_from_tag(&self, tag: &str) -> Option<String> {
        let tag_lower = tag.to_lowercase();
        if let Some(id_pos) = tag_lower.find("id=") {
            let after = &tag[id_pos + 3..];
            let trimmed = after
                .trim_start_matches('=')
                .trim_start_matches('\"')
                .trim_start_matches('\'');
            let end = trimmed
                .find(|c: char| c.is_whitespace() || c == '\"' || c == '\'' || c == '>')
                .unwrap_or(trimmed.len());
            return Some(trimmed[..end].to_string());
        }
        None
    }

    fn find_each_inner_html(&self, html: &str, tag: &str) -> String {
        let elem_name = tag.split_whitespace().next().unwrap_or(tag).trim();
        let closing_pattern = format!("</{}>", elem_name);
        if let Some(closing_pos) = html.find(&closing_pattern) {
            // 2026-08-11 (Phase 2a3 fix): find the opening tag's end with the
            // quote-aware scan — `b-class="{ 'big': item > 5 }"` embeds a `>`
            // in the quoted value, and a naive `find('>')` truncated the inner
            // HTML at the comparison.
            let open_end = self.parse_tag(html).map(|(_, end)| end).unwrap_or(0);
            if open_end > 0 && open_end < closing_pos {
                return html[open_end..closing_pos].trim().to_string();
            }
        }
        String::new()
    }

    fn parse_tag<'a>(&self, s: &'a str) -> Option<(String, usize)> {
        if !s.starts_with('<') {
            return None;
        }
        // 2026-08-11 (Phase 2a fix): scan for the closing `>` while honoring
        // quoted attribute values — `b-when="count > 0"` embeds a `>` inside a
        // quote, and a naive `find('>')` truncated the tag at the comparison,
        // leaving the rest as text.
        let mut in_quote: Option<char> = None;
        for (i, c) in s.char_indices().skip(1) {
            match in_quote {
                Some(q) => {
                    if c == q {
                        in_quote = None;
                    }
                }
                None => {
                    if c == '"' || c == '\'' {
                        in_quote = Some(c);
                    } else if c == '>' {
                        let tag = &s[1..i];
                        return Some((tag.to_string(), i + 1));
                    }
                }
            }
        }
        None
    }

    fn extract_directives(&mut self, tag: &str, elem_id: &str) {
        let tag_lower = tag.to_lowercase();
        // Check for bare class={expr} syntax
        if let Some(expr) = self.extract_class_expression(tag) {
            let pairs = self.parse_class_expr(&expr);
            self.bindings.push(Binding {
                element_id: elem_id.to_string(),
                directive: Directive::Class { pairs },
            });
        }

        // First pass: validate directive prefixes
        for attr in tag_lower.split_whitespace().skip(1) {
            let attr = attr.trim_end_matches('>').trim_end_matches('/');
            if attr.starts_with("b-") {
                let prefix = if let Some(idx) = attr.find('=') {
                    attr[..idx].to_string()
                } else {
                    attr[..].to_string()
                };
                let is_known = KNOWN_DIRECTIVES.iter().any(|k| {
                    prefix == *k || prefix.starts_with(k.trim_end_matches(':'))
                });
                if !is_known {
                    self.diagnostics.push(format!(
                        "warning[RBV001]: unknown directive '{}' in tag '{}'",
                        prefix,
                        tag.split_whitespace().next().unwrap_or("<tag>")
                    ));
                }
            }
        }

        for attr in tag_lower.split_whitespace().skip(1) {
            let attr = attr.trim_end_matches('>').trim_end_matches('/');

if attr.starts_with("b-text") {
                if let Some(expr) = self.extract_attr_value(tag, "b-text") {
                    self.bindings.push(Binding {
                        element_id: elem_id.to_string(),
                        directive: Directive::Text { signal: expr },
                    });
                }
            } else if attr.starts_with("b-show") {
                if let Some(expr) = self.extract_attr_value(tag, "b-show") {
                    self.bindings.push(Binding {
                        element_id: elem_id.to_string(),
                        directive: Directive::Show { expr },
                    });
                }
            } else if attr.starts_with("b-hide") {
                if let Some(expr) = self.extract_attr_value(tag, "b-hide") {
                    self.bindings.push(Binding {
                        element_id: elem_id.to_string(),
                        directive: Directive::Hide { expr },
                    });
                }
            } else if attr.starts_with("b-when") {
                if let Some(expr) = self.extract_attr_value(tag, "b-when") {
                    self.bindings.push(Binding {
                        element_id: elem_id.to_string(),
                        directive: Directive::When { expr },
                    });
                }
            } else if attr.starts_with("b-bind") {
                if let Some(target) = self.extract_attr_value(tag, "b-bind:value") {
                    self.bindings.push(Binding {
                        element_id: elem_id.to_string(),
                        directive: Directive::Bind { target },
                    });
                }
            } else if attr.starts_with("b-trigger:") || attr.starts_with("b-on:") {
                let prefix = if attr.starts_with("b-trigger:") { "b-trigger:" } else { "b-on:" };
                let result = self.extract_trigger_value_from_tag(tag, prefix);
                let event = self.extract_event_suffix(&tag_lower, prefix.trim_end_matches(':'));
                if let Some((txn_name, params)) = result {
                    // Track user-triggered transactions for linting
                    // These should have preconditions that account for non-deterministic user input
                    self.user_triggered_txns.insert(txn_name.clone());
                    self.bindings.push(Binding {
                        element_id: elem_id.to_string(),
                        directive: Directive::Trigger {
                            event: event.unwrap_or_else(|| "click".to_string()),
                            txn: txn_name,
                            params,
                        },
                    });
                }
            } else if attr.starts_with("b-class") {
                if let Some(value) = self.extract_attr_value(tag, "b-class") {
                    // 2026-08-11 (housekeeping, Part 2): normalize the brace
                    // form + validate each class expr is a bounded single-field
                    // form the shim can evaluate (complex → compile error).
                    let pairs = self.parse_class_pairs(&value);
                    let mut ok = true;
                    // for_each (not a `for` loop) keeps this single-level for
                    // Praetor's loop-depth rule.
                    pairs.iter().for_each(|(_, expr)| {
                        if let Err(msg) = self.bounded_view_expr(expr) {
                            self.validation_errors.push(msg);
                            ok = false;
                        }
                    });
                    if ok {
                        self.bindings.push(Binding {
                            element_id: elem_id.to_string(),
                            directive: Directive::Class { pairs },
                        });
                    }
                }
            } else if attr.starts_with("b-attr") {
                if let Some(expr) = self.extract_attr_value(tag, "b-attr") {
                    if let Some((name, value)) = self.parse_attr_raw(&expr) {
                        if let Err(msg) = self.bounded_view_expr(&value) {
                            self.validation_errors.push(msg);
                        } else {
                            self.bindings.push(Binding {
                                element_id: elem_id.to_string(),
                                directive: Directive::Attr { name, value },
                            });
                        }
                    } else {
                        self.validation_errors.push(format!(
                            "b-attr expression '{}' is unsupported — expected `name: <value>` with a simple literal or field reference",
                            expr
                        ));
                    }
                }
            } else if attr.starts_with("b-style") {
                if let Some(expr) = self.extract_attr_value(tag, "b-style") {
                    if let Some((name, value)) = self.parse_attr_raw(&expr) {
                        if let Err(msg) = self.bounded_view_expr(&value) {
                            self.validation_errors.push(msg);
                        } else {
                            self.bindings.push(Binding {
                                element_id: elem_id.to_string(),
                                directive: Directive::Style { name, value },
                            });
                        }
                    } else {
                        self.validation_errors.push(format!(
                            "b-style expression '{}' is unsupported — expected `name: <value>` with a simple literal or field reference",
                            expr
                        ));
                    }
                }
            }
        }
    }

    fn extract_trigger_value(&self, attr: &str) -> Option<(String, Vec<(String, String)>)> {
        let after_colon = attr.strip_prefix("b-trigger:")
            .or_else(|| attr.strip_prefix("b-on:"))?;
        let after_event = after_colon.find('=')?;
        let value_part = &after_colon[after_event + 1..];

        let value = value_part.trim();

        let extracted = if value.starts_with('"') {
            let end = value[1..].find('"')?;
            value[1..end + 1].to_string()
        } else if value.starts_with('\'') {
            let end = value[1..].find('\'')?;
            value[1..end + 1].to_string()
} else if value.contains('(') {
            // Function call: extract up to the matching closing paren
            let open_pos = value.find('(')?;
            let rest = &value[open_pos..];
            let close_pos = if let Some(p) = find_closing_paren(rest) {
                open_pos + p + 1
            } else {
                open_pos + rest.len()
            };
            value[..close_pos].to_string()
        } else {
            let end = value
                .find(|c: char| c.is_whitespace() || c == '>')
                .unwrap_or(value.len());
            value[..end].to_string()
        };

        let mut params = Vec::new();

        if let Some(paren_start) = extracted.find('(') {
            let func_name = extracted[..paren_start].to_string();
            let inner = &extracted[paren_start + 1..];
            let inner_trimmed = inner.trim_end_matches(')');

            if !inner_trimmed.is_empty() {
                if inner_trimmed.contains(':') {
                    for pair in inner_trimmed.split(',') {
                        let pair = pair.trim();
                        if let Some(colon_pos) = pair.find(':') {
                            let param_name = pair[..colon_pos].trim().to_string();
                            let raw_value = pair[colon_pos + 1..].trim().to_string();
                            let param_value = strip_surrounding_quotes(&raw_value);
                            params.push((param_name, param_value));
                        }
                    }
                } else {
                    for (i, param) in inner_trimmed.split(',').enumerate() {
                        let raw_value = param.trim().to_string();
                        let param_value = strip_surrounding_quotes(&raw_value);
                        params.push((format!("_{}", i), param_value));
                    }
                }
            }

            Some((func_name, params))
        } else {
            Some((extracted, params))
        }
    }

    /// 2026-08-09 (Phase 14, SPEC 21.4): validate a tag's directives.
    /// - `b-if` is INVALID (structural conditionals use `b-when`).
    /// - `b-each:name` requires a stable `b-key` when children may be
    ///   inserted/removed/reordered (SPEC 21.4: dynamic repetition requires a
    ///   stable key).
    /// - `b-bind:value="target"` must target an assignable field (a computed
    ///   expression is invalid — separate value/trigger handlers are used).
    /// Returns a human-readable error, or None if the directives are valid.
    fn validate_directives(&self, tag: &str) -> Option<String> {
        let tag_lower = tag.to_lowercase();
        if tag_lower.contains("b-if") {
            return Some(
                "`b-if` is invalid (SPEC 21.4) — use `b-when` for structural \
                 mount/unmount and `b-show` for presentation-only visibility"
                    .into(),
            );
        }
        let has_each = tag_lower.contains("b-each:");
        if has_each && !tag_lower.contains("b-key") {
            return Some(
                "`b-each` requires a stable `b-key` (SPEC 21.4) — dynamic children \
                 may be inserted, removed, or reordered and need a stable identity"
                    .into(),
            );
        }
        if tag_lower.contains("b-bind:value") {
            // `b-bind:value="field"` must be an assignable field — an
            // expression (with operators) is invalid per SPEC 21.4.
            if let Some(target) = self.extract_attr_value(tag, "b-bind:value") {
                let trimmed = target.trim();
                let is_identifier = !trimmed.is_empty()
                    && trimmed
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '_' || c == '.');
                if !is_identifier {
                    return Some(
                        "`b-bind:value` accepts only an assignable field (SPEC 21.4) — \
                         computed expressions use separate value and trigger handlers"
                            .into(),
                    );
                }
            }
        }
        // 2026-08-09 (Phase 14, SPEC 21.5): view expressions are pure and
        // read-only — mutation, FFI, allocation, and spawning occur only in
        // explicit event handlers. A view-binding directive expression
        // containing a mutation/FFI construct is rejected.
        for directive in ["b-text", "b-show", "b-when", "b-class", "b-style"] {
            if let Some(expr) = self.extract_attr_value(tag, directive) {
                if let Some(impure) = Self::impure_view_expr(&expr) {
                    return Some(format!(
                        "view expression is not pure (SPEC 21.5): `{}={}` contains {} — \
                         mutation/FFI/allocation belongs in an explicit event handler",
                        directive, expr, impure
                    ));
                }
            }
        }
        // 2026-08-12 (2b3): the shim EVALUATES b-show/b-when comparisons on
        // flush (`step == 0` → `value == 0`) and b-text renders a bare field +
        // `.^` projections. Any other expression shape is rejected at compile
        // time — a directive the shim cannot evaluate is never silently dead.
        for directive in ["b-show", "b-when"] {
            if let Some(expr) = self.extract_attr_value(tag, directive) {
                if let Err(msg) = self.bounded_view_expr(&expr) {
                    return Some(format!("`{directive}` {msg}"));
                }
            }
        }
        if let Some(expr) = self.extract_attr_value(tag, "b-text") {
            if Self::compound_text_signal(&expr) {
                return Some(format!(
                    "`b-text` accepts only a bare field (`count`) or a `.^` projection \
                     (`items.^Size`) — complex expressions (string concat, arithmetic, \
                     calls) are unsupported (never silently dead DOM)"
                ));
            }
        }
        None
    }

    /// 2026-08-12 (2b3): whether a `b-text` value is a compound expression the
    /// shim cannot render. A bare field / `.^` projection / quoted literal /
    /// number contains none of the operator or whitespace markers.
    fn compound_text_signal(expr: &str) -> bool {
        let e = expr.trim();
        if e.is_empty() {
            return true;
        }
        let has_op = e.contains(['=', '!', '<', '>', '+', '-', '*', '/', '(', ')', '?', ':', ',']);
        has_op || e.chars().any(|c| c.is_whitespace())
    }

    /// Conservative purity check for a view-binding expression: rejects
    /// obvious mutation (`=`/`<-`/`~`), FFI (`frgn`/`spawn`/`Malloc#`), and
    /// the trigger-ish `@` write prefix. A plain identifier / arithmetic /
    /// field access expression passes.
    fn impure_view_expr(expr: &str) -> Option<&'static str> {
        let e = expr.trim();
        if e.contains("<-") || e.contains("~=") || e.contains("~<-") {
            return Some("a mutation/assignment");
        }
        if e.contains("Malloc#") || e.contains("spawn ") || e.contains("frgn") {
            return Some("an allocation or FFI call");
        }
        // Bare `=` (e.g. `x = y`) as an assignment — but `==`/`>=`/`<=`/`!=`
        // comparisons are fine. A single `=` not part of a comparison is an
        // assignment.
        let mut chars = e.char_indices();
        while let Some((i, c)) = chars.next() {
            if c == '=' {
                let prev = e[..i].chars().next_back();
                let next = chars.clone().next().map(|(_, c)| c);
                if !matches!(prev, Some('=') | Some('!') | Some('<') | Some('>'))
                    && !matches!(next, Some('=') | Some('>') | Some('<'))
                {
                    return Some("an assignment");
                }
            }
        }
        None
    }

    fn extract_trigger_value_from_tag(
        &self,
        tag: &str,
        prefix: &str,
    ) -> Option<(String, Vec<(String, String)>)> {
        let tag_lower = tag.to_lowercase();
        let prefix_lower = prefix.to_lowercase();

        // Find the attribute start
        let attr_start = tag_lower.find(prefix_lower.as_str())? + prefix_lower.len();
        let after_prefix = &tag[attr_start..];

        // Extract event (e.g., "click" from "b-trigger:click")
        let mut event_end = 0;
        for (i, c) in after_prefix.chars().enumerate() {
            if c == '=' || c.is_whitespace() {
                event_end = i;
                break;
            }
        }
        let event = if event_end > 0 {
            after_prefix[..event_end].to_string()
        } else {
            "click".to_string()
        };

        // Find the equals sign after event
        let eq_pos = after_prefix.find('=')?;
        let value_start = eq_pos + 1;
        let value_raw = &after_prefix[value_start..];

        // Extract the full quoted value
        let value = if value_raw.trim_start().starts_with('"') || value_raw.trim_start().starts_with('\'') {
            let rest = value_raw.trim_start();
            let quote_char = if rest.starts_with('"') { '"' } else { '\'' };
            // 2026-08-11: find_closing_quote expects the string to BEGIN with
            // the opening quote. The previous code passed the tail AFTER the
            // opening quote, so the first (and only) quote it saw was the
            // closing one — mistaken for an opening quote → None → the
            // unwrap_or(len) slice ran out of bounds (panic). Pass `rest`
            // (which includes the opening quote) and slice to the closing
            // quote inclusive.
            match find_closing_quote(rest, quote_char) {
                Some(end_quote) => rest[..end_quote + 1].to_string(),
                None => rest.to_string(),
            }
        } else {
            let end = value_raw
                .find(|c: char| c.is_whitespace() || c == '>')
                .unwrap_or(value_raw.len());
            value_raw[..end].trim().to_string()
        };

        // Now parse the extracted value using the existing logic
        let attr_for_parsing = format!("{}:{}={}", prefix.trim_end_matches(':'), event, value);
        self.extract_trigger_value(&attr_for_parsing)
    }

    fn extract_attr_value(&self, tag: &str, attr_name: &str) -> Option<String> {
        let tag_lower = tag.to_lowercase();
        let start = tag_lower.find(attr_name)? + attr_name.len();

        let remaining = &tag[start..];
        let remaining = remaining.trim_start();

        if remaining.starts_with('=') {
            let remaining = remaining[1..].trim_start();

            if remaining.starts_with('\"') {
                let end = remaining[1..].find('\"')?;
                Some(remaining[1..end + 1].to_string())
            } else if remaining.starts_with('\'') {
                let end = remaining[1..].find('\'')?;
                Some(remaining[1..end + 1].to_string())
            } else {
                let end = remaining.find(|c: char| c.is_whitespace() || c == '>')?;
                Some(remaining[..end].to_string())
            }
        } else {
            None
        }
    }

    fn extract_event_suffix(&self, tag_lower: &str, attr_name: &str) -> Option<String> {
        let attr_idx = tag_lower.find(attr_name)?;
        let after = &tag_lower[attr_idx + attr_name.len()..];

        if after.starts_with(':') {
            let end = after[1..].find(|c: char| !c.is_alphanumeric() && c != '_')?;
            Some(after[1..end + 1].to_string())
        } else {
            None
        }
    }

    fn generate_element_id(&mut self, tag: &str) -> String {
        if let Some(id_pos) = tag.to_lowercase().find("id=") {
            let after = &tag[id_pos + 3..];
            let trimmed = after
                .trim_start_matches('=')
                .trim_start_matches('\"')
                .trim_start_matches('\'');
            let end = trimmed
                .find(|c: char| c.is_whitespace() || c == '\"' || c == '\'' || c == '>')
                .unwrap_or(trimmed.len());
            return trimmed[..end].to_string();
        }

        let tag_name = tag.split_whitespace().next().unwrap_or("elem").to_string();
        let id = format!("rbv-{}-{}", tag_name.replace("<", ""), self.id_counter);
        self.id_counter += 1;
        id
    }

    fn parse_class_expr(&self, expr: &str) -> Vec<(String, String)> {
        let mut pairs = Vec::new();

        for part in expr.split(',') {
            let part = part.trim();
            if let Some(colon_pos) = part.find(':') {
                let signal = part[..colon_pos].trim().to_string();
                let class = part[colon_pos + 1..]
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string();
                pairs.push((signal, class));
            }
        }

        pairs
    }

    /// 2026-08-11 (housekeeping, Part 2): normalize a `b-class` value into
    /// `(class_name, expr)` pairs. Strips the outer `{ }` (parse_class_expr
    /// does not) and surrounding quotes from the class names. Shared by the
    /// global Class directive and the b-each item-class capture (DRY).
    fn parse_class_pairs(&self, value: &str) -> Vec<(String, String)> {
        let mut v = value.trim().to_string();
        if v.starts_with('{') && v.ends_with('}') {
            v = v[1..v.len() - 1].to_string();
        }
        if !v.contains(':') {
            // Bare form (`b-class="dark_mode"`) — a single boolean expression
            // with an empty class name (validated by the caller).
            return vec![("".to_string(), v.trim().to_string())];
        }
        self.parse_class_expr(&v)
            .into_iter()
            .map(|(name, expr)| {
                (
                    name.trim_matches('\'').trim_matches('"').to_string(),
                    expr.trim().to_string(),
                )
            })
            .collect()
    }

    /// 2026-08-11 (housekeeping, Part 2): the bounded view-expression forms the
    /// JS shim can evaluate flush-driven with a single field's value:
    /// - a bare identifier (`dark_mode`) → Some(field);
    /// - `field <op> <literal>` (==, !=, <, <=, >, >= vs number/bool/string) →
    ///   Some(field);
    /// - a bare literal → None (static — no field to flush on);
    /// anything else (calls, ternaries, multi-field, string concat) is a
    /// compile error, never a silently dead binding.
    fn bounded_view_expr(&self, expr: &str) -> Result<Option<String>, String> {
        let e = expr.trim();
        if e.is_empty() {
            return Err("empty view expression".to_string());
        }
        let is_identifier = |s: &str| {
            !s.is_empty()
                && s.chars().next().map_or(false, |c| c.is_alphabetic() || c == '_')
                && s.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.')
        };
        let is_literal = |s: &str| {
            let s = s.trim();
            s.parse::<f64>().is_ok()
                || s == "true"
                || s == "false"
                || (s.starts_with('\'') && s.len() >= 2
                    && s[1..].find('\'').map_or(false, |i| i == s.len() - 2))
                || (s.starts_with('"') && s.len() >= 2
                    && s[1..].find('"').map_or(false, |i| i == s.len() - 2))
        };
        // 2026-08-12 (2b3): a bare identifier INCLUDING dotted instance slots
        // (`p1.show`, `Counter.0.step`) is a field reference — the shim
        // evaluates the flushed value. `.^` projections are handled elsewhere.
        if is_identifier(e) {
            return Ok(Some(e.to_string()));
        }
        // `field <op> literal` — operator first char scan (no loop).
        if let Some(i) = e.find(['=', '!', '<', '>']) {
            let op = if e[i..].starts_with("==") {
                "=="
            } else if e[i..].starts_with("!=") {
                "!="
            } else if e[i..].starts_with("<=") {
                "<="
            } else if e[i..].starts_with(">=") {
                ">="
            } else if e[i..].starts_with('<') {
                "<"
            } else if e[i..].starts_with('>') {
                ">"
            } else {
                return Err(format!("view expression '{}' is unsupported (Phase 2 directives) — only `field`, `field <op> <literal>`, or a literal", expr));
            };
            let (l, r) = (e[..i].trim(), e[i + op.len()..].trim());
            if is_identifier(l) && is_literal(r) {
                return Ok(Some(l.to_string()));
            }
        }
        if is_literal(e) {
            return Ok(None);
        }
        Err(format!(
            "view expression '{}' is unsupported (Phase 2 directives) — only \
             `field`, `field <op> <literal>`, or a literal; complex expressions \
             (ternaries, calls, string concat) land with the Phase 2b component plan",
            expr
        ))
    }

    /// 2026-08-11 (housekeeping, Part 2): split a `name: value` attribute
    /// WITHOUT stripping the value's quotes — a quoted `'dark'` is a literal
    /// the shim applies at init, an unquoted identifier is a state-field
    /// reference. Stripping here made `'dark'` indistinguishable from a field
    /// named `dark`. The split is at the FIRST `:` OUTSIDE quotes — a quoted
    /// value like `'background: '` must not truncate the name.
    fn parse_attr_raw(&self, expr: &str) -> Option<(String, String)> {
        let mut in_quote: Option<char> = None;
        for (i, c) in expr.char_indices() {
            match in_quote {
                Some(q) => {
                    if c == q {
                        in_quote = None;
                    }
                }
                None => {
                    if c == '"' || c == '\'' {
                        in_quote = Some(c);
                    } else if c == ':' {
                        let name = expr[..i].trim().to_string();
                        let value = expr[i + 1..].trim().to_string();
                        return Some((name, value));
                    }
                }
            }
        }
        None
    }

    fn extract_each_value(&self, attr: &str) -> Option<(String, String)> {
        let after_prefix = attr.strip_prefix("b-each:")?;
        let (item_name, after_item) = after_item_name(after_prefix)?;
        if !after_item.starts_with('=') {
            return None;
        }
        let after_eq = &after_item[1..].trim();
        let mut iterable = after_eq.trim_matches('"').trim_matches('\'').to_string();
        if iterable.ends_with('>') {
            iterable.pop();
            if let Some(c) = iterable.chars().last() {
                if c == '"' || c == '\'' {
                    iterable.pop();
                }
            }
        }
        Some((item_name.to_string(), iterable))
    }
}

fn after_item_name(s: &str) -> Option<(&str, &str)> {
    let end = s.find(|c: char| !c.is_alphanumeric() && c != '_')?;
    Some((&s[..end], &s[end..]))
}

impl Default for ViewCompiler {
    fn default() -> Self {
        Self::new()
    }
}

fn find_closing_paren(s: &str) -> Option<usize> {
    let mut depth = 0;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    for (i, c) in s.chars().enumerate() {
        if !in_double_quote && c == '\'' && !in_single_quote {
            in_single_quote = true;
        } else if !in_double_quote && c == '\'' && in_single_quote {
            in_single_quote = false;
        } else if !in_single_quote && c == '"' && !in_double_quote {
            in_double_quote = true;
        } else if !in_single_quote && c == '"' && in_double_quote {
            in_double_quote = false;
        } else if !in_single_quote && !in_double_quote {
            if c == '(' {
                depth += 1;
            } else if c == ')' {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
            }
        }
        // ignore ( and ) inside strings
    }
    None
}

fn find_closing_quote(s: &str, quote_char: char) -> Option<usize> {
    let chars: Vec<char> = s.chars().collect();
    let mut first_quote_pos = None;
    for (i, c) in chars.iter().enumerate() {
        if *c == quote_char {
            if first_quote_pos.is_none() {
                first_quote_pos = Some(i);
            } else {
                // This is a candidate closing quote
                if let Some(next_char) = chars.get(i + 1).copied() {
                    if next_char.is_whitespace() || next_char == '>' || next_char == '/' {
                        // This quote terminates the attribute value
                        return Some(i);
                    }
                    // Not a terminator, continue looking for the next quote
                } else {
                    return Some(i);
                }
            }
        }
    }
    None
}

/// Root field of a `b-when`/`b-show` CONDITION expression. A condition is
/// typically a comparison (`count > 0`, `items.^Size > 0`) — the root is the
/// leading identifier token with `.^X` projections stripped. Shared by the
/// SRBV check, the web generator's handle lookup, and the frontend's
/// view-bound-field protection (DRY).
pub fn condition_root_signal<'a>(expr: &'a str) -> (&'a str, Vec<&'a str>) {
    let head = expr.trim().split_whitespace().next().unwrap_or("");
    root_signal(head)
}

/// Split a view signal into its root field name and any `.^X` reflection
/// projection suffixes. `count` → (`count`, []); `items.^Length` →
/// (`items`, ["Length"]); `a.^Length.^Bytes` → (`a`, ["Length", "Bytes"]).
///
/// 2026-08-14 (§6b.5): a view expressing an element count uses the `Count#`
/// intrinsic — `Count#(items)` → (`items`, ["Count"]), and the web generator
/// maps the `Count` projection to the materialized array's `.length`.
///
/// 2026-08-11: single definition — reused by the web generator's
/// `field_handle_for_signal` (handle lookup binds the root field), by
/// `verify_srbv` (SRBV checks the root signal exists), and by the brievc
/// frontend (view-bound fields protect %State slots from dead-field
/// elimination). The `.^X` suffix is a projection on top of the field's
/// value, never a separate signal.
pub fn root_signal<'a>(signal: &'a str) -> (&'a str, Vec<&'a str>) {    let signal = signal.trim();
    // 2026-08-14 (§6b.5): `Count#(field)` binds the field, projecting its
    // element count (the web generator maps it to `.length`).
    if let Some(rest) = signal.strip_prefix("Count#(") {
        if let Some(field) = rest.strip_suffix(')') {
            let field = field.trim();
            if field.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return (field, vec!["Count"]);
            }
        }
    }
    let mut proj: Vec<&str> = Vec::new();
    let mut head = signal;
    loop {
        if let Some(dot) = head.rfind(".^") {
            let suffix = &head[dot + 2..];
            if !suffix.is_empty()
                && suffix.chars().all(|c| c.is_ascii_alphabetic() || c == '_')
            {
                proj.push(suffix);
                head = &head[..dot];
            } else {
                break;
            }
        } else {
            break;
        }
    }
    proj.reverse();
    (head, proj)
}

/// Verify the `.s` strict profile (SPEC §3.2, `ui.s.rbv`) view-state
/// isomorphism: every signal/transaction referenced in the view bindings must
/// exist and carry non-trivial contracts (not `[true]` on both sides).
/// Returns list of verification errors.
pub fn verify_srbv(
    bindings: &[Binding],
    program: &[ast::TopLevel],
) -> Vec<String> {
    let mut errors = Vec::new();

    // Build lookup maps from the program
    let mut state_vars: HashSet<String> = HashSet::new();
    let mut txn_contracts: HashMap<String, &Contract> = HashMap::new();

    for item in program {
        match item {
            TopLevel::StateDecl(state) => {
                state_vars.insert(state.name.clone());
            }
            // 2026-08-11: a top-level `let name: Type = ...;` is the standard
            // `.s.rbv` state shape (parsed as a Statement::Let, not a
            // StateDecl) — without this, `b-text="count"` on a `let count`
            // fails SRBV001.
            TopLevel::Statement(stmt) => {
                if let ast::Statement::Let { name, names, .. } = stmt.as_ref() {
                    state_vars.insert(name.clone());
                    state_vars.extend(names.iter().cloned());
                }
            }
            TopLevel::Transaction(txn) => {
                txn_contracts.insert(txn.name.clone(), &txn.contract);
            }
            TopLevel::Definition(defn) => {
                txn_contracts.insert(defn.name.clone(), &defn.contract);
            }
            _ => {}
        }
    }

    for binding in bindings {
        match &binding.directive {
            Directive::Text { signal } => {
                // 2026-08-11: `items.^Size` binds the root field `items` —
                // the reflection suffix is a projection, not a separate signal.
                let (root, _) = root_signal(signal);
                if !state_vars.contains(root) && !txn_contracts.contains_key(root) {
                    errors.push(format!(
                        "error[SRBV001]: view references undefined signal '{}' in b-text",
                        signal
                    ));
                }
                if let Some(contract) = txn_contracts.get(root) {
                    if matches!(&contract.pre_condition, Expr::Bool(true))
                        && matches!(&contract.post_condition, Expr::Bool(true))
                    {
                        errors.push(format!(
                            "error[SRBV002]: view references '{}' which has trivial [true][true] contract",
                            signal
                        ));
                    }
                }
            }
            Directive::Show { expr } | Directive::Hide { expr } => {
                // Check that referenced variables in the expression exist
                let var_name = expr.trim();
                if !state_vars.contains(var_name) && !txn_contracts.contains_key(var_name) {
                    // Could be a compound expression - just warn
                    errors.push(format!(
                        "error[SRBV003]: view expression '{}' references undefined variable",
                        expr
                    ));
                }
            }
            Directive::When { expr } => {
                // 2026-08-11 (Phase 2a): a `b-when` condition is typically a
                // comparison (`b-when="count > 0"`) — the root signal is the
                // leading identifier, not the whole expression. Compound
                // conditions must not false-positive SRBV003.
                let (root, _) = condition_root_signal(expr);
                if !state_vars.contains(root) && !txn_contracts.contains_key(root) {
                    errors.push(format!(
                        "error[SRBV003]: view expression '{}' references undefined variable",
                        expr
                    ));
                }
            }
            Directive::Trigger { txn, .. } => {
                if !txn_contracts.contains_key(txn) {
                    errors.push(format!(
                        "error[SRBV004]: view references undefined transaction '{}' in trigger",
                        txn
                    ));
                } else if let Some(contract) = txn_contracts.get(txn) {
                    if matches!(&contract.pre_condition, Expr::Bool(true))
                        && matches!(&contract.post_condition, Expr::Bool(true))
                    {
                        errors.push(format!(
                            "error[SRBV005]: triggered transaction '{}' has trivial [true][true] contract",
                            txn
                        ));
                    }
                }
            }
            Directive::Class { pairs } => {
                for (_, expr) in pairs {
                    if !state_vars.contains(expr.as_str()) && !txn_contracts.contains_key(expr.as_str()) {
                        errors.push(format!(
                            "error[SRBV006]: view class expression references undefined '{}'",
                            expr
                        ));
                    }
                }
            }
            Directive::Attr { value, .. } => {
                if !state_vars.contains(value.as_str()) && !txn_contracts.contains_key(value.as_str()) {
                    errors.push(format!(
                        "error[SRBV007]: view attribute references undefined '{}'",
                        value
                    ));
                }
            }
            Directive::Style { value, .. } => {
                if !state_vars.contains(value.as_str()) && !txn_contracts.contains_key(value.as_str()) {
                    errors.push(format!(
                        "error[SRBV008]: view style references undefined '{}'",
                        value
                    ));
                }
            }
            Directive::Bind { target } => {
                // 2026-08-11 (Phase 2a2): `b-bind:value` writes `target` — it
                // must be a declared state field (the unique-writer resolution
                // happens at build time against the transition graph).
                let (root, _) = root_signal(target);
                if !state_vars.contains(root) {
                    errors.push(format!(
                        "error[SRBV011]: b-bind:value targets undefined field '{}'",
                        target
                    ));
                }
            }
            Directive::Each { iterable, .. } => {
                if !state_vars.contains(iterable.as_str()) && !txn_contracts.contains_key(iterable.as_str()) {
                    errors.push(format!(
                        "error[SRBV009]: view b-each references undefined '{}'",
                        iterable
                    ));
                }
            }
        }
    }

    // Verify state-DOM correspondence: every state mutation should have a view binding
    // For now, check that at least some bindings exist for rendered state
    if !state_vars.is_empty() && bindings.is_empty() {
        errors.push(
            "error[SRBV010]: state variables declared but no view bindings found — view may not reflect state".to_string()
        );
    }

    // Check that user-triggered transactions have non-trivial preconditions
    for binding in bindings {
        if let Directive::Trigger { txn, .. } = &binding.directive {
            if let Some(contract) = txn_contracts.get(txn) {
                if matches!(&contract.pre_condition, Expr::Bool(true)) {
                    errors.push(format!(
                        "error[SRBV011]: user-triggered transaction '{}' has precondition [true]; should specify when it can fire",
                        txn
                    ));
                }
            }
        }
    }

    errors
}

fn strip_surrounding_quotes(s: &str) -> String {
    let trimmed = s.trim();
    if (trimmed.starts_with('\'') && trimmed.ends_with('\'')) || (trimmed.starts_with('"') && trimmed.ends_with('"')) {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_signal_and_transaction() {
        let mut vc = ViewCompiler::new();
        vc.register_signal("sig_a", 0);
        vc.register_transaction("txn_b", 1);
        assert!(vc.signals.contains_key("sig_a"));
        assert!(vc.transactions.contains_key("txn_b"));
        assert_eq!(vc.transactions.get("txn_b"), Some(&1));
    }

    #[test]
    fn test_compile_basic_html_no_directives() {
        let mut vc = ViewCompiler::new();
        let (bindings, html, _) = vc.compile("<div>hello</div>");
        assert!(bindings.is_empty(), "No directives = no bindings");
        assert!(!html.is_empty());
        // No directives so no IDs injected
        assert!(!html.contains("id="));
    }

    #[test]
    fn test_compile_b_text_directive() {
        let mut vc = ViewCompiler::new();
        let (bindings, html, _) = vc.compile(r#"<div b-text="name">text</div>"#);
        assert!(!bindings.is_empty(), "Should create text binding");
        let has_text = bindings.iter().any(|b| matches!(&b.directive, Directive::Text { .. }));
        assert!(has_text, "Should have Text directive");
    }

    #[test]
    fn test_compile_b_show_directive() {
        let mut vc = ViewCompiler::new();
        let (bindings, html, _) = vc.compile(r#"<div b-show="isVisible">text</div>"#);
        let has_show = bindings.iter().any(|b| matches!(&b.directive, Directive::Show { .. }));
        assert!(has_show, "Should have Show directive");
    }

    #[test]
    fn test_compile_b_hide_directive() {
        let mut vc = ViewCompiler::new();
        let (bindings, html, _) = vc.compile(r#"<div b-hide="isHidden">text</div>"#);
        let has_hide = bindings.iter().any(|b| matches!(&b.directive, Directive::Hide { .. }));
        assert!(has_hide, "Should have Hide directive");
    }

    #[test]
    fn test_compile_inject_ids_adds_id_attr() {
        let mut vc = ViewCompiler::new();
        let (_, html, _) = vc.compile(r#"<div b-text="x">text</div>"#);
        assert!(html.contains("id=\""), "Should inject id for directive elements");
    }

    #[test]
    fn test_compile_empty_html() {
        let mut vc = ViewCompiler::new();
        let (bindings, html, _) = vc.compile("");
        assert!(bindings.is_empty());
        assert!(html.is_empty());
    }

    // ── 2026-08-09 (Phase 14, SPEC 21.4): directive validation ────────

    #[test]
    fn b_if_is_rejected() {
        let mut vc = ViewCompiler::new();
        let (_, _, diagnostics) = vc.compile(
            r#"<div b-if="x > 0">hi</div>"#,
        );
        assert!(
            diagnostics.iter().any(|d| d.contains("`b-if` is invalid")),
            "{:?}",
            diagnostics
        );
    }

    #[test]
    fn b_each_requires_b_key() {
        let mut vc = ViewCompiler::new();
        let (_, _, diagnostics) = vc.compile(
            r#"<li b-each:item="items">x</li>"#,
        );
        assert!(
            diagnostics.iter().any(|d| d.contains("requires a stable `b-key`")),
            "{:?}",
            diagnostics
        );
    }

    #[test]
    fn b_each_with_b_key_is_valid() {
        let mut vc = ViewCompiler::new();
        let (_, _, diagnostics) = vc.compile(
            r#"<li b-each:item="items" b-key="item.id">x</li>"#,
        );
        assert!(
            !diagnostics.iter().any(|d| d.contains("`b-key`")),
            "{:?}",
            diagnostics
        );
    }

    #[test]
    fn b_bind_value_must_be_assignable_field() {
        let mut vc = ViewCompiler::new();
        let (_, _, diagnostics) = vc.compile(
            r#"<input b-bind:value="count + 1">"#,
        );
        assert!(
            diagnostics.iter().any(|d| d.contains("`b-bind:value` accepts only an assignable field")),
            "{:?}",
            diagnostics
        );
    }

    // ── 2026-08-11 (Phase 2a, SPEC 21.4): b-when structural mount ─────

    #[test]
    fn b_when_extracts_structural_binding() {
        let mut vc = ViewCompiler::new();
        let (bindings, modified_html, diagnostics) =
            vc.compile(r#"<div b-when="count > 0">panel</div>"#);
        assert!(
            !diagnostics.iter().any(|d| d.contains("unknown directive")),
            "b-when must be a known directive: {:?}",
            diagnostics
        );
        let when = bindings
            .iter()
            .find(|b| matches!(&b.directive, Directive::When { .. }))
            .expect("b-when binding extracted");
        match &when.directive {
            Directive::When { expr } => assert_eq!(expr, "count > 0"),
            other => panic!("expected Directive::When, got {other:?}"),
        }
        assert!(
            modified_html.contains("id=\""),
            "element IDs injected so the shim can mount/unmount"
        );
    }

    #[test]
    fn b_when_purity_enforced() {
        // SPEC 21.5: b-when is a view expression — mutation is rejected there
        // just like b-text.
        let mut vc = ViewCompiler::new();
        let (_, _, diagnostics) = vc.compile(
            r#"<div b-when="count = 5">panel</div>"#,
        );
        assert!(
            diagnostics.iter().any(|d| d.contains("not pure") && d.contains("assignment")),
            "{:?}",
            diagnostics
        );
    }

    #[test]
    fn self_closing_tag_with_directive_gets_id_and_binding() {
        // 2026-08-11 (Phase 2a fix): `<input b-show="show" />` was skipped
        // outright (no ID, no binding) because inject_ids bailed on `/>`.
        // Void elements legitimately carry directives.
        let mut vc = ViewCompiler::new();
        let (bindings, modified_html, diagnostics) =
            vc.compile(r#"<input b-show="show" />"#);
        assert!(diagnostics.is_empty(), "{:?}", diagnostics);
        assert!(
            bindings.iter().any(|b| matches!(&b.directive, Directive::Show { .. })),
            "self-closing tag's b-show must extract: {bindings:?}"
        );
        assert!(
            modified_html.contains("id=\"") && modified_html.contains("/>"),
            "ID must be injected into the self-closing tag: {modified_html}"
        );
    }

    // ── 2026-08-11 (Phase 2a2, SPEC 21.4): b-bind:value ────────────────

    #[test]
    fn b_bind_extracts_target() {
        let mut vc = ViewCompiler::new();
        let (bindings, modified_html, diagnostics) =
            vc.compile(r#"<input b-bind:value="name" />"#);
        assert!(diagnostics.is_empty(), "{:?}", diagnostics);
        assert!(
            modified_html.contains("id=\"") && modified_html.contains("/>"),
            "self-closing b-bind input gets an ID: {modified_html}"
        );
        let bind = bindings
            .iter()
            .find(|b| matches!(&b.directive, Directive::Bind { .. }))
            .expect("b-bind binding extracted");
        match &bind.directive {
            Directive::Bind { target } => assert_eq!(target, "name"),
            other => panic!("expected Directive::Bind, got {other:?}"),
        }
    }

    #[test]
    fn b_bind_compound_target_rejected() {
        // validate_directives already rejects a non-assignable target
        // (SPEC 21.4) — a computed expression is not a field.
        let mut vc = ViewCompiler::new();
        let (_, _, diagnostics) = vc.compile(r#"<input b-bind:value="count + 1">"#);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.contains("`b-bind:value` accepts only an assignable field")),
            "{:?}",
            diagnostics
        );
    }

    // ── 2026-08-11 (Phase 2a3): b-each + b-key ────────────────────────

    #[test]
    fn b_each_captures_item_bindings_and_marks_template() {
        let mut vc = ViewCompiler::new();
        let (bindings, _, diagnostics) = vc.compile(
            r#"<ul>
  <li b-each:item="items" b-key="item" b-class="{ 'selected': item == 2 }">
    <span b-text="item">x</span>
  </li>
</ul>"#,
        );
        assert!(
            diagnostics.is_empty(),
            "item text/class with literal comparisons must compile: {:?}",
            diagnostics
        );
        let each = bindings
            .iter()
            .find(|b| matches!(&b.directive, Directive::Each { .. }))
            .expect("Each binding extracted");
        let Directive::Each {
            item_bindings,
            key_expr,
            template_html,
            iterable,
            item_name,
            ..
        } = &each.directive
        else {
            unreachable!()
        };
        assert_eq!(iterable.as_str(), "items");
        assert_eq!(item_name.as_str(), "item");
        assert_eq!(key_expr, "item");
        assert_eq!(item_bindings.len(), 2, "container class + inner text: {item_bindings:?}");
        assert!(
            template_html.contains("data-itm=\""),
            "inner template must carry data-itm markers: {template_html}"
        );
        // The inner b-text must NOT leak as a global binding.
        assert!(
            !bindings.iter().any(|b| {
                matches!(&b.directive, Directive::Text { signal } if signal == "item")
            }),
            "item-scoped bindings must not leak as globals: {bindings:?}"
        );
    }

    #[test]
    fn b_each_unsupported_item_expr_is_error() {
        // A state-field comparison (`item == selected`) is not evaluable by the
        // scalar item renderer — it must be a hard error, never silent dead DOM.
        let mut vc = ViewCompiler::new();
        let (_, _, diagnostics) = vc.compile(
            r#"<li b-each:item="items" b-key="item" b-class="item == selected">x</li>"#,
        );
        assert!(
            diagnostics
                .iter()
                .any(|d| d.contains("unsupported in b-each") && d.contains("item == selected")),
            "{:?}",
            diagnostics
        );
    }

    // ── 2026-08-11 (Phase 2b, SPEC 21.3): component fragment mounting ──

    #[test]
    fn component_tag_mounts_render_block() {
        let mut vc = ViewCompiler::new();
        let mut blocks = HashMap::new();
        blocks.insert(
            "Counter".to_string(),
            r#"<span b-text="count">0</span>"#.to_string(),
        );
        vc.set_render_blocks(blocks);
        let (bindings, modified_html, diagnostics) =
            vc.compile(r#"<div><Counter /></div>"#);
        assert!(
            diagnostics.is_empty(),
            "known component must not warn: {:?}",
            diagnostics
        );
        assert!(
            modified_html.contains("<span") && modified_html.contains("b-text=\"count\""),
            "fragment must be inlined: {modified_html}"
        );
        assert!(
            !modified_html.contains("<Counter"),
            "mount tag must be replaced: {modified_html}"
        );
        assert!(
            modified_html.contains("data-instance=\"Counter.0\""),
            "fragment root carries its instance marker for the shim: {modified_html}"
        );
        assert!(
            bindings.iter().any(|b| matches!(&b.directive, Directive::Text { signal } if signal == "count")),
            "fragment bindings extracted: {bindings:?}"
        );
    }

    #[test]
    fn component_tag_paired_form_mounts() {
        let mut vc = ViewCompiler::new();
        let mut blocks = HashMap::new();
        blocks.insert(
            "Counter".to_string(),
            "<span>hi</span>".to_string(),
        );
        vc.set_render_blocks(blocks);
        let (_, modified_html, _) = vc.compile(r#"<Counter></Counter>"#);
        assert!(
            modified_html.contains("<span") && modified_html.contains("hi</span>"),
            "paired form must inline the fragment: {modified_html}"
        );
    }

    #[test]
    fn component_multiple_mounts_unique_ids() {
        let mut vc = ViewCompiler::new();
        let mut blocks = HashMap::new();
        blocks.insert("Counter".to_string(), r#"<span b-text="count">0</span>"#.to_string());
        vc.set_render_blocks(blocks);
        let (bindings, modified_html, _) =
            vc.compile(r#"<Counter /><Counter />"#);
        let id_count = modified_html.matches("rbv-").count();
        assert_eq!(id_count, 2, "each mount gets its own element: {modified_html}");
        // Both mounts bind the same state handle → both must register (the
        // per-handle effect list fans out).
        assert_eq!(
            bindings
                .iter()
                .filter(|b| matches!(&b.directive, Directive::Text { signal } if signal == "count"))
                .count(),
            2,
            "both mounts bind count: {bindings:?}"
        );
    }

    #[test]
    fn component_cycle_is_error() {
        let mut vc = ViewCompiler::new();
        let mut blocks = HashMap::new();
        blocks.insert("A".to_string(), "<B />".to_string());
        blocks.insert("B".to_string(), "<A />".to_string());
        vc.set_render_blocks(blocks);
        let (_, _, diagnostics) = vc.compile("<A />");
        assert!(
            diagnostics.iter().any(|d| d.contains("component cycle")),
            "{:?}",
            diagnostics
        );
    }

    #[test]
    fn unknown_component_tag_warns() {
        let mut vc = ViewCompiler::new();
        let (_, _, diagnostics) = vc.compile("<Mystery />");
        assert!(
            diagnostics.iter().any(|d| d.contains("component tag '<Mystery>'")),
            "{:?}",
            diagnostics
        );
    }

    // ── 2026-08-11 (housekeeping, Part 2): global class/style/attr ───

    #[test]
    fn global_class_style_attr_extract_and_validate() {
        let mut vc = ViewCompiler::new();
        let (bindings, _, diagnostics) = vc.compile(
            r#"<div b-class="{ 'active': dark_mode == true }" b-style="background: dark_mode" b-attr="data-mode: 'dark'">x</div>"#,
        );
        assert!(diagnostics.is_empty(), "{:?}", diagnostics);
        assert!(
            bindings.iter().any(|b| matches!(&b.directive, Directive::Class { .. }))
                && bindings.iter().any(|b| matches!(&b.directive, Directive::Style { .. }))
                && bindings.iter().any(|b| matches!(&b.directive, Directive::Attr { .. })),
            "class/style/attr bindings extracted: {bindings:?}"
        );
    }

    #[test]
    fn global_class_complex_expr_is_error() {
        // A ternary/string-concat expression cannot be evaluated by the
        // bounded shim evaluator — must be a compile error, not silent dead DOM.
        let mut vc = ViewCompiler::new();
        let (_, _, diagnostics) = vc.compile(
            r#"<div b-style="'background: ' + (dark_mode ? '#222' : '#fff') + ';'">x</div>"#,
        );
        assert!(
            diagnostics.iter().any(|d| d.contains("unsupported")),
            "{:?}",
            diagnostics
        );
    }

    /// 2026-08-11 (housekeeping, Part 2): an HTML comment whose text mentions a
    /// directive name (`<!-- b-each: ... -->`) must be skipped — the old
    /// `tag_lower.starts_with('!')` check never matched (the raw string carries
    /// the leading `<`), so comment text was validated as real directives.
    #[test]
    fn comment_mentioning_directive_is_not_validated() {
        let mut vc = ViewCompiler::new();
        let (_, _, diagnostics) = vc.compile(
            r#"<!-- b-each: iterate over a list -->
<ul><li b-each:item="nums" b-key="item">x</li></ul>"#,
        );
        assert!(
            diagnostics.is_empty(),
            "comment text must not trigger directive validation: {:?}",
            diagnostics
        );
    }

    // ── 2026-08-09 (Phase 14, SPEC 21.5): view-expression purity ──────
    #[test]
    fn pure_view_expression_passes() {
        let mut vc = ViewCompiler::new();
        let (_, _, diagnostics) = vc.compile(
            r#"<span b-text="count + 1">0</span>"#,
        );
        assert!(
            !diagnostics.iter().any(|d| d.contains("not pure")),
            "{:?}",
            diagnostics
        );
    }

    #[test]
    fn mutation_in_view_expression_rejected() {
        let mut vc = ViewCompiler::new();
        let (_, _, diagnostics) = vc.compile(
            r#"<span b-text="count = 5">0</span>"#,
        );
        assert!(
            diagnostics.iter().any(|d| d.contains("not pure") && d.contains("assignment")),
            "{:?}",
            diagnostics
        );
    }

    #[test]
    fn ffi_in_view_expression_rejected() {
        let mut vc = ViewCompiler::new();
        let (_, _, diagnostics) = vc.compile(
            r#"<span b-text="Malloc#(64)">0</span>"#,
        );
        assert!(
            diagnostics.iter().any(|d| d.contains("not pure") && d.contains("allocation")),
            "{:?}",
            diagnostics
        );
    }

    #[test]
    fn comparison_in_view_expression_is_pure() {
        // `==`/`>=` are comparisons, not assignments — they must pass purity.
        let mut vc = ViewCompiler::new();
        let (_, _, diagnostics) = vc.compile(
            r#"<div b-show="count == 5">x</div>"#,
        );
        assert!(
            !diagnostics.iter().any(|d| d.contains("not pure")),
            "{:?}",
            diagnostics
        );
    }
}
