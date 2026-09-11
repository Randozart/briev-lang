// ── GLUE Web Generator (WASM-first webstack) ────────────────────────────
// 2026-07-26: Phase 3 — Produces dom-shim.mjs and .d.ts from view bindings
// and WASM state layout. The JS shim reads WASM linear memory at known
// offsets — no JS watchers, no Proxy, no polling.
//
// Architecture:
//   The WASM module exposes a state_layout() function that returns a pointer
//   to a struct in linear memory describing each state field's offset, size,
//   and type. The JS shim reads this at init time to build its binding table.
//   At each transaction commit (term;), the WASM module calls __web_flush_state
//   with a batch of (field_handle, value_ptr, value_len) tuples. JS applies
//   the DOM mutations synchronously and returns.
//
// Trade-off: One FFI crossing per transaction (not per field). This is optimal
// for transactions with 1-5 field mutations (the common case). For transactions
// with 20+ field mutations, a single large batch still beats per-field crossings.
// See docs/architecture/features/rendered-briev-wasm.md.

use std::collections::{HashMap, HashSet};

/// 2026-08-11 (housekeeping, Part 2): convert a Briev literal token to a JS
/// literal — `'str'`/`"str"` → a quoted JS string, `true`/`false` → booleans,
/// numbers pass through.
fn js_literal(s: &str) -> String {
    let s = s.trim();
    if (s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2)
        || (s.starts_with('"') && s.ends_with('"') && s.len() >= 2)
    {
        format!("\"{}\"", &s[1..s.len() - 1])
    } else {
        s.to_string()
    }
}

/// Descriptor for a single state field in WASM linear memory.
/// 2026-07-26: Phase 3 — Emitted by the LLVM backend in state_layout export.
#[derive(Debug, Clone)]
pub struct FieldLayout {
    /// Unique handle for this field. Matches bindings in the view compiler output.
    pub field_handle: u32,
    /// 2026-08-10: The Briev field NAME (e.g. "count"). Compile-time only — the
    /// WASM table carries handle/offset/size/tag, but the Rust-side layout adds
    /// the name so view bindings (whose `signal` is a field name) can map to
    /// handles. Absent in old hardcoded layouts (empty string).
    pub name: String,
    /// Byte offset of this field in WASM linear memory.
    pub offset: u32,
    /// Byte size of this field's value.
    pub size: u32,
    /// 2026-08-11 (Phase 2a3): per-element byte width — a vector field
    /// (`[N x i32]` on wasm32) reports the element width so b-each can derive
    /// the item count from `size`. Non-vector fields report `size`.
    pub element_size: u32,
    /// Type tag for interpreting the value bytes.
    pub type_tag: TypeTag,
}

/// Type tag for interpreting state field values.
/// 2026-07-26: Phase 3 — Used by JS shim to decode value bytes appropriately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeTag {
    Int,
    Float,
    Bool,
    String,
}

impl TypeTag {
    /// 2026-08-10: Derive the JS type tag from a Briev TYPE via its protocol
    /// category (Cast. lanes) — never by matching type names (rule 18).
    /// Matches the webstack normalizer's js_type mapping: Int/UInt/Float →
    /// number, Bool → boolean, String/Char/Data → string, everything else
    /// (structs, collections, Ptr, ...) → Int (the shim's raw-word default).
    pub fn from_protocol_category(cat: Option<&str>) -> TypeTag {
        match cat {
            Some("Int" | "UInt" | "Float") => TypeTag::Int,
            Some("Bool") => TypeTag::Bool,
            Some("String" | "Char" | "Blob") => TypeTag::String,
            _ => TypeTag::Int,
        }
    }

    fn js_type_name(&self) -> &'static str {
        match self {
            TypeTag::Int => "number",
            TypeTag::Float => "number",
            TypeTag::Bool => "boolean",
            TypeTag::String => "string",
        }
    }

    fn decoder_expr(&self) -> &'static str {
        match self {
            TypeTag::Int => "mem.getInt32(valPtr, true)",
            TypeTag::Float => "mem.getFloat64(valPtr, true)",
            TypeTag::Bool => "mem.getUint8(valPtr, true) !== 0",
            TypeTag::String => {
                "valLen > 0 ? new TextDecoder().decode(new Uint8Array(mem.buffer, valPtr, valLen)) : null"
            }
        }
    }
}

/// Describes the layout of all state fields in WASM linear memory.
/// 2026-07-26: Phase 3 — Read by the JS shim at init time from state_layout() export.
#[derive(Debug, Clone)]
pub struct StateLayout {
    /// Description of the application for naming.
    pub app_name: String,
    /// Offset of the generation counter (u32) in WASM linear memory.
    pub generation_offset: u32,
    /// Offset of the flush buffer in WASM linear memory.
    pub flush_buffer_offset: u32,
    /// Maximum number of entries the flush buffer can hold.
    pub max_flush_entries: u32,
    /// Per-field layout descriptors.
    pub fields: Vec<FieldLayout>,
}

/// Output of the GlueWebGenerator.
/// 2026-07-26: Phase 3 — Two files: the JS runtime shim and TS declarations.
#[derive(Debug, Clone)]
pub struct GlueWebOutput {
    /// JavaScript module: dom-shim.mjs — WasmDomRuntime class.
    pub dom_shim: String,
    /// TypeScript declarations: app.d.ts.
    pub dts: String,
}

/// Generates the JS runtime shim and TS declarations for a WASM-first webstack app.
///
/// 2026-07-26: Phase 3 — Takes compile-time data (state layout, view bindings,
/// protocol mappings) and produces the minimal JS shim that:
///   1. Instantiates the WASM module with __web_flush_state import
///   2. Reads state_layout() export at init to build the binding table
///   3. Applies DOM mutations when __web_flush_state is called
///   4. (Phase 6) Generates frgn from #Web import stubs for DOM/Canvas operations
///      and wraps render_frame in requestAnimationFrame loop when present.
pub struct GlueWebGenerator {
    /// The compiled WASM module bytes (or empty during testing).
    wasm_module: Vec<u8>,
    /// View bindings from the view compiler.
    bindings: Vec<crate::view_compiler::Binding>,
    /// Compile-time state layout.
    state_layout: StateLayout,
    /// Protocol mappings from GLUE config (for type resolution).
    protocol_mappings: HashMap<String, crate::glue::config::ProtocolEntry>,
    /// 2026-07-26: Phase 6 — Foreign function declarations using from #Web protocol.
    /// Each produces a JS import stub in the WASM instantiation's import object.
    frgn_decls: Vec<crate::ast::top::ForeignBinding>,
    /// 2026-08-11 (Phase 2a2, SPEC 21.4): `b-bind:value` input routing. Maps a
    /// bound field to the UNIQUE transaction that writes it (the write-contract
    /// proof) plus the JS marshalling category for the transaction's sole
    /// parameter. Resolved at build time from the transition-graph write sets
    /// (the same source the flush batch covers) — never guessed at runtime.
    bind_routes: HashMap<String, BindRoute>,
    /// 2026-08-12 (Iterable protocol, slice 4): `b-each` iterable fields that
    /// are generic collections — the shim renders them from a
    /// `__view_items_<field>()` snapshot instead of vector layout bytes.
    collection_iterables: HashSet<String>,
    /// 2026-08-12 (slice 4): the subset whose ELEMENT type is String — the
    /// shim decodes each snapshot word as a `[len][bytes]` string pointer.
    collection_string_iterables: HashSet<String>,
}

/// JS marshalling category for a `b-bind:value` transaction parameter,
/// derived from the Briev parameter type (type-driven — no name matching).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamKind {
    /// String — write via `_writeString(value)` (pointer into WASM memory).
    String,
    /// Int / Float — `Number(value)`.
    Number,
    /// Bool — checkbox `.checked`.
    Bool,
}

impl ParamKind {
    /// 2026-08-11 (Phase 2a2): derive the marshalling category from the
    /// existing type-tag machinery (protocol category → TypeTag → ParamKind).
    /// No Briev type-name matching.
    pub fn from_type_tag(tag: TypeTag) -> ParamKind {
        match tag {
            TypeTag::String => ParamKind::String,
            TypeTag::Bool => ParamKind::Bool,
            TypeTag::Int | TypeTag::Float => ParamKind::Number,
        }
    }
}

/// Build-time routing for a `b-bind:value` binding.
#[derive(Debug, Clone, PartialEq)]
pub struct BindRoute {
    /// The transaction the input event fires on each change.
    pub txn: String,
    /// How the input value is marshalled into the transaction's parameter.
    pub param_kind: ParamKind,
}

impl GlueWebGenerator {
    /// Create a new GlueWebGenerator with compile-time data.
    /// 2026-07-26: Phase 3 — `wasm_module` may be empty during testing.
    /// 2026-07-26: Phase 6 — Add frgn_decls for from #Web import stub generation.
    /// 2026-08-11 (Phase 2a2): b-bind routes are set via `with_bind_routes`
    /// (keeps `new` at 5 params — Praetor Datalog Rule 4).
    pub fn new(
        wasm_module: Vec<u8>,
        bindings: Vec<crate::view_compiler::Binding>,
        state_layout: StateLayout,
        protocol_mappings: HashMap<String, crate::glue::config::ProtocolEntry>,
        frgn_decls: Vec<crate::ast::top::ForeignBinding>,
    ) -> Self {
        GlueWebGenerator {
            wasm_module,
            bindings,
            state_layout,
            protocol_mappings,
            frgn_decls,
            bind_routes: HashMap::new(),
            collection_iterables: HashSet::new(),
            collection_string_iterables: HashSet::new(),
        }
    }

    /// 2026-08-11 (Phase 2a2): set the resolved `b-bind:value` routes.
    pub fn with_bind_routes(
        mut self,
        routes: HashMap<String, BindRoute>,
    ) -> Self {
        self.bind_routes = routes;
        self
    }

    /// 2026-08-12 (Iterable protocol, slice 4): the collection `b-each`
    /// iterable fields (rendered from a snapshot materializer).
    pub fn with_collection_iterables(mut self, fields: HashSet<String>) -> Self {
        self.collection_iterables = fields;
        self
    }

    /// 2026-08-12 (slice 4): the collection `b-each` iterables whose ELEMENT
    /// type is String — the shim decodes snapshot words as strings.
    pub fn with_collection_string_iterables(mut self, fields: HashSet<String>) -> Self {
        self.collection_string_iterables = fields;
        self
    }

    /// Generate the JS runtime shim and TS declarations.
    /// 2026-07-26: Phase 3 — Produces ES module with WasmDomRuntime class.
    pub fn generate(&self) -> Result<GlueWebOutput, String> {
        let dom_shim = self.generate_dom_shim();
        let dts = self.generate_dts();
        Ok(GlueWebOutput { dom_shim, dts })
    }

    /// Generate the dom-shim.mjs content.
    /// 2026-07-26: Phase 3 — Two sections:
    ///   1. WasmDomRuntime class (instantiate WASM, read layout, apply flushes)
    ///   2. createApp factory function
    fn generate_dom_shim(&self) -> String {
        let bindings_js = self.generate_binding_table();
        let imports_js = self.generate_imports();

        format!(
            r#"// dom-shim.mjs — Auto-generated GLUE web runtime for {app_name}
// Reads WASM linear memory at known state offsets.

export class WasmDomRuntime {{
  constructor(wasmBytes) {{
    this._handles = [null];
    this._bindingTable = [];
    // 2026-08-11 (Phase 2a): per-handle view-effect lists. A field can be
    // bound by many elements/directives (`b-text` + `b-when` on different
    // nodes, or two elements reading the same field) — a single override slot
    // per handle would clobber earlier bindings. The default applyFn fans the
    // value out to every registered effect.
    this._viewEffects = new Map();
    this._instance = null;
    this._memory = null;
    this._generationOffset = {generation_offset};
    this._flushBufferOffset = {flush_buffer_offset};
    this._maxFlushEntries = {max_flush_entries};
    this._init(wasmBytes);
  }}

  async _init(wasmBytes) {{
    const importObject = {{
      env: this._buildImports(),
    }};
    const wasm = await WebAssembly.instantiate(wasmBytes, importObject);
    this._instance = wasm.instance;
    this._memory = wasm.instance.exports.memory;
    // 2026-08-12 (Iterable protocol, slice 4): run init_state into the
    // long-lived @__web_state and remember its pointer — EVERY txn export
    // takes `%state` as its first param, and the shim must pass it (the
    // exports were previously called with no state, silently operating on
    // garbage at the wasm heap base).
    if (this._instance.exports.__web_boot) this._instance.exports.__web_boot();
    this._statePtr = this._instance.exports.__briev_state_ptr
      ? this._instance.exports.__briev_state_ptr()
      : 0;
    this._loadStateLayout();
    this._startRenderLoop();
  }}

  _buildImports() {{
    return {{
      __web_flush_state: (updatesPtr, count) => {{
        this._applyFlush(updatesPtr, count);
      }},
{imports_js}
    }};
  }}

  _loadStateLayout() {{
    const layoutPtr = this._instance.exports.state_layout();
    const mem = new DataView(this._memory.buffer);
    // state_layout table layout:
    //   field_count:    u32 at layoutPtr + 0
    //   generation_off: u32 at layoutPtr + 4
    //   flush_off:      u32 at layoutPtr + 8
    //   max_entries:    u32 at layoutPtr + 12
    //   fields[]:       16 bytes each at layoutPtr + 16
    const fieldCount = mem.getUint32(layoutPtr + 0, true);
    // re-read offsets from WASM module's declared layout
    const genOff = mem.getUint32(layoutPtr + 4, true);
    const flushOff = mem.getUint32(layoutPtr + 8, true);
    if (genOff !== undefined) this._generationOffset = genOff;
    if (flushOff !== undefined) this._flushBufferOffset = flushOff;
    this._bindingTable = [];
    for (let i = 0; i < fieldCount; i++) {{
      const off = layoutPtr + 16 + i * 16;
      const handle = mem.getUint32(off + 0, true);
      const fieldOff = mem.getUint32(off + 4, true);
      const fieldSize = mem.getUint32(off + 8, true);
      const typeTag = mem.getUint32(off + 12, true);
      this._bindingTable[handle] = this._makeBinding(handle, fieldOff, fieldSize, typeTag);
    }}
{bindings_js}
  }}

  _makeBinding(handle, fieldOff, fieldSize, typeTag) {{
    const _this = this;
    // 2026-08-10: type-aware value decoding. The flush record's value_ptr
    // points at the field's %State slot: scalars decode from the slot bytes
    // (Int/Float/Bool), String dereferences the pointer stored in the slot.
    let decode;
    if (typeTag === 1) {{
      decode = (valPtr, valLen) => {{
        const mem = new DataView(_this._memory.buffer);
        return mem.getFloat64(valPtr, true);
      }};
    }} else if (typeTag === 2) {{
      decode = (valPtr, valLen) => {{
        const mem = new DataView(_this._memory.buffer);
        return mem.getUint8(valPtr, true) !== 0;
      }};
    }} else if (typeTag === 3) {{
      decode = (valPtr, valLen) => {{
        // The slot holds the string ADDRESS (i64); read it, then the
        // [len][bytes] payload at that address.
        const mem = new DataView(_this._memory.buffer);
        const strPtr = Number(mem.getBigUint64(valPtr, true));
        return _this._readString(strPtr);
      }};
    }} else {{
      decode = (valPtr, valLen) => {{
        const mem = new DataView(_this._memory.buffer);
        return mem.getInt32(valPtr, true);
      }};
    }}
    return {{
      handle,
      fieldOff,
      typeTag,
      decode,
      // 2026-08-11 (Phase 2a3): applyFn receives the flush's value_ptr/value_len
      // too — the b-each renderer reads raw vector slots from WASM (the decoded
      // scalar would lose the array base address). Existing effects ignore the
      // extra args.
      applyFn: function(value, valPtr, valLen) {{
        const fns = _this._viewEffects.get(handle);
        if (fns) for (const fn of fns) fn(value, valPtr, valLen);
      }},
    }};
  }}

  _readString(ptr) {{
    if (!ptr) return null;
    const mem = new Uint8Array(this._memory.buffer);
    // Briev strings: [i64 length][data\0] — read length, then slice data
    const lenView = new DataView(this._memory.buffer);
    const len = Number(lenView.getBigUint64(ptr, true));
    const bytes = mem.slice(Number(ptr) + 8, Number(ptr) + 8 + len);
    return new TextDecoder().decode(bytes);
  }}

  _writeString(str) {{
    // Allocate WASM memory for a Briev string (i64 length + bytes + \0)
    // and return the pointer. Uses Module.malloc or pre-allocated buffer.
    const encoder = new TextEncoder();
    const bytes = encoder.encode(str);
    const ptr = this._instance.exports.malloc
      ? this._instance.exports.malloc(8 + bytes.length + 1)
      : this._allocateStatic(8 + bytes.length + 1);
    const mem = new DataView(this._memory.buffer);
    mem.setBigUint64(ptr, BigInt(bytes.length), true);
    const arr = new Uint8Array(this._memory.buffer);
    arr.set(bytes, ptr + 8);
    arr[ptr + 8 + bytes.length] = 0;
    return ptr;
  }}

  _allocateStatic(size) {{
    // Fallback: bump-allocate from a known free region.
    // Overridden by the WASM module's malloc when available.
    if (!this._heapPtr) this._heapPtr = 65536;
    const ptr = this._heapPtr;
    this._heapPtr += size;
    return ptr;
  }}

  _startRenderLoop() {{
    if (this._instance.exports.render_frame) {{
      const loop = () => {{
        this._instance.exports.render_frame();
        requestAnimationFrame(loop);
      }};
      requestAnimationFrame(loop);
    }}
  }}

  _serializeState() {{
    const mem = new DataView(this._memory.buffer);
    const state = {{}};
    for (const binding of this._bindingTable) {{
      if (binding) {{
        state[binding.handle] = mem.getUint32(binding.offset, true);
      }}
    }}
    return state;
  }}

  _deserializeState(state) {{
    const mem = new DataView(this._memory.buffer);
    for (const [handle, val] of Object.entries(state)) {{
      const binding = this._bindingTable[Number(handle)];
      if (binding) {{
        mem.setUint32(binding.offset, val, true);
      }}
    }}
  }}

  async hotReload(wasmBytes) {{
    const oldState = this._serializeState();
    const oldHandles = this._handles.slice();
    const newRuntime = new WasmDomRuntime(wasmBytes);
    newRuntime._handles = oldHandles;
    newRuntime._deserializeState(oldState);
    this._instance = newRuntime._instance;
    this._memory = newRuntime._memory;
    this._bindingTable = newRuntime._bindingTable;
    this._loadStateLayout();
  }}

  _applyFlush(updatesPtr, count) {{
    const mem = new DataView(this._memory.buffer);
    let off = updatesPtr;
    for (let i = 0; i < count; i++) {{
      const handle = mem.getUint32(off, true); off += 4;
      const valPtr = mem.getUint32(off, true); off += 4;
      const valLen = mem.getUint32(off, true); off += 4;
      const binding = this._bindingTable[handle];
      if (binding) {{
        // 2026-08-10: decode by the field's type tag (Int/Float/Bool raw
        // values from the slot; String dereferences the stored pointer).
        const value = binding.decode ? binding.decode(valPtr, valLen) : null;
        // 2026-08-11 (Phase 2a3): pass valPtr/valLen so b-each can read the
        // raw vector slots.
        binding.applyFn(value, valPtr, valLen);
      }}
    }}
  }}

  _registerViewEffect(handle, fn) {{
    if (!this._viewEffects.has(handle)) this._viewEffects.set(handle, []);
    this._viewEffects.get(handle).push(fn);
  }}

  _txn(name) {{
    // 2026-08-11 (Phase 2a3): a callable txn exports as `@<name>`, a reactive
    // txn as `@txn_<name>`. Resolve either so the DOM can fire both.
    // 2026-08-12 (slice 4): prepend the state pointer — every txn export
    // takes `%state` as its first param.
    const fn = this._instance.exports[name] || this._instance.exports["txn_" + name];
    if (!fn) return fn;
    return (...args) => fn(this._statePtr, ...args);
  }}

  get generation() {{
    if (!this._memory) return 0;
    const mem = new DataView(this._memory.buffer);
    return mem.getUint32(this._generationOffset, true);
  }}

  set generation(val) {{
    if (!this._memory) return;
    const mem = new DataView(this._memory.buffer);
    mem.setUint32(this._generationOffset, val, true);
  }}
}}

export async function createApp(wasmBytes) {{
  const runtime = new WasmDomRuntime(wasmBytes);
  return runtime._instance.exports;
}}
"#,
            app_name = self.state_layout.app_name,
            generation_offset = self.state_layout.generation_offset,
            flush_buffer_offset = self.state_layout.flush_buffer_offset,
            max_flush_entries = self.state_layout.max_flush_entries,
            bindings_js = bindings_js,
            imports_js = imports_js,
        )
    }

    /// 2026-08-12 (Iterable protocol, slice 4): a `b-each` over a COLLECTION
    /// iterable. The shim calls the `__view_items_<field>(statePtr)` wasm
    /// materializer — which drives the collection's own op Count/op At into a
    /// `[len][word…]` snapshot the shim CAN index — and renders one template
    /// clone per item, reconciled by key, exactly like the vector renderer.
    /// Element words are i64 (Int items for now; String/struct elements are a
    /// documented follow-up that decodes each word by the element type tag).
    fn emit_collection_each(
        &self,
        iterable: &str,
        item_name: &str,
        template_html: &str,
        container_id: &str,
        item_bindings: &[crate::view_compiler::ItemBinding],
        key_expr: &str,
        el: &str,
    ) -> String {
        if key_expr.trim() != item_name {
            return String::new();
        }
        let Some(handle) = self.field_handle_for_signal(iterable) else {
            return String::new();
        };
        let ibs: Vec<String> = item_bindings
            .iter()
            .map(|ib| {
                let (kind, expr) = match &ib.directive {
                    crate::view_compiler::ItemDirective::Text { signal } => ("text", signal.as_str()),
                    crate::view_compiler::ItemDirective::Class { .. } => ("class", ""),
                    crate::view_compiler::ItemDirective::Show { expr } => ("show", expr.as_str()),
                    crate::view_compiler::ItemDirective::When { expr } => ("when", expr.as_str()),
                    crate::view_compiler::ItemDirective::Trigger { .. } => ("trigger", ""),
                };
                let extra = match &ib.directive {
                    crate::view_compiler::ItemDirective::Trigger { event, txn } => {
                        format!(", event: {event:?}, txn: {txn:?}")
                    }
                    crate::view_compiler::ItemDirective::Class { pairs } => {
                        let cls: Vec<String> = pairs
                            .iter()
                            .map(|(cls_name, cls_expr)| format!("({cls_name:?}, {cls_expr:?})"))
                            .collect();
                        format!(", cls: [{}]", cls.join(", "))
                    }
                    _ => String::new(),
                };
                format!(
                    "{{ marker: {m}, kind: {kind:?}, expr: {expr:?}{extra} }}",
                    m = ib.marker
                )
            })
            .collect();
        let ibs_js = format!("[{}]", ibs.join(", "));
        let materializer = format!("__view_items_{}", iterable);
        // 2026-08-12 (slice 4): a String-element collection materializes the
        // string POINTERS — the shim reads each `[len][bytes]` string. An Int
        // element is the raw number.
        let is_string_items = self.collection_string_iterables.contains(iterable);
        let item_reader = if is_string_items {
            "const __word = Number(dv.getBigInt64(snapPtr + 8 + i * 8, true));\
             const __sp = Number(BigInt.asUintN(32, BigInt(__word)));\
             const __len = Number(dv.getBigInt64(__sp, true));\
             const __bytes = new Uint8Array(this._memory.buffer, __sp + 8, __len);\
             const item = String.fromCharCode(...__bytes);".to_string()
        } else {
            "const item = Number(dv.getBigInt64(snapPtr + 8 + i * 8, true));".to_string()
        };
        format!(
            "(() => {{\n\
             \x20         const anchor = {el};\n\
             \x20         if (!anchor) return;\n\
             \x20         const container = anchor.parentNode;\n\
             \x20         if (!container) return;\n\
             \x20         const tagName = anchor.tagName.toLowerCase();\n\
             \x20         const templateInner = {template_html:?};\n\
             \x20         anchor.remove();\n\
             \x20         const itemBindings = {ibs_js};\n\
             \x20         const evalItem = (expr, item) => {{\n\
             \x20           if (expr === {item_name:?}) return Boolean(item);\n\
             \x20           for (const op of [\"==\", \"!=\", \"<=\", \">=\", \"<\", \">\"]) {{\n\
             \x20             const i = expr.indexOf(op);\n\
             \x20             if (i < 0) continue;\n\
             \x20             const l = expr.slice(0, i).trim(), r = expr.slice(i + op.length).trim();\n\
             \x20             const lv = l === {item_name:?} ? item : Number(l);\n\
             \x20             const rv = r === {item_name:?} ? item : Number(r);\n\
             \x20             switch (op) {{\n\
             \x20               case \"==\": return lv === rv;\n\
             \x20               case \"!=\": return lv !== rv;\n\
             \x20               case \"<=\": return lv <= rv;\n\
             \x20               case \">=\": return lv >= rv;\n\
             \x20               case \"<\": return lv < rv;\n\
             \x20               case \">\": return lv > rv;\n\
             \x20             }}\n\
             \x20           }}\n\
             \x20           return Boolean(item);\n\
             \x20         }};\n\
             \x20         const applyItem = (el, item) => {{\n\
             \x20           for (const ib of itemBindings) {{\n\
             \x20             const target = ib.marker === 0 ? el : el.querySelector('[data-itm=\"' + ib.marker + '\"]');\n\
             \x20             if (!target) continue;\n\
             \x20             if (ib.kind === \"text\") target.textContent = evalItem(ib.expr, item);\n\
             \x20             else if (ib.kind === \"show\") target.style.display = evalItem(ib.expr, item) ? '' : 'none';\n\
             \x20             else if (ib.kind === \"when\") target.style.display = evalItem(ib.expr, item) ? '' : 'none';\n\
             \x20             else if (ib.kind === \"trigger\") {{\n\
             \x20               target.addEventListener(ib.event, () => this._txn(ib.txn)(item));\n\
             \x20             }}\n\
             \x20           }}\n\
             \x20         }};\n\
             \x20         let rendered = new Map();\n\
             \x20         this._registerViewEffect({handle}, () => {{\n\
             \x20           const snapPtr = this._instance.exports['{materializer}'](this._statePtr);\n\
             \x20           if (!snapPtr) return;\n\
             \x20           const dv = new DataView(this._memory.buffer);\n\
              \x20           const n = Number(dv.getBigInt64(snapPtr, true));\n\
              \x20           const seen = new Set();\n\
              \x20           for (let i = 0; i < n; i++) {{\n\
              \x20             {item_reader}\n\
              \x20             const key = String(item);\n\
              \x20             seen.add(key);\n\
              \x20             let el2 = rendered.get(key);\n\
              \x20             if (!el2) {{\n\
              \x20               el2 = document.createElement(tagName);\n\
              \x20               el2.innerHTML = templateInner;\n\
              \x20               applyItem(el2, item);\n\
              \x20               container.appendChild(el2);\n\
             \x20               rendered.set(key, el2);\n\
             \x20             }}\n\
             \x20           }}\n\
             \x20           for (const [key, el2] of rendered) {{\n\
             \x20             if (!seen.has(key)) {{\n\
             \x20               el2.remove();\n\
             \x20               rendered.delete(key);\n\
             \x20             }}\n\
             \x20           }}\n\
             \x20         }});\n\
             \x20       }})();",
            handle = handle,
            template_html = template_html,
            materializer = materializer,
            item_reader = item_reader,
        )
    }

    /// Generate per-binding apply functions from view compiler bindings.
    /// 2026-07-26: Phase 3 — Each binding directive (Text, Show, Trigger, etc.)
    /// produces a JS apply function that wires the binding-table entry for the
    /// bound state field to the DOM element.
    /// 2026-08-10: real wiring — signal (a Briev field name) is mapped to the
    /// state_layout handle, and the emitted JS overrides that handle's
    /// binding-table applyFn with the DOM mutation. Previously a placeholder
    /// (comments only). Trigger directives wire event listeners that call the
    /// WASM transaction export. Emitted inside _loadStateLayout so the default
    /// _makeBinding entries exist before the overrides.
    fn generate_binding_table(&self) -> String {
        if self.bindings.is_empty() {
            return String::new();
        }

        let mut out = String::new();
        for binding in &self.bindings {
            let js = self.binding_to_js(binding);
            out.push_str(&format!("    // binding: {} {:?}\n", binding.element_id, binding.directive));
            if !js.is_empty() {
                out.push_str(&format!("    {}\n", js));
            }
        }
        out
    }

    /// Map a binding's signal (Briev field name) to its state_layout handle.
    /// 2026-08-10: the state_layout fields carry `name`; a binding whose signal
    /// names a field resolves to that field's handle. Unresolved signals (e.g.
    /// compound expressions) fall back to `None` — the binding is left to the
    /// default (log-only) applyFn rather than guessed.
    /// 2026-08-11: resolve a directive signal to its state-layout handle.
    /// A view expression like `items.^Size` binds to the root field `items` —
    /// the `.^X` reflection suffix is a projection applied on top of the
    /// field's value, so handle lookup uses the head only. Shared root_signal
    /// lives in view_compiler.rs (also used by verify_srbv — DRY).
    fn field_handle_for_signal(&self, signal: &str) -> Option<u32> {
        let (root, _) = crate::view_compiler::root_signal(signal);
        self.state_layout.fields.iter()
            .find(|f| f.name == root)
            .map(|f| f.field_handle)
    }

    /// Convert a single view binding to a JS apply function.
    /// 2026-08-11 (Phase 2a): every per-field binding registers a view effect
    /// with `_registerViewEffect(handle, fn)` — the default applyFn fans each
    /// flush value out to ALL registered effects, so two elements binding the
    /// same field (or `b-text` + `b-when` on one field) both react. Previously
    /// each binding replaced `this._bindingTable[H].applyFn`, and the last one
    /// emitted silently won.
    fn binding_to_js(&self, binding: &crate::view_compiler::Binding) -> String {
        use crate::view_compiler::Directive;
        let element = binding.element_id.trim();
        let el = format!("document.getElementById({:?})", element);
        match &binding.directive {
            Directive::Text { signal } => {
                let (root, proj) = crate::view_compiler::root_signal(signal);
                let Some(handle) = self.field_handle_for_signal(root) else {
                    return String::new();
                };
                // 2026-08-11: apply `.^X` reflection projections inline in JS.
                // `.^Length` on a string/collection → `.length`; other
                // reflections pass through as a property access. 2026-08-14
                // (§6b.5): a `Count#(field)` view signal projects `Count` →
                // the materialized array's `.length`. `.^Size` is deleted.
                let mut apply_value = "value".to_string();
                for p in &proj {
                    match *p {
                        "Length" | "Count" => apply_value.push_str(".length"),
                        other => {
                            apply_value.push('.');
                            apply_value.push_str(other);
                        }
                    }
                }
                format!(
                    "this._registerViewEffect({handle}, (value) => {{\n\
                     \x20         const el = {el};\n\
                     \x20         if (el) el.textContent = {apply_value};\n\
                     \x20       }});"
                )
            }
            Directive::Show { expr } | Directive::Hide { expr } => {
                // 2026-08-12 (2b3): `b-show="step == 0"` binds the ROOT field
                // (first token) and the shim EVALUATES the comparison on flush —
                // previously only the raw value was tested (`step != 0` showed
                // the element). Literal-only exprs have no field handle.
                let (root, _) = crate::view_compiler::condition_root_signal(expr);
                let Some(handle) = self.field_handle_for_signal(root) else {
                    return String::new();
                };
                let hidden = matches!(binding.directive, Directive::Hide { .. });
                let cond = Self::simple_expr_js(expr, "value");
                format!(
                    "this._registerViewEffect({handle}, (value) => {{\n\
                     \x20         const el = {el};\n\
                     \x20         if (el) el.style.display = ({hidden} ? !({cond}) : ({cond})) ? 'none' : '';\n\
                     \x20       }});",
                    hidden = if hidden { "true" } else { "false" },
                    cond = cond
                )
            }
            Directive::When { expr } => {
                // 2026-08-11 (Phase 2a, SPEC 21.4): `b-when` structurally
                // mounts/unmounts the subtree — the element is present iff the
                // condition field is truthy. The element starts in the DOM as
                // authored (consistent with every other binding: the DOM shows
                // authored content until the first flush). On the first falsy
                // flush the element is detached, its position marked with a
                // comment anchor, and a template snapshot kept; truthy flushes
                // re-insert a fresh clone (identity is NOT preserved — that is
                // the `b-show` distinction). The IIFE closes over the per-node
                // mount state so it survives across flushes.
                let (root, _) = crate::view_compiler::condition_root_signal(expr);
                let Some(handle) = self.field_handle_for_signal(root) else {
                    return String::new();
                };
                // 2026-08-12 (2b3): the shim EVALUATES the b-when comparison on
                // flush (`step == 0` → `value == 0`) — the raw value alone
                // would mount on `step != 0`.
                let cond = Self::simple_expr_js(expr, "value");
                format!(
                    "(() => {{\n\
                     \x20         const el = {el};\n\
                     \x20         if (!el) return;\n\
                     \x20         let anchor = null;\n\
                     \x20         let template = null;\n\
                     \x20         let mounted = true;\n\
                     \x20         this._registerViewEffect({handle}, (value) => {{\n\
                     \x20           const show = ({cond}) ? true : false;\n\
                      \x20           if (show && !mounted) {{\n\
                      \x20             anchor.parentNode.insertBefore(template.cloneNode(true), anchor);\n\
                      \x20             mounted = true;\n\
                      \x20           }} else if (!show && mounted) {{\n\
                      \x20             template = template || el.cloneNode(true);\n\
                      \x20             anchor = document.createComment('b-when');\n\
                      \x20             el.parentNode.insertBefore(anchor, el);\n\
                      \x20             // 2026-08-12 (2b3 slice 3): unmounting a subtree\n\
                      \x20             // releases any component instances inside it — fire their\n\
                      \x20             // reset TXN (contract + flush) so a remount starts fresh.\n\
                      \x20             el.querySelectorAll('[data-instance]').forEach((m) => {{\n\
                      \x20               const inst = m.getAttribute('data-instance');\n\
                      \x20               const reset = this._txn('__reset_' + inst.replace('.', '_'));\n\
                      \x20               if (reset) reset();\n\
                      \x20             }});\n\
                      \x20             el.remove();\n\
                      \x20             mounted = false;\n\
                      \x20           }}\n\
                      \x20         }});\n\
                      \x20       }})();",
                    cond = cond
                 )
            }
            Directive::Bind { target } => {
                // 2026-08-11 (Phase 2a2, SPEC 21.4): `b-bind:value="field"` wires
                // the `input` event to the UNIQUE transaction that writes the
                // field (resolved at build time — the write-contract proof).
                // The value is marshalled by the transaction's sole parameter
                // type: String → `_writeString` (pointer), Int/Float → Number,
                // Bool → checkbox `.checked`. No silent guesses — an unresolvable
                // route is a build-time error, not an inert input.
                let Some(route) = self.bind_routes.get(target) else {
                    return String::new();
                };
                let route_txn = &route.txn;
                match route.param_kind {
                    ParamKind::String => {
                        format!(
                            "(() => {{\n\
                             \x20         const el = {el};\n\
                             \x20         if (!el) return;\n\
                             \x20         el.addEventListener('input', () => {{\n\
                             \x20           const ptr = this._writeString(el.value);\n\
                             \x20           this._txn({route_txn:?})(ptr);\n\
                             \x20         }});\n\
                             \x20       }})();"
                        )
                    }
                    ParamKind::Number => {
                        format!(
                            "(() => {{\n\
                             \x20         const el = {el};\n\
                             \x20         if (!el) return;\n\
                             \x20         el.addEventListener('input', () => {{\n\
                             \x20           this._txn({route_txn:?})(Number(el.value) || 0);\n\
                             \x20         }});\n\
                             \x20       }})();"
                        )
                    }
                    ParamKind::Bool => {
                        format!(
                            "(() => {{\n\
                             \x20         const el = {el};\n\
                             \x20         if (!el) return;\n\
                             \x20         el.addEventListener('change', () => {{\n\
                             \x20           this._txn({route_txn:?})(el.checked ? 1 : 0);\n\
                             \x20         }});\n\
                             \x20       }})();"
                        )
                    }
                }
            }
            Directive::Trigger { event, txn, params } => {
                // 2026-08-10: wire a DOM event listener that calls the WASM
                // transaction export (b._instance.exports[<txn>]) with the
                // binding's static params. The listener is attached once, at
                // shim init, outside the per-flush binding table.
                let arg_str = if params.is_empty() {
                    String::new()
                } else {
                    let args: Vec<String> = params.iter().map(|(_, v)| v.clone()).collect();
                    format!(", {}", args.join(", "))
                };
                format!(
                    "(() => {{\n\
                     \x20         const el = {el};\n\
                     \x20         if (el) el.addEventListener({event:?}, () => this._txn({txn:?})({arg_str}));\n\
                     \x20       }})();"
                )
            }
            Directive::Each {
                iterable,
                item_name,
                template_html,
                container_id,
                item_bindings,
                key_expr,
            } => {
                // 2026-08-11 (Phase 2a3): a vector-field iteration renderer.
                // On each flush of the iterable field, read `count` i64 slots
                // from valPtr (the array base), render one fresh template clone
                // per item, and reconcile by key (b-key). Item-scoped
                // directives apply to each clone (marker 0 = the clone root,
                // marker N = [data-itm="N"]).
                let Some(handle) = self.field_handle_for_signal(iterable) else {
                    return String::new();
                };
                let field = self.state_layout.fields.iter().find(|f| f.field_handle == handle);
                let Some(field) = field else { return String::new(); };
                // 2026-08-12 (Iterable protocol, slice 4): a COLLECTION
                // iterable renders from the `__view_items_<field>()` snapshot —
                // the wasm materializer drives the collection's own op Count/
                // op At into `[len][word…]`, which the shim CAN index (the
                // heap List's flush bytes are just the handle and cannot be).
                if self.collection_iterables.contains(iterable.as_str()) {
                    return self.emit_collection_each(
                        iterable.as_str(), item_name.as_str(), template_html.as_str(),
                        container_id.as_str(), item_bindings, key_expr.as_str(), el.as_str(),
                    );
                }
                // 2026-08-11 (housekeeping 1b): only a VECTOR field is
                // iterable — for a scalar field element_size == size (one
                // "element" the whole value), and a heap List is a pointer
                // row the slot reader cannot index. Emit no renderer (the
                // compile-side warning explains; never a wrong render).
                if field.element_size == 0 || field.element_size >= field.size {
                    return String::new();
                }
                // 2026-08-11 (Phase 2a3): item count = field size / element
                // width. Vector slots are width-aware (i32 on wasm32, i64 on
                // x86_64), so the reader + stride follow element_size.
                let elem_size = field.element_size.max(1);
                let count = field.size / elem_size;
                let slot_reader = if elem_size >= 8 {
                    "Number(dv.getBigInt64(valPtr + i * 8, true))"
                } else {
                    "dv.getInt32(valPtr + i * 4, true)"
                };
                // Serialize the item-scoped directives.
                let ibs: Vec<String> = item_bindings
                    .iter()
                    .map(|ib| {
                        let (kind, expr) = match &ib.directive {
                            crate::view_compiler::ItemDirective::Text { signal } => ("text", signal.as_str()),
                            crate::view_compiler::ItemDirective::Class { .. } => ("class", ""),
                            crate::view_compiler::ItemDirective::Show { expr } => ("show", expr.as_str()),
                            crate::view_compiler::ItemDirective::When { expr } => ("when", expr.as_str()),
                            crate::view_compiler::ItemDirective::Trigger { .. } => ("trigger", ""),
                        };
                        let extra = match &ib.directive {
                            crate::view_compiler::ItemDirective::Trigger { event, txn } => {
                                format!(", event: {event:?}, txn: {txn:?}")
                            }
                            crate::view_compiler::ItemDirective::Class { pairs } => {
                                let cls: Vec<String> = pairs
                                    .iter()
                                    .map(|(cls_name, cls_expr)| format!("({cls_name:?}, {cls_expr:?})"))
                                    .collect();
                                format!(", cls: [{}]", cls.join(", "))
                            }
                            _ => String::new(),
                        };
                        format!(
                            "{{ marker: {m}, kind: {kind:?}, expr: {expr:?}{extra} }}",
                            m = ib.marker
                        )
                    })
                    .collect();
                let ibs_js = format!("[{}]", ibs.join(", "));
                // Only the bare item key is supported for scalar vectors.
                if key_expr.trim() != item_name {
                    return String::new();
                }
                format!(
                    "(() => {{\n\
                     \x20         const anchor = {el};\n\
                     \x20         if (!anchor) return;\n\
                     \x20         const container = anchor.parentNode;\n\
                     \x20         if (!container) return;\n\
                     \x20         const tagName = anchor.tagName.toLowerCase();\n\
                     \x20         const templateInner = {template_html:?};\n\
                     \x20         anchor.remove();\n\
                     \x20         const itemBindings = {ibs_js};\n\
                     \x20         const evalItem = (expr, item) => {{\n\
                     \x20           if (expr === {item_name:?}) return Boolean(item);\n\
                     \x20           for (const op of [\"==\", \"!=\", \"<=\", \">=\", \"<\", \">\"]) {{\n\
                     \x20             const i = expr.indexOf(op);\n\
                     \x20             if (i < 0) continue;\n\
                     \x20             const l = expr.slice(0, i).trim(), r = expr.slice(i + op.length).trim();\n\
                     \x20             const lv = l === {item_name:?} ? item : Number(l);\n\
                     \x20             const rv = r === {item_name:?} ? item : Number(r);\n\
                     \x20             switch (op) {{\n\
                     \x20               case \"==\": return lv === rv;\n\
                     \x20               case \"!=\": return lv !== rv;\n\
                     \x20               case \"<=\": return lv <= rv;\n\
                     \x20               case \">=\": return lv >= rv;\n\
                     \x20               case \"<\": return lv < rv;\n\
                     \x20               case \">\": return lv > rv;\n\
                     \x20             }}\n\
                     \x20           }}\n\
                     \x20           return Boolean(item);\n\
                     \x20         }};\n\
                     \x20         const applyItem = (el, item) => {{\n\
                     \x20           for (const ib of itemBindings) {{\n\
                     \x20             const target = ib.marker === 0 ? el : el.querySelector('[data-itm=\"' + ib.marker + '\"]');\n\
                     \x20             if (!target) continue;\n\
                     \x20             if (ib.kind === \"text\") {{\n\
                     \x20               target.textContent = String(item);\n\
                     \x20             }} else if (ib.kind === \"class\") {{\n\
                     \x20               for (const [clsName, clsExpr] of (ib.cls || [])) {{\n\x20                 target.classList.toggle(clsName, evalItem(clsExpr, item));\n\x20               }}\n\
                     \x20             }} else if (ib.kind === \"show\" || ib.kind === \"when\") {{\n\
                     \x20               target.style.display = evalItem(ib.expr, item) ? \"\" : \"none\";\n\
                     \x20             }} else if (ib.kind === \"trigger\") {{\n\
                     \x20               target.addEventListener(ib.event, () => this._txn(ib.txn)(item));\n\
                     \x20             }}\n\
                     \x20           }}\n\
                     \x20         }};\n\
                     \x20         let rendered = new Map();\n\
                     \x20         this._registerViewEffect({handle}, (value, valPtr, valLen) => {{\n\
                     \x20           if (!valPtr) return;\n\
                     \x20           const dv = new DataView(this._memory.buffer);\n\
                     \x20           const n = Math.min({count}, Math.floor(valLen / {elem_size}));\n\
                     \x20           const seen = new Set();\n\
                     \x20           for (let i = 0; i < n; i++) {{\n\
                     \x20             const item = {slot_reader};\n\
                     \x20             const key = String(item);\n\
                     \x20             seen.add(key);\n\
                     \x20             let el = rendered.get(key);\n\
                     \x20             if (!el) {{\n\
                     \x20               el = document.createElement(tagName);\n\
                     \x20               el.innerHTML = templateInner;\n\
                     \x20               applyItem(el, item);\n\
                     \x20               container.appendChild(el);\n\
                     \x20               rendered.set(key, el);\n\
                     \x20             }}\n\
                     \x20           }}\n\
                     \x20           for (const [key, el] of rendered) {{\n\
                     \x20             if (!seen.has(key)) {{\n\
                     \x20               el.remove();\n\
                     \x20               rendered.delete(key);\n\
                     \x20             }}\n\
                     \x20           }}\n\
                     \x20         }});\n\
                     \x20       }})();",
                     count = count,
                    elem_size = elem_size,
                    slot_reader = slot_reader,
                    template_html = template_html,
                )
            }
            Directive::Class { pairs } => {
                // 2026-08-11 (housekeeping, Part 2): `b-class="{ 'cls': expr }"`.
                // Each expr is a bounded single-field form (validated at view
                // compile): dynamic pairs register on the root field's handle
                // (evaluated on flush); literal-only pairs apply once at init.
                 let mut js = String::from("(() => {\n        const el = {el};\n        if (!el) return;\n");
                 for (cls_name, expr) in pairs {
                     match self.bounded_root(expr) {
                         Some((root, None)) if !root.is_empty() => {
                             if let Some(handle) = self.field_handle_for_signal(&root) {
                                 let cond = Self::simple_expr_js(expr, "value");
                                 js.push_str(&format!(
                                     "        this._registerViewEffect({handle}, (value) => {{\n\
                                      \x20         el.classList.toggle({cls_name:?}, {cond});\n\
                                      \x20       }});\n"
                                 ));
                             }
                         }
                         Some((_, Some(_))) => {
                             // Literal-only pair — apply once at init.
                             let cond = Self::simple_expr_js(expr, "");
                             js.push_str(&format!(
                                 "        el.classList.toggle({cls_name:?}, {cond});\n"
                             ));
                         }
                         _ => {}
                     }
                 }
                 js.push_str("      })();");
                 js
             }
             Directive::Style { name, value } => {
                 // 2026-08-11 (housekeeping, Part 2): `b-style="name: value"`.
                 let mut js = format!(
                     "(() => {{\n\
                      \x20         const el = {el};\n\
                      \x20         if (!el) return;\n"
                 );
                 match self.bounded_root(value) {
                     Some((root, None)) if !root.is_empty() => {
                         if let Some(handle) = self.field_handle_for_signal(&root) {
                             js.push_str(&format!(
                                 "        this._registerViewEffect({handle}, (value) => {{\n\
                                  \x20         el.style[{name:?}] = value;\n\
                                  \x20       }});\n"
                             ));
                         }
                     }
                     Some((_, Some(_))) => {
                         let lit = js_literal(value);
                         js.push_str(&format!("        el.style[{name:?}] = {lit};\n"));
                     }
                     _ => {}
                 }
                 js.push_str("      })();");
                 js
             }
             Directive::Attr { name, value } => {
                 // 2026-08-11 (housekeeping, Part 2): `b-attr="name: value"`.
                 let mut js = format!(
                     "(() => {{\n\
                      \x20         const el = {el};\n\
                      \x20         if (!el) return;\n"
                 );
                 match self.bounded_root(value) {
                     Some((root, None)) if !root.is_empty() => {
                         if let Some(handle) = self.field_handle_for_signal(&root) {
                             js.push_str(&format!(
                                 "        this._registerViewEffect({handle}, (value) => {{\n\
                                  \x20         el.setAttribute({name:?}, value);\n\
                                  \x20       }});\n"
                             ));
                         }
                     }
                     Some((_, Some(_))) => {
                         let lit = js_literal(value);
                         js.push_str(&format!("        el.setAttribute({name:?}, {lit});\n"));
                     }
                     _ => {}
                 }
                 js.push_str("      })();");
                 js
             }
             _ => String::new(),
        }
    }

    /// 2026-08-11 (housekeeping, Part 2): the bounded single-field root of a
    /// view expression (the same forms the view compiler validated). Returns
    /// Some(root) for a field-referencing expr, None for a literal-only one.
    fn bounded_root(&self, expr: &str) -> Option<(String, Option<String>)> {
        let e = expr.trim();
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
        if is_identifier(e) && !e.contains('.') {
            return Some((e.to_string(), None));
        }
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
                return None;
            };
            let (l, r) = (e[..i].trim(), e[i + op.len()..].trim());
            if is_identifier(l) && is_literal(r) {
                return Some((l.to_string(), None));
            }
        }
        if is_literal(e) {
            return Some(("".to_string(), Some(e.to_string())));
        }
        None
    }

    /// 2026-08-11 (housekeeping, Part 2): translate a bounded view expression
    /// to a JS boolean/string expression given the flushed `value` variable
    /// (or an empty string for a literal-only eval). `field` → Boolean(value);
    /// `field <op> literal` → the comparison; a bare literal → its JS value.
    fn simple_expr_js(expr: &str, value_var: &str) -> String {
        let e = expr.trim();
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
        // (`p1.show`, `Counter.0.step`) is a field reference → the flushed
        // value. (`items.^Size` projections contain `^` and never reach here.)
        if is_identifier(e) {
            return if value_var.is_empty() {
                "true".to_string()
            } else {
                format!("Boolean({value_var})")
            };
        }
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
                return "false".to_string();
            };
            let (l, r) = (e[..i].trim(), e[i + op.len()..].trim());
            if is_identifier(l) && is_literal(r) {
                return format!("{value_var} {op} {}", js_literal(r));
            }
        }
        js_literal(e)
    }

    /// Generate custom imports section for frgn from #Web handlers.
    ///
    /// 2026-07-26: Phase 6 — Type-driven dispatch. For each frgn declaration
    /// using from #Web, emit a JS import stub in the WASM instantiation's
    /// import object. The stub unmarshals each parameter from WASM ABI to JS
    /// value (using GLUE protocol mappings), calls the native function by its
    /// Briev name, then marshals the return value back to WASM ABI.
    ///
    /// No function name matching — the TYPE determines marshalling:
    ///   String       → _readString(ptr) / _writeString(str)
    ///   Int          → raw i32 val
    ///   Float        → raw f64 val
    ///   Bool         → val !== 0
    ///   #Element      → this._handles[handle]
    ///   #CanvasContext → this._handles[handle]
    ///
    /// The user provides the native JS function implementation (as a module
    /// or global). Future GLUE templates for `[web]` will provide standard
    /// browser API wrappers automatically.
    fn generate_imports(&self) -> String {
        let mut out = String::new();
        for fb in &self.frgn_decls {
            let fn_name = fb.effective_briev_name();
            let param_names = self.frgn_param_names(&fb.inputs);
            let marshal_in = self.frgn_marshal_in(&fb.inputs, &param_names);
            let marshal_out = self.frgn_marshal_out(&fb.success_output);

            // Build the JS stub body: unmarshal params, call native, marshal result
            let mut body = String::new();
            for line in &marshal_in {
                body.push_str(&format!("        {}\n", line));
            }
            // If the function has a non-void return, capture the result
            if fb.success_output.is_empty()
                || matches!(fb.success_output[0].1, crate::ast::Type::Void) {
                body.push_str(&format!("        {}(", fn_name));
                for (i, pn) in param_names.iter().enumerate() {
                    if i > 0 { body.push_str(", "); }
                    body.push_str(pn);
                }
                body.push_str(");\n");
            } else {
                body.push_str(&format!(
                    "        const _result = {}({});\n",
                    fn_name,
                    param_names.join(", "),
                ));
                for line in &marshal_out {
                    body.push_str(&format!("        {}\n", line));
                }
            }

            out.push_str(&format!(
                "      {}({}) => {{\n{}\n      }},\n",
                fn_name,
                param_names.join(", "),
                body,
            ));
        }
        out
    }

    /// Derive JS parameter names from Briev parameter types.
    /// 2026-07-26: Phase 6 — Type-driven naming so the generated JS is readable.
    fn frgn_param_names(&self, inputs: &[(String, crate::ast::Type)]) -> Vec<String> {
        inputs.iter().enumerate().map(|(i, (name, ty))| {
            if !name.is_empty() {
                name.clone()
            } else {
                self.param_name_from_type(ty, i)
            }
        }).collect()
    }

    /// Generate a JS parameter name based on its Briev type.
    fn param_name_from_type(&self, ty: &crate::ast::Type, idx: usize) -> String {
        match ty {
            crate::ast::Type::Ptr(_) => format!("ptr{}", idx),
            crate::ast::Type::Custom(s) if s == "String" => format!("str{}", idx),
            crate::ast::Type::Custom(s) if s == "Int" => format!("val{}", idx),
            crate::ast::Type::Custom(s) if s == "Float" => format!("f{}", idx),
            crate::ast::Type::Custom(s) if s == "Bool" => format!("b{}", idx),
            crate::ast::Type::Custom(s) if s == "Element" || s == "CanvasContext" => format!("handle{}", idx),
            _ => format!("arg{}", idx),
        }
    }

    /// Generate marshal-in statements: convert WASM ABI values to JS values.
    /// 2026-07-26: Phase 6 — Type-driven. Each parameter becomes a JS const
    /// declaration if the type needs decoding (String → _readString, etc.).
    fn frgn_marshal_in(&self, inputs: &[(String, crate::ast::Type)], param_names: &[String])
        -> Vec<String>
    {
        let mut stmts = Vec::new();
        for (i, (_, ty)) in inputs.iter().enumerate() {
            let pn = &param_names[i];
            match ty {
                crate::ast::Type::Custom(s) if s == "String" => {
                    stmts.push(format!("const {} = this._readString({});", pn, pn));
                }
                crate::ast::Type::Custom(s) if s == "Bool" => {
                    stmts.push(format!("const {} = {} !== 0;", pn, pn));
                }
                crate::ast::Type::Custom(s) if s == "Element" || s == "CanvasContext" => {
                    stmts.push(format!("const {} = this._handles[{}];", pn, pn));
                }
                _ => {} // Int, Float pass through as-is
            }
        }
        stmts
    }

    /// Generate marshal-out statements: convert JS return value to WASM ABI.
    /// 2026-07-26: Phase 6 — Type-driven. Handles handle registration,
    /// string allocation, etc.
    fn frgn_marshal_out(&self, outputs: &[(String, crate::ast::Type)]) -> Vec<String> {
        let mut stmts = Vec::new();
        if let Some((_, ty)) = outputs.first() {
            match ty {
                crate::ast::Type::Custom(s) if s == "Element" || s == "CanvasContext" => {
                    stmts.push("return this._handles.push(_result) - 1;".to_string());
                }
                crate::ast::Type::Custom(s) if s == "String" => {
                    stmts.push("return this._writeString(_result);".to_string());
                }
                crate::ast::Type::Custom(s) if s == "Bool" => {
                    stmts.push("return _result ? 1 : 0;".to_string());
                }
                _ if matches!(ty, crate::ast::Type::Void) => {} // void — no return statement
                _ => {
                    stmts.push("return _result;".to_string());
                }
            }
        }
        stmts
    }

    /// Generate the app.d.ts TypeScript declarations.
    /// 2026-07-26: Phase 3 — Declares the WasmDomRuntime class and createApp function.
    fn generate_dts(&self) -> String {
        let txn_methods = self.generate_txn_declarations();

        format!(
            r#"// app.d.ts — Auto-generated TypeScript declarations for {app_name}

export interface WasmDomRuntime {{
  readonly generation: number;
  readonly _instance: WebAssembly.Instance;
  readonly _memory: WebAssembly.Memory;
{txn_methods}}}

export function createApp(wasmBytes: Uint8Array): Promise<WebAssembly.Exports>;
"#,
            app_name = self.state_layout.app_name,
            txn_methods = txn_methods,
        )
    }

    /// Generate TS method declarations for each transaction.
    /// 2026-07-26: Phase 3 — Each Trigger binding maps to a method on the WASM exports.
    fn generate_txn_declarations(&self) -> String {
        let mut seen = std::collections::HashSet::new();
        let mut out = String::new();

        for binding in &self.bindings {
            use crate::view_compiler::Directive;
            if let Directive::Trigger { txn, .. } = &binding.directive {
                if seen.insert(txn.clone()) {
                    out.push_str(&format!("  {}(...args: any[]): any;\n", txn));
                }
            }
        }

        if out.is_empty() {
            out.push_str("  // No transaction bindings declared\n");
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_empty_generator() -> GlueWebGenerator {
        GlueWebGenerator::new(
            Vec::new(),
            Vec::new(),
            StateLayout {
                app_name: "test_app".to_string(),
                generation_offset: 0,
                flush_buffer_offset: 64,
                max_flush_entries: 16,
                fields: vec![
                    FieldLayout {
                        field_handle: 1,
                        name: "count".to_string(),
                        offset: 4,
                        size: 4,
                        element_size: 4,
                        type_tag: TypeTag::Int,
                    },
                    FieldLayout {
                        field_handle: 2,
                        name: "speed".to_string(),
                        offset: 8,
                        size: 4,
                        element_size: 4,
                        type_tag: TypeTag::Float,
                    },
                    FieldLayout {
                        field_handle: 3,
                        name: "ready".to_string(),
                        offset: 12,
                        size: 1,
                        element_size: 1,
                        type_tag: TypeTag::Bool,
                    },
                ],
            },
            HashMap::new(),
            Vec::new(),
        )
    }

    #[test]
    fn test_generate_produces_output() {
        let g = make_empty_generator();
        let output = g.generate().expect("generate should succeed");
        assert!(output.dom_shim.contains("WasmDomRuntime"),
            "dom-shim should contain WasmDomRuntime class");
        assert!(output.dts.contains("createApp"),
            "dts should contain createApp function");
    }

    #[test]
    fn test_generate_includes_generation_offset() {
        let g = make_empty_generator();
        let output = g.generate().unwrap();
        assert!(output.dom_shim.contains("_generationOffset"),
            "should include generation offset");
        assert!(output.dom_shim.contains("generation"),
            "should include generation accessor");
    }

    #[test]
    fn test_generate_with_bindings() {
        let bindings = vec![
            crate::view_compiler::Binding {
                element_id: "counter".to_string(),
                directive: crate::view_compiler::Directive::Text {
                    signal: "count".to_string(),
                },
            },
            crate::view_compiler::Binding {
                element_id: "inc-btn".to_string(),
                directive: crate::view_compiler::Directive::Trigger {
                    event: "click".to_string(),
                    txn: "increment".to_string(),
                    params: vec![],
                },
            },
        ];
        let g = GlueWebGenerator::new(
            Vec::new(),
            bindings,
            StateLayout {
                app_name: "counter_app".to_string(),
                generation_offset: 0,
                flush_buffer_offset: 64,
                max_flush_entries: 8,
                fields: vec![
                    FieldLayout {
                        field_handle: 1,
                        name: "count".to_string(),
                        offset: 4,
                        size: 4,
                        element_size: 4,
                        type_tag: TypeTag::Int,
                    },
                ],
            },
            HashMap::new(),
            Vec::new(),
        );
        let output = g.generate().expect("generate should succeed");
        assert!(output.dom_shim.contains("counter_app"),
            "dom-shim should include app name");
        assert!(output.dts.contains("increment"),
            "dts should include transaction method declarations");
        assert!(output.dom_shim.contains("_applyFlush"),
            "dom-shim should include flush handler");
        // 2026-08-10: bindings must wire real DOM ops, not placeholders.
        // 2026-08-11 (Phase 2a): bindings register per-field view effects
        // (fanned out by the handle's default applyFn) instead of replacing
        // `_bindingTable[1].applyFn` — the old replacement clobbered earlier
        // bindings when two elements bound the same field.
        assert!(output.dom_shim.contains("_registerViewEffect(1,"),
            "text binding must register a view effect on the count field; got:\n{}", output.dom_shim);
        assert!(output.dom_shim.contains("document.getElementById(\"counter\")"),
            "text binding must target its element; got:\n{}", output.dom_shim);
        assert!(output.dom_shim.contains("el.textContent = value"),
            "text binding must set textContent; got:\n{}", output.dom_shim);
        assert!(output.dom_shim.contains("addEventListener(\"click\"")
            && output.dom_shim.contains("this._txn(\"increment\")"),
            "trigger binding must wire the event listener; got:\n{}", output.dom_shim);
    }

    #[test]
    fn test_multiple_bindings_same_field_do_not_clobber() {
        // 2026-08-11 (Phase 2a): two elements reading the same field must BOTH
        // react — the per-handle effect list fans the flush value out instead
        // of the last-emitted override winning.
        let bindings = vec![
            crate::view_compiler::Binding {
                element_id: "a".to_string(),
                directive: crate::view_compiler::Directive::Text {
                    signal: "count".to_string(),
                },
            },
            crate::view_compiler::Binding {
                element_id: "b".to_string(),
                directive: crate::view_compiler::Directive::Text {
                    signal: "count".to_string(),
                },
            },
        ];
        let g = GlueWebGenerator::new(
            Vec::new(),
            bindings,
            StateLayout {
                app_name: "multi".to_string(),
                generation_offset: 0,
                flush_buffer_offset: 64,
                max_flush_entries: 8,
                fields: vec![FieldLayout {
                    field_handle: 1,
                    name: "count".to_string(),
                    offset: 4,
                    size: 4,
                    element_size: 4,
                    type_tag: TypeTag::Int,
                }],
            },
            HashMap::new(),
            Vec::new(),
        );
        let output = g.generate().expect("generate should succeed");
        let shim = &output.dom_shim;
        let a_regs = shim.matches("_registerViewEffect(1,").count();
        assert_eq!(a_regs, 2, "both elements must register on handle 1:\n{shim}");
        assert!(shim.contains("document.getElementById(\"a\")")
            && shim.contains("document.getElementById(\"b\")"),
            "both elements targeted:\n{shim}");
    }

    #[test]
    fn test_when_binding_emits_mount_unmount_effect() {        // 2026-08-11 (Phase 2a, SPEC 21.4): `b-when` structurally mounts/
        // unmounts a subtree. The emitted effect must snapshot a template,
        // anchor the position with a comment, and insert/remove on truthiness.
        let bindings = vec![crate::view_compiler::Binding {
            element_id: "panel".to_string(),
            directive: crate::view_compiler::Directive::When {
                expr: "count > 0".to_string(),
            },
        }];
        let g = GlueWebGenerator::new(
            Vec::new(),
            bindings,
            StateLayout {
                app_name: "when_app".to_string(),
                generation_offset: 0,
                flush_buffer_offset: 64,
                max_flush_entries: 8,
                fields: vec![FieldLayout {
                    field_handle: 1,
                    name: "count".to_string(),
                    offset: 4,
                    size: 4,
                    element_size: 4,
                    type_tag: TypeTag::Int,
                }],
            },
            HashMap::new(),
            Vec::new(),
        );
        let output = g.generate().expect("generate should succeed");
        let shim = &output.dom_shim;
        assert!(shim.contains("_registerViewEffect(1,"),
            "when binding must register on the count field:\n{shim}");
        assert!(shim.contains("document.getElementById(\"panel\")"),
            "when binding must target its element:\n{shim}");
        assert!(shim.contains("template.cloneNode(true)"),
            "remount must clone the template:\n{shim}");
        assert!(shim.contains("createComment('b-when')"),
            "unmount must anchor the position:\n{shim}");
        assert!(shim.contains("el.remove()"),
            "unmount must detach the element:\n{shim}");
    }

    #[test]
    fn test_bind_binding_emits_type_driven_input_wiring() {
        // 2026-08-11 (Phase 2a2, SPEC 21.4): b-bind:value routes the input
        // event to the resolved writer transaction, marshalling the value by
        // the transaction's parameter type (String → _writeString, Number →
        // Number(...), Bool → checkbox).
        let mk = |target: &str, kind: ParamKind| {
            let bindings = vec![crate::view_compiler::Binding {
                element_id: "fld".to_string(),
                directive: crate::view_compiler::Directive::Bind {
                    target: target.to_string(),
                },
            }];
            let mut routes = std::collections::HashMap::new();
            routes.insert(
                target.to_string(),
                BindRoute {
                    txn: "set_field".to_string(),
                    param_kind: kind,
                },
            );
            let g = GlueWebGenerator::new(
                Vec::new(),
                bindings,
                StateLayout {
                    app_name: "bind_app".to_string(),
                    generation_offset: 0,
                    flush_buffer_offset: 64,
                    max_flush_entries: 8,
                    fields: vec![],
                },
                HashMap::new(),
                Vec::new(),
            )
            .with_bind_routes(routes);
            g.generate().expect("generate should succeed").dom_shim
        };
        let shim_str = mk("name", ParamKind::String);
        assert!(
            shim_str.contains("_writeString(el.value)") && shim_str.contains("set_field"),
            "String bind must write via _writeString:\n{shim_str}"
        );
        let shim_num = mk("count", ParamKind::Number);
        assert!(
            shim_num.contains("Number(el.value)"),
            "Int/Float bind must marshal Number(...):\n{shim_num}"
        );
        let shim_bool = mk("flag", ParamKind::Bool);
        assert!(
            shim_bool.contains("el.checked"),
            "Bool bind must read checkbox:\n{shim_bool}"
        );
    }

    #[test]
    fn test_empty_bindings() {
        let g = make_empty_generator();
        let output = g.generate().unwrap();
        assert!(output.dom_shim.contains("createApp"),
            "should still produce createApp with empty bindings");
    }

    #[test]
    fn test_class_style_attr_emission() {
        // 2026-08-11 (housekeeping, Part 2): global (non-each) b-class/
        // b-style/b-attr were DEAD (fell to `_ => ""`). Now they register on
        // the root field's handle (dynamic) or apply once at init (literal).
        use crate::view_compiler::{Binding, Directive};
        let field = || FieldLayout {
            field_handle: 1,
            name: "dark_mode".to_string(),
            offset: 4,
            size: 1,
            element_size: 1,
            type_tag: TypeTag::Bool,
        };
        let bindings = vec![
            Binding {
                element_id: "d1".to_string(),
                directive: Directive::Class {
                    pairs: vec![("active".to_string(), "dark_mode == true".to_string())],
                },
            },
            Binding {
                element_id: "d2".to_string(),
                directive: Directive::Style {
                    name: "background".to_string(),
                    value: "dark_mode".to_string(),
                },
            },
            Binding {
                element_id: "d3".to_string(),
                directive: Directive::Attr {
                    name: "data-mode".to_string(),
                    value: "'dark'".to_string(),
                },
            },
        ];
        let g = GlueWebGenerator::new(
            Vec::new(),
            bindings,
            StateLayout {
                app_name: "csd".to_string(),
                generation_offset: 0,
                flush_buffer_offset: 64,
                max_flush_entries: 8,
                fields: vec![field()],
            },
            HashMap::new(),
            Vec::new(),
        );
        let output = g.generate().expect("generate should succeed");
        let shim = &output.dom_shim;
        assert!(shim.contains("el.classList.toggle(\"active\", value == true)"),
            "class comparison registers on the field:\n{shim}");
        assert!(shim.contains("el.style[\"background\"] = value"),
            "style field ref registers on the field:\n{shim}");
        assert!(shim.contains("el.setAttribute(\"data-mode\", \"dark\")"),
            "literal attr applies once at init:\n{shim}");
    }

    #[test]
    fn test_each_binding_emits_vector_renderer() {
        // 2026-08-11 (Phase 2a3): b-each over a vector state field renders one
        // template clone per slot, reconciled by key. The iterable field is an
        // [N x i64] vector — count derives from the layout size (16 bytes → 2
        // items).
        use crate::view_compiler::{Binding, Directive, ItemBinding, ItemDirective};
        let bindings = vec![Binding {
            element_id: "lst".to_string(),
            directive: Directive::Each {
                iterable: "items".to_string(),
                item_name: "item".to_string(),
                template_html: r#"<span data-itm="1">x</span>"#.to_string(),
                container_id: "lst".to_string(),
                item_bindings: vec![
                    ItemBinding {
                        marker: 0,
                        directive: ItemDirective::Trigger {
                            event: "click".to_string(),
                            txn: "select_item".to_string(),
                        },
                    },
                    ItemBinding {
                        marker: 1,
                        directive: ItemDirective::Text {
                            signal: "item".to_string(),
                        },
                    },
                ],
                key_expr: "item".to_string(),
            },
        }];
        let g = GlueWebGenerator::new(
            Vec::new(),
            bindings,
            StateLayout {
                app_name: "each_app".to_string(),
                generation_offset: 0,
                flush_buffer_offset: 64,
                max_flush_entries: 8,
                fields: vec![FieldLayout {
                    field_handle: 1,
                    name: "items".to_string(),
                    offset: 4,
                    size: 16,
                    element_size: 8,
                    type_tag: TypeTag::Int,
                }],
            },
            HashMap::new(),
            Vec::new(),
        );
        let output = g.generate().expect("generate should succeed");
        let shim = &output.dom_shim;
        assert!(shim.contains("_registerViewEffect(1,"),
            "each must register on the items field:\n{shim}");
        assert!(shim.contains("document.createElement(tagName)")
            && shim.contains("el.innerHTML = templateInner"),
            "each must build a fresh clone per item:\n{shim}");
        assert!(shim.contains("getBigInt64(valPtr + i * 8"),
            "each must read i64 slots from WASM:\n{shim}");
        assert!(shim.contains("txn: \"select_item\"")
            && shim.contains("this._txn(ib.txn)(item)"),
            "item trigger must call the txn with the item:\n{shim}");
        assert!(shim.contains("data-itm=\\\""),
            "marker-1 inner elements resolved in the clone:\n{shim}");
    }

    #[test]
    fn test_type_aware_flush_decode() {
        // 2026-08-10: _makeBinding must store a type-aware decode — Int reads
        // the slot as i32, Float as f64, Bool as i8, String dereferences the
        // stored pointer. The old _applyFlush TextDecoded every value.
        let g = make_empty_generator();
        let output = g.generate().unwrap();
        let shim = &output.dom_shim;
        assert!(shim.contains("typeTag === 1") && shim.contains("getFloat64(valPtr, true)"),
            "Float decode must be emitted; got:\n{shim}");
        assert!(shim.contains("typeTag === 2") && shim.contains("getUint8(valPtr, true)"),
            "Bool decode must be emitted; got:\n{shim}");
        assert!(shim.contains("typeTag === 3") && shim.contains("_readString(strPtr)"),
            "String decode must dereference the slot pointer; got:\n{shim}");
        assert!(shim.contains("getInt32(valPtr, true)"),
            "Int decode must be emitted; got:\n{shim}");
        assert!(shim.contains("binding.decode ? binding.decode(valPtr, valLen) : null"),
            "_applyFlush must use the type-aware decoder; got:\n{shim}");
        assert!(!shim.contains("valLen > 0\n          ? new TextDecoder()"),
            "blind TextDecoder path must be gone; got:\n{shim}");
    }

    #[test]
    fn test_state_layout_roundtrip() {
        let g = make_empty_generator();
        let output = g.generate().unwrap();
        // Verify the flush buffer config appears in the output
        assert!(output.dom_shim.contains("_maxFlushEntries"),
            "should include max flush entries");
        assert!(output.dom_shim.contains("_flushBufferOffset"),
            "should include flush buffer offset");
    }

    // ── Phase 6: frgn from #Web import stubs (type-driven) ─────

    /// Helper: create a frgn from #Web with given param and return types.
    /// 2026-07-26: Phase 6 — Type-driven testing: no name matching.
    fn make_web_frgn(
        name: &str,
        inputs: Vec<(String, crate::ast::Type)>,
        outputs: Vec<(String, crate::ast::Type)>,
    ) -> crate::ast::top::ForeignBinding {
        let mut fb = crate::ast::ForeignBinding::new(
            name.to_string(),
            None,
            crate::ast::FromSpec::Protocol("#Web".to_string()),
            crate::ast::ForeignTarget::Native,
        );
        fb.inputs = inputs;
        fb.success_output = outputs;
        fb
    }

    #[test]
    fn test_frgn_string_param_uses_read_string() {
        // frgn foo(msg: String) from #Web
        let frgn = make_web_frgn(
            "foo",
            vec![("msg".to_string(), crate::ast::Type::string())],
            vec![],
        );
        let g = GlueWebGenerator::new(
            Vec::new(), Vec::new(),
            StateLayout { app_name: "t".into(), generation_offset: 0, flush_buffer_offset: 0, max_flush_entries: 0, fields: vec![] },
            HashMap::new(),
            vec![frgn],
        );
        let out = g.generate().unwrap();
        assert!(out.dom_shim.contains("_readString"),
            "String param should generate _readString call");
        assert!(!out.dom_shim.contains("document.createElement"),
            "no DOM-specific names should appear in the compiler");
    }

    #[test]
    fn test_frgn_element_creates_handle() {
        // frgn make_widget() -> Element from #Web
        let frgn = make_web_frgn(
            "make_widget",
            vec![],
            vec![("result".to_string(), crate::ast::Type::Custom("Element".to_string()))],
        );
        let g = GlueWebGenerator::new(
            Vec::new(), Vec::new(),
            StateLayout { app_name: "t".into(), generation_offset: 0, flush_buffer_offset: 0, max_flush_entries: 0, fields: vec![] },
            HashMap::new(),
            vec![frgn],
        );
        let out = g.generate().unwrap();
        assert!(out.dom_shim.contains("_handles.push"),
            "Element return should register in handle table");
    }

    #[test]
    fn test_frgn_bool_returns_01() {
        // frgn is_ready() -> Bool from #Web
        let frgn = make_web_frgn(
            "is_ready",
            vec![],
            vec![("result".to_string(), crate::ast::Type::bool_())],
        );
        let g = GlueWebGenerator::new(
            Vec::new(), Vec::new(),
            StateLayout { app_name: "t".into(), generation_offset: 0, flush_buffer_offset: 0, max_flush_entries: 0, fields: vec![] },
            HashMap::new(),
            vec![frgn],
        );
        let out = g.generate().unwrap();
        assert!(out.dom_shim.contains("? 1 : 0"),
            "Bool return should marshal to 1/0");
    }

    #[test]
    fn test_frgn_int_float_raw_params() {
        // frgn compute(a: Int, b: Float) from #Web
        let frgn = make_web_frgn(
            "compute",
            vec![
                ("a".to_string(), crate::ast::Type::int()),
                ("b".to_string(), crate::ast::Type::float()),
            ],
            vec![],
        );
        let g = GlueWebGenerator::new(
            Vec::new(), Vec::new(),
            StateLayout { app_name: "t".into(), generation_offset: 0, flush_buffer_offset: 0, max_flush_entries: 0, fields: vec![] },
            HashMap::new(),
            vec![frgn],
        );
        let out = g.generate().unwrap();
        // Find the compute() stub — it starts with "compute: (" (inside _buildImports return)
        let stub_start = out.dom_shim.find("compute(").unwrap_or(0);
        let stub_end = out.dom_shim[stub_start..].find("},")
            .map(|e| stub_start + e + 2)
            .unwrap_or(out.dom_shim.len());
        let stub_body = &out.dom_shim[stub_start..stub_end];
        assert!(!stub_body.contains("_readString"),
            "Int/Float params should NOT generate _readString in the frgn stub body");
        assert!(out.dom_shim.contains("compute"),
            "should include the frgn name");
    }

    #[test]
    fn test_frgn_without_frgn_skips_imports() {
        let g = GlueWebGenerator::new(
            Vec::new(), Vec::new(),
            StateLayout { app_name: "t".into(), generation_offset: 0, flush_buffer_offset: 0, max_flush_entries: 0, fields: vec![] },
            HashMap::new(),
            vec![],
        );
        let out = g.generate().unwrap();
        // No frgns — import section should still be valid (empty)
        assert!(out.dom_shim.contains("_buildImports"),
            "should still create _buildImports method");
        assert!(out.dom_shim.contains("__web_flush_state"),
            "should still include __web_flush_state");
    }

    #[test]
    fn test_frgn_string_return_uses_write_string() {
        // frgn get_data() -> String from #Web
        let frgn = make_web_frgn(
            "get_data",
            vec![],
            vec![("result".to_string(), crate::ast::Type::string())],
        );
        let g = GlueWebGenerator::new(
            Vec::new(), Vec::new(),
            StateLayout { app_name: "t".into(), generation_offset: 0, flush_buffer_offset: 0, max_flush_entries: 0, fields: vec![] },
            HashMap::new(),
            vec![frgn],
        );
        let out = g.generate().unwrap();
        assert!(out.dom_shim.contains("_writeString"),
            "String return should generate _writeString call");
    }

    #[test]
    fn test_frgn_mixed_params() {
        // frgn update(elem: Element, msg: String, count: Int) from #Web
        let frgn = make_web_frgn(
            "update",
            vec![
                ("elem".to_string(), crate::ast::Type::Custom("Element".to_string())),
                ("msg".to_string(), crate::ast::Type::string()),
                ("count".to_string(), crate::ast::Type::int()),
            ],
            vec![],
        );
        let g = GlueWebGenerator::new(
            Vec::new(), Vec::new(),
            StateLayout { app_name: "t".into(), generation_offset: 0, flush_buffer_offset: 0, max_flush_entries: 0, fields: vec![] },
            HashMap::new(),
            vec![frgn],
        );
        let out = g.generate().unwrap();
        assert!(out.dom_shim.contains("_handles["),
            "Element param should use handle lookup");
        assert!(out.dom_shim.contains("_readString"),
            "String param should use _readString");
        // count: Int passes through as raw value — no _readString in the stub body
        let stub_start = out.dom_shim.find("update(").unwrap_or(0);
        let stub_end = out.dom_shim[stub_start..].find("},")
            .map(|e| stub_start + e + 2)
            .unwrap_or(out.dom_shim.len());
        let stub_body = &out.dom_shim[stub_start..stub_end];
        let readstring_count = stub_body.matches("_readString").count();
        assert_eq!(readstring_count, 1,
            "only one _readString in the update() stub (for String param, not Int); got {}", readstring_count);
    }
}
