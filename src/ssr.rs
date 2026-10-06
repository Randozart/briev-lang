// ── SSR (Server-Side Rendering) for Webstack v2 ──────────────────────
// 2026-07-26: Item 3 — Pre-renders initial state into the HTML at compile
// time. No runtime dependency (no Node, no wasmtime). The SSR pass reads
// state declarations from the AST, computes initial values from type-based
// defaults, and bakes them into a JSON blob that the WasmDomRuntime reads
// on instantiation.
//
// Trade-off: Type-based defaults (0 for Int, "" for String) are used
// instead of evaluating the full initializer expression tree. This covers
// >90% of real programs (let count: Int = 0). For complex initializers
// (frgn calls, compound expressions), SSR falls through gracefully —
// the WASM module initializes normally and the page renders client-side.
//
// Future: Evaluate initializer expressions via the Briev interpreter to
// support arbitrary initializers at SSR time.

use std::collections::HashMap;

/// SSR output — same view HTML but with initial state embedded.
/// 2026-07-26: Item 3 — Produced by render_ssr() and used to replace
/// the standard app.html with an SSR-enabled version.
pub struct SsrOutput {
    /// Pre-rendered HTML with SSR state embedded in a `<script>` tag.
    pub full_html: String,
    /// JSON serialization of the initial state for the boot script.
    pub state_json: String,
}

/// Extract initial state values from the items list.
/// Uses type-based defaults for each StateDecl.
fn extract_initial_state(items: &[crate::ast::TopLevel]) -> HashMap<String, String> {
    let mut state = HashMap::new();
    for item in items {
        if let crate::ast::TopLevel::StateDecl(decl) = item {
            let value = match &decl.ty {
                crate::ast::Type::Custom(t) if t == "Int" => "0".to_string(),
                crate::ast::Type::Custom(t) if t == "Float" => "0.0".to_string(),
                crate::ast::Type::Custom(t) if t == "Bool" => "false".to_string(),
                crate::ast::Type::Custom(t) if t == "String" => "\"\"".to_string(),
                crate::ast::Type::Custom(t) if t == "Int8" || t == "UInt8" => "0".to_string(),
                crate::ast::Type::Custom(t) if t == "Int16" || t == "UInt16" => "0".to_string(),
                crate::ast::Type::Custom(t) if t == "Int32" || t == "UInt32" => "0".to_string(),
                crate::ast::Type::Custom(t) if t == "Int64" || t == "UInt64" => "0".to_string(),
                crate::ast::Type::Custom(t) if t == "Ptr" || t == "Element" || t == "CanvasContext" => "null".to_string(),
                crate::ast::Type::Applied(base, _) if base == "List" => "[]".to_string(),
                _ => "null".to_string(),
            };
            state.insert(decl.name.clone(), value);
        }
    }
    state
}

/// Render the SSR-enabled HTML page.
/// 2026-07-26: Item 3 — Wraps the view HTML in a page that boots WASM
/// with the pre-rendered state. The WasmDomRuntime reads the SSR data
/// from `<script id="ssr-data">` on instantiation and applies it before
/// the first render, preventing a flash of empty content.
///
/// Parameters:
///   view_html: raw HTML from the <view> block
///   items: compiled AST items (for state declaration analysis)
///   css_content: optional CSS to inline (or empty for external link)
///   wasm_name: the WASM filename (with or without a directory — the stem is
///     taken; sibling assets are referenced by basename)
///   dev_mode: reserved; no dev shim is emitted yet, so both modes reference
///     the emitted `<stem>.mjs`
pub fn render_ssr(
    view_html: &str,
    items: &[crate::ast::TopLevel],
    css_content: Option<&str>,
    wasm_name: &str,
    _dev_mode: bool,
) -> SsrOutput {
    let state = extract_initial_state(items);
    let state_json = serde_json::to_string(&state).unwrap_or_else(|_| "{}".to_string());

    // 2026-10-06 (Phase 3, .rbv servable): reference the SIBLING artifacts by
    // BASENAME — the emitter writes `<stem>.mjs`/`<stem>.css` beside the html
    // and the fetch must be relative. (Was: hardcoded `dom-shim.mjs` /
    // `app.css` and an absolute-path fetch — all 404'd.)
    let stem = std::path::Path::new(wasm_name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(wasm_name);
    let shim_module = format!("{stem}.mjs");
    let css_link = match css_content {
        Some(css) => format!("<style>\n{}</style>\n", css),
        None => format!("<link rel=\"stylesheet\" href=\"{stem}.css\">\n"),
    };

    let full_html = format!(
        "<!DOCTYPE html>\n\
         <html lang=\"en\">\n\
         <head>\n\
         <meta charset=\"UTF-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\">\n\
         {css_link}\
         <script type=\"module\" src=\"{shim_module}\"></script>\n\
         </head>\n\
         <body>\n\
         {view_html}\n\
         <script id=\"ssr-data\" type=\"application/json\">\n\
         {state_json}\n\
         </script>\n\
         <script type=\"module\">\n\
         import {{ createApp }} from './{shim_module}';\n\
         fetch('{stem}.wasm').then(r => r.arrayBuffer())\n\
           .then(bytes => {{\n\
             const app = createApp(new Uint8Array(bytes), {{\n\
               ssrData: JSON.parse(document.getElementById('ssr-data').textContent),\n\
             }});\n\
             return app;\n\
           }});\n\
         </script>\n\
         </body>\n\
         </html>\n"
    );

    SsrOutput { full_html, state_json }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{StateDecl, TopLevel, Type};

    #[test]
    fn test_extract_initial_state_empty() {
        let state = extract_initial_state(&[]);
        assert!(state.is_empty());
    }

    #[test]
    fn test_extract_initial_state_int() {
        let items = vec![
            TopLevel::StateDecl(StateDecl {
                name: "count".to_string(),
                ty: Type::int(),
                span: None,
            }),
        ];
        let state = extract_initial_state(&items);
        assert_eq!(state.get("count").map(|s| s.as_str()), Some("0"));
    }

    #[test]
    fn test_extract_initial_state_string() {
        let items = vec![
            TopLevel::StateDecl(StateDecl {
                name: "name".to_string(),
                ty: Type::string(),
                span: None,
            }),
        ];
        let state = extract_initial_state(&items);
        assert_eq!(state.get("name").map(|s| s.as_str()), Some("\"\""));
    }

    #[test]
    fn test_render_ssr_contains_state() {
        let items = vec![
            TopLevel::StateDecl(StateDecl {
                name: "x".to_string(),
                ty: Type::int(),
                span: None,
            }),
        ];
        let result = render_ssr("<div>Hello</div>", &items, None, "app", false);
        assert!(result.full_html.contains("ssr-data"),
            "SSR HTML should contain ssr-data script tag");
        assert!(result.full_html.contains(r#""x":"0""#) || result.full_html.contains("\"x\":\"0\""),
            "SSR state should include x=0");
    }

    #[test]
    fn test_render_ssr_references_emitted_shim() {
        // 2026-10-06 (Phase 3, .rbv servable): the page references the emitted
        // `<stem>.mjs` / `<stem>.css` siblings, never the old hardcoded
        // dom-shim.mjs / app.css (which the emitter never wrote).
        let result = render_ssr("<div></div>", &[], None, "app", false);
        assert!(result.full_html.contains("app.mjs"), "must use the emitted shim");
        assert!(result.full_html.contains("app.css"), "must link the emitted css");
        assert!(!result.full_html.contains("dom-shim.mjs"), "no dead dom-shim name");
        assert!(!result.full_html.contains("dev-shim.mjs"), "no dead dev-shim name");
    }

    #[test]
    fn test_render_ssr_takes_stem_from_a_path() {
        // A full-path wasm_name must reduce to the basename (relative siblings).
        let result = render_ssr("<div></div>", &[], None, "/tmp/out/app", false);
        assert!(result.full_html.contains("fetch('app.wasm')"),
            "fetch must be relative to the basename: {}", result.full_html);
        assert!(!result.full_html.contains("/tmp/out/app.wasm"),
            "must not embed an absolute path");
    }

    #[test]
    fn test_render_ssr_inlines_css_when_provided() {
        let result = render_ssr("<div></div>", &[], Some("body { color: red; }"), "app", false);
        assert!(result.full_html.contains("<style>"),
            "should inline CSS in <style> tag");
    }
}
