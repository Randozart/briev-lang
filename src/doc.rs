// ── Documentation Generator ─────────────────────────────────────────────
// 2026-07-24: Renders /// doc comments from .bv files to HTML.
// Reads a parsed program and generates a single HTML page with
// sections for each documented definition.

use std::path::Path;

use crate::ast::*;
use crate::parser::Parser;

/// 2026-07-24: Generate documentation for a Briev source file.
/// Reads the file, parses it, extracts doc comments, produces HTML.
pub fn generate_doc(input_path: &str) -> Result<(), String> {
    let source = std::fs::read_to_string(input_path)
        .map_err(|e| format!("cannot read '{}': {}", input_path, e))?;

    let tokens = crate::lexer::tokenize(&source)
        .map_err(|e| format!("lex error: {}", e))?;
    let mut parser = Parser::new(tokens, &source);
    let program = parser.parse_program()
        .map_err(|e| format!("parse error: {}", e))?;

    let html = render_html(&program, input_path);

    let output_path = Path::new(input_path).with_extension("html");
    std::fs::write(&output_path, &html)
        .map_err(|e| format!("cannot write '{}': {}", output_path.display(), e))?;

    eprintln!("wrote {}", output_path.display());
    Ok(())
}

/// 2026-07-24: Render the entire program as an HTML document.
fn render_html(program: &[TopLevel], source_path: &str) -> String {
    let source_name = Path::new(source_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("program");

    let mut items = String::new();
    for tl in program {
        if let Some(entry) = render_item(tl) {
            items.push_str(&entry);
        }
    }

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>{title} — Briev Documentation</title>
<style>
  body {{ font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; max-width: 960px; margin: 0 auto; padding: 20px; background: #fafafa; color: #333; }}
  h1 {{ border-bottom: 2px solid #4a90d9; padding-bottom: 8px; }}
  h2 {{ color: #2c5282; margin-top: 24px; margin-bottom: 4px; }}
  h3 {{ color: #2d3748; margin: 16px 0 4px 0; }}
  .doc {{ background: #fff; padding: 12px 16px; border-radius: 6px; border-left: 4px solid #4a90d9; margin: 8px 0 16px 0; box-shadow: 0 1px 3px rgba(0,0,0,0.08); }}
  .doc p {{ margin: 4px 0; }}
  .meta {{ color: #718096; font-size: 0.9em; }}
  .section {{ margin: 8px 0; }}
  code {{ background: #edf2f7; padding: 1px 4px; border-radius: 3px; font-size: 0.9em; }}
  pre {{ background: #2d3748; color: #e2e8f0; padding: 12px; border-radius: 6px; overflow-x: auto; }}
  .empty {{ color: #a0aec0; font-style: italic; }}
</style>
</head>
<body>
<h1>{title} <span class="meta">— Briev Module</span></h1>
<p class="meta">Source: {source}</p>
{items}
</body>
</html>"#,
        title = source_name,
        source = source_path,
        items = items
    )
}

/// 2026-07-24: Render a single TopLevel item as an HTML section, if it has a doc comment.
fn render_item(tl: &TopLevel) -> Option<String> {
    match tl {
        TopLevel::Definition(d) => render_defn("defn", &d.name, &d.doc, &d.parameters, None, &d.body),
        TopLevel::Transaction(t) => render_transaction(t),
        TopLevel::Cell(c) => render_cell(c),
        TopLevel::ForeignBinding(f) => render_frgn(f),
        TopLevel::StaticStruct(s) => render_struct(s),
        TopLevel::CompileTimeDefn(d) => render_defn("$defn", &d.name, &d.doc, &d.parameters, None, &d.body),
        _ => None,
    }
}

/// 2026-07-24: Render a transaction or node.
fn render_transaction(t: &Transaction) -> Option<String> {
    let kind = if t.is_reactive { "node" } else { "txn" };
    let params = format_params(&t.parameters);
    let sig = format!("{}{}", kind, params);
    render_section(&t.name, &sig, &t.doc, &t.body)
}

/// 2026-07-24: Render a cell (state holder).
fn render_cell(c: &CellDef) -> Option<String> {
    if c.doc.is_none() { return None; }
    let params = format_params(&c.parameters);
    Some(format!(
        r#"<div class="section">
<h2>cell {name}</h2>
<div class="meta"><code>cell {name}({params})</code></div>
<div class="doc"><p>{doc}</p></div>
<p class="meta">{fields} field(s), {txns} transaction(s)</p>
</div>
"#,
        name = html_escape(&c.name),
        params = html_escape(&params),
        doc = html_escape(c.doc.as_deref().unwrap_or("")),
        fields = c.fields.len(),
        txns = c.transactions.len(),
    ))
}

/// 2026-07-24: Render a foreign function declaration.
fn render_frgn(f: &ForeignBinding) -> Option<String> {
    if f.doc.is_none() { return None; }
    let params = f.inputs.iter()
        .map(|(n, t)| format!("{}: {}", n, format_type(t)))
        .collect::<Vec<_>>()
        .join(", ");
    let sig = format!("frgn {}({})", f.foreign_name, params);
    render_section(&f.foreign_name, &sig, &f.doc, &[])
}

/// 2026-07-24: Render a struct definition.
fn render_struct(s: &StructDef) -> Option<String> {
    // 2026-07-24: StructDef has no doc field. Only render if metadata has a "doc" key.
    if !s.metadata.contains_key("doc") { return None; }
    let doc_str = match s.metadata.get("doc") {
        Some(PropertyValue::String(d)) => d.clone(),
        _ => return None,
    };
    let fields: String = s.fields.iter()
        .map(|(name, ty)| format!("  {}: {};", name, format_type(ty)))
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!(
        r#"<div class="section">
<h2>struct {name}</h2>
<div class="doc"><p>{doc}</p></div>
<pre>{fields}</pre>
</div>
"#,
        name = html_escape(&s.name),
        doc = html_escape(&doc_str),
        fields = html_escape(&fields),
    ))
}

/// 2026-07-24: Render a definition with its signature, doc comment, and body preview.
fn render_defn(
    kind: &str,
    name: &str,
    doc: &Option<String>,
    params: &[(String, Type)],
    _outputs: Option<&[Type]>,
    body: &[Statement],
) -> Option<String> {
    if doc.is_none() { return None; }
    let params_str = format_params(params);
    let sig = format!("{}{}", kind, params_str);
    render_section(name, &sig, doc, body)
}

/// 2026-07-24: Render a generic section with name, signature, doc comment, and body preview.
fn render_section(name: &str, sig: &str, doc: &Option<String>, body: &[Statement]) -> Option<String> {
    let doc_text = doc.as_deref()?;
    let body_lines: Vec<String> = body.iter()
        .take(5)
        .map(|s| format!("  {};", display_stmt(s)))
        .collect();
    let body_pre = if body_lines.is_empty() {
        String::new()
    } else {
        let mut joined = body_lines.join("\n");
        if body.len() > 5 {
            joined.push_str("\n  ...");
        }
        format!("\n<pre>{}</pre>", html_escape(&joined))
    };

    Some(format!(
        r#"<div class="section">
<h2>{name}</h2>
<div class="meta"><code>{sig}</code></div>
<div class="doc"><p>{doc}</p></div>{body}
</div>
"#,
        name = html_escape(name),
        sig = html_escape(sig),
        doc = html_escape(doc_text),
        body = body_pre,
    ))
}

/// 2026-07-24: Format a parameter list.
fn format_params(params: &[(String, Type)]) -> String {
    if params.is_empty() {
        String::new()
    } else {
        format!("({})", params.iter()
            .map(|(n, t)| format!("{}: {}", n, format_type(t)))
            .collect::<Vec<_>>()
            .join(", "))
    }
}

/// 2026-07-24: Format a type for display.
fn format_type(t: &Type) -> String {
    match t {
        Type::Bits(n) => format!("Bits({})", n),
        Type::Void => "Void".into(),
        Type::Custom(name) => name.clone(),
        Type::Generic(name, args) => format_generic(name, args),
        Type::Applied(name, args) => format_type_applied(name, args),
        Type::Union(ts) => format!("({})", ts.iter().map(format_type).collect::<Vec<_>>().join(" | ")),
        Type::Tuple(ts) => format!("({})", ts.iter().map(format_type).collect::<Vec<_>>().join(", ")),
        Type::TypeVar(v) => format!("'{}", v),
        Type::Ptr(inner) => format!("Ptr<{}>", format_type(inner)),
        Type::PtrConst(inner) => format!("Ptr&lt;const {}&gt;", format_type(inner)),
        Type::Function(args, ret) => format!("({}) -> {}", args.iter().map(format_type).collect::<Vec<_>>().join(", "), format_type(ret)),
        Type::Width(n) => format!("Width({})", n),
        _ => format!("{:?}", t),
    }
}

/// 2026-07-24: Format a generic type with type parameters.
fn format_generic(name: &str, args: &[Type]) -> String {
    if args.is_empty() {
        name.to_string()
    } else {
        format!("{}<{}>", name, args.iter().map(|a| format_type(a)).collect::<Vec<_>>().join(", "))
    }
}

/// 2026-07-24: Format an applied (concrete) type.
fn format_type_applied(name: &str, args: &[Type]) -> String {
    format_generic(name, args)
}

/// 2026-07-24: Display a single statement as a one-liner.
fn display_stmt(s: &Statement) -> String {
    match s {
        Statement::Let { name, ty, .. } => {
            if let Some(t) = ty {
                format!("let {}: {}", name, format_type(t))
            } else {
                format!("let {}", name)
            }
        }
        Statement::Expression(e) => format!("{:?}", e),
        Statement::Term(_) => "term ...".into(),
        Statement::Guarded(_, _) => "when ... { ... }".into(),
        _ => format!("{:?}", s).chars().take(60).collect(),
    }
}

/// 2026-07-24: HTML-escape a string.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
