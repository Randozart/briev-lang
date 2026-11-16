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

use thiserror::Error;

#[derive(Error, Debug)]
pub enum RbvError {
    #[error("Missing <view> block")]
    MissingView,
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct RbvFile {
    pub briev_source: String,
    pub view_html: String,
    pub style_css: Option<String>,
}

impl RbvFile {
    /// Parse an `.rbv` file.
    ///
    /// Briev code is the default content — everything outside `<view>` and
    /// `<style>` tags is treated as Briev source.
    ///
    /// 2026-08-09 (Phase 14, SPEC 21.1): legacy `<script>` wrappers are
    /// INVALID — the script-wrapper compatibility was removed. A `<script>`
    /// tag anywhere in the document is a hard error, not a fallback.
    pub fn parse(source: &str) -> Result<Self, RbvError> {
        // 2026-08-11: only reject a `<script>` that starts a line (the legacy
        // wrapper shape). A line comment or string containing the literal text
        // `<script>` is not markup — `examples/todo.rbv` documents the removal
        // in a `//` comment and must still compile.
        let has_script_tag = source.lines().any(|line| {
            let l = line.trim_start();
            l.starts_with("<script") || l.starts_with("</script")
        });
        if has_script_tag {
            return Err(RbvError::Parse(
                "<script> wrappers are invalid (SPEC 21.1) — write Briev source \
                 directly; the `<view>`/`<style>` blocks carry the markup"
                    .into(),
            ));
        }
        let view = extract_tag(source, "<view>", "</view>").ok_or(RbvError::MissingView)?;

        let style = extract_tag(source, "<style>", "</style>");

        let briev_source = strip_known_blocks(source).trim().to_string();

        Ok(RbvFile {
            briev_source,
            view_html: view.trim().to_string(),
            style_css: style.map(|s| s.trim().to_string()),
        })
    }
}

fn extract_tag(source: &str, start_tag: &str, end_tag: &str) -> Option<String> {
    let start = source.find(start_tag)? + start_tag.len();
    let end = source.find(end_tag)?;
    Some(source[start..end].to_string())
}

fn strip_known_blocks(source: &str) -> String {
    let known_blocks: [(&str, &str); 2] = [("<view>", "</view>"), ("<style>", "</style>")];
    let mut result = source.to_string();
    for &(start_tag, end_tag) in &known_blocks {
        while let (Some(start), Some(end)) = (result.find(start_tag), result.find(end_tag)) {
            let block_end = end + end_tag.len();
            result.drain(start..block_end);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_rbv_briev_as_default() {
        let source = r#"
let count: Int = 0;

txn increment [true][@count + 1 == count] {
    &count = count + 1;
    term;
};

<view>
<p b-text="count">0</p>
<button b-trigger:click="increment">+</button>
</view>
"#;
        let rbv = RbvFile::parse(source).unwrap();
        assert!(rbv.briev_source.contains("count"));
        assert!(rbv.briev_source.contains("increment"));
        assert!(rbv.view_html.contains("b-text"));
        assert!(rbv.style_css.is_none());
    }

    #[test]
    fn test_parse_rbv_no_script_style_is_extracted() {
        // Briev code interleaved with view — everything outside <view> is source
        let source = r#"
let x: Int = 42;

<view>
<span b-text="x">42</span>
</view>

txn double [true][@x * 2 == x] {
    &x = x * 2;
    term;
};
"#;
        let rbv = RbvFile::parse(source).unwrap();
        assert!(rbv.briev_source.contains("let x: Int = 42"));
        assert!(rbv.briev_source.contains("txn double"));
        assert!(rbv.view_html.contains("b-text"));
    }

    /// 2026-08-09 (Phase 14, SPEC 21.1): a `<script>` wrapper is a hard error.
    #[test]
    fn test_parse_rbv_script_wrapper_is_invalid() {
        let source = r#"
<script type="briev">
let count: Int = 0;
</script>

<view>
<p b-text="count">0</p>
</view>
"#;
        let err = RbvFile::parse(source).unwrap_err();
        assert!(
            format!("{}", err).contains("<script> wrappers are invalid"),
            "script wrapper must be rejected: {err}"
        );
    }

    /// 2026-08-11: a `//` comment containing the literal text `<script>` is
    /// NOT markup — `examples/todo.rbv` documents the legacy wrapper removal
    /// in a comment and must still compile.
    #[test]
    fn test_parse_rbv_script_in_comment_is_allowed() {
        let source = r#"// removed old <script> wrapper — plain Briev now
let count: Int = 0;
txn inc [count < 10][true] {
    count = count + 1;
    term;
};
<view>
<span b-text="count">0</span>
</view>
"#;
        let rbv = RbvFile::parse(source).expect("comment mentioning <script> must not be rejected");
        assert!(rbv.briev_source.contains("let count: Int = 0;"));
        assert!(rbv.view_html.contains("b-text"));
    }

    #[test]
    fn test_parse_rbv_missing_view_errors() {
        let err = RbvFile::parse("let x: Int = 0;").unwrap_err();
        assert!(format!("{}", err).contains("Missing <view>"));
    }
}
