// ── Type Parser ────────────────────────────────────────────────────────
// 2026-07-12: Phase 1.4 — Parse Briev type annotations.
// Flat code: each function is max 2 levels of nesting.

use super::helpers::Parser;
use crate::ast::{BitRange, Dimension, Type};
use crate::errors::SyntaxError;
use crate::lexer::Token;

impl<'a> Parser<'a> {
    /// Parse a type annotation.
    /// 2026-07-16: Bits-thesis — all type names are Token::Identifier.
    /// Dispatch on identifier string, not token variant.
    pub fn parse_type(&mut self) -> Result<Type, SyntaxError> {
        // 2026-08-22 (Phase 5, SPEC §8.6): `dyn TraitName` — a trait object.
        // `dyn` is contextual (no Token); recognized only in type-prefix
        // position, so identifiers named dyn elsewhere are untouched.
        if self.check_identifier("dyn") {
            if let Some(Token::Identifier(trait_name)) = self.tokens.get(self.pos + 1).map(|(t, _)| t) {
                let trait_name = trait_name.clone();
                self.pos += 2;
                return Ok(Type::Dyn(Box::new(Type::Custom(trait_name))));
            }
        }
        let base = match self.peek() {
            Some(Token::Identifier(name)) => {
                let name = name.clone();
                self.pos += 1;
                match name.as_str() {
                    // Type::int() etc. are frontend conveniences for Bits(N) + metadata.
                    "Int" => ("Int", Type::int()),
                    "UInt" => ("UInt", Type::Custom("UInt".into())),
                    "Float" | "Float32" | "F32" => ("Float", Type::float()),
                    "Float64" | "F64" | "Double" => ("Float64", Type::float64()),
                    "String" => ("String", Type::string()),
                    "Bool" => ("Bool", Type::bool_()),
                    "Void" => return Ok(Type::void()),
                    "Char" => ("Char", Type::char_()),
                    "Blob" => ("Blob", Type::blob()),
                    // 2026-08-03: callback/function-pointer type annotation:
                    //   fn(Int) -> Int  /  fn(Int)  (void return)
                    // Crosses an FFI boundary as an opaque function pointer.
                    "fn" => return self.parse_fn_type(),
                    _ if name.starts_with('#') => {
                        // 2026-07-20: Hashword category: #Int, #Float, #String, etc.
                        // Optional protocol variant: #String<UTF8>, #Float<IEEE754>
                        if self.eat(&Token::Lt) {
                            let variant = self.expect_identifier()?;
                            if !self.eat_type_close() {
                                return self.error_at_current("expected '>' or '>>' in hashword variant");
                            }
                            return Ok(Type::HashWordVariant(name, variant));
                        }
                        // 2026-07-20: Bare hashwords resolve to their default variant.
                        // UTF-8 is the universal default for all files.
                        let variant = match name.as_str() {
                            "#String" => "UTF8",
                            "#Float" => "IEEE754",
                            "#Char" => "unicode",
                            _ => "",
                        };
                        if !variant.is_empty() {
                            return Ok(Type::HashWordVariant(name, variant.to_string()));
                        }
                        return Ok(Type::HashWord(name));
                    }
                    _ => {
                        // Unknown name — delegate to parse_named_type_body
                        // (handles Ptr<T>, Foo<T>, .ext suffixes, Custom types)
                        let ty = self.parse_named_type_body(&name)?;
                        return Ok(ty);
                    }
                }
            }
            Some(Token::LParen) => return self.parse_tuple_type(),
            _ => return self.error_at_current("expected type"),
        };
        // 2026-08-25 (sized scalars, user decision): `Int<8>` / `UInt<8>` /
        // `Bool<1>` — angle brackets are compile-time type-level
        // specialization (delimiter contract, SPEC): here a bit-width
        // specialization of the primitive. Produces the Constrained type the
        // hardware lowering already maps to iN. Primitives take no generic
        // args, so `<` in type position here is unambiguous.
        let prim_name = base.0;
        if matches!(prim_name, "Int" | "UInt" | "Bool") && self.check(&Token::Lt) {
            if let Some(Token::Integer(w)) = self.peek_next() {
                let w = *w as usize;
                if w == 0 || w > 64 {
                    return self.error_at_current("width in Int<N>/UInt<N> must be 1..=64");
                }
                if prim_name == "Bool" && w != 1 {
                    return self.error_at_current("Bool<1> is the only valid Bool width");
                }
                self.pos += 2; // consume '<' and the width literal
                if !self.eat_type_close() {
                    return self.error_at_current("expected '>' after the width");
                }
                let base_ty = match prim_name {
                    "Int" => Type::int(),
                    "UInt" => Type::Custom("UInt".into()),
                    _ => Type::bool_(),
                };
                return Ok(Type::Constrained(Box::new(base_ty), BitRange::Single(w)));
            }
        }
        // 2026-09-11 (fundamentals doctrine, Phase A): `Float<Posit>` /
        // `String<C_String>` / `Char<ASCII>` — the fundamental IS the
        // protocol, so the variant rides on the type in angle brackets
        // (replaces `#Float<Posit>`). Produces Applied(fundamental, [variant])
        // which the casting graph peels to (category, variant).
        if matches!(prim_name, "Float" | "String" | "Char") && self.check(&Token::Lt) {
            if let Some(Token::Identifier(variant)) = self.peek_next() {
                let variant = variant.clone();
                self.pos += 2; // consume '<' and the variant identifier
                if !self.eat_type_close() {
                    return self.error_at_current("expected '>' after the protocol variant");
                }
                return Ok(Type::Applied(prim_name.to_string(), vec![Type::Custom(variant)]));
            }
        }
        // 2026-08-05 (Phase 3): free-form dot-extension type suffixes
        // (`String.c`, `Int.c.sso`) are removed; host/target qualifiers live
        // in configured GLUE bindings and protocol variants (SPEC §8.7).
        // 2026-07-25: Array syntax: Int[1024] → Type::Vector.
        // 2026-08-07: MULTI-dim — `T[M][N]` accumulates dimensions into one
        // `Type::Vector(inner, [M, N])` (the `Matrix<T, Rows, Cols>` enabler,
        // SPEC §16.6).
        // 2026-07-31: `[` is an array suffix ONLY when the next token is an
        // integer literal (`Int[8]`) or an identifier directly followed by
        // `]` (`T[N]` generic array). A contract bracket (`-> Int [b != 0]`)
        // is left for parse_contract.
        let mut dims: Vec<Dimension> = Vec::new();
        while self.check(&Token::LBracket) {
            if matches!(self.peek_next(), Some(Token::Integer(_))) {
                self.pos += 1; // consume LBracket
                let size = match self.peek() {
                    Some(&Token::Integer(n)) => { self.pos += 1; n as usize }
                    _ => { return self.error_at_current("expected array size (integer)"); }
                };
                self.expect(Token::RBracket)?;
                dims.push(Dimension::Anonymous(size));
            } else if let Some(Token::Identifier(_)) = self.peek_next() {
                let after_ident = self.tokens.get(self.pos + 2).map(|(t, _)| t);
                if matches!(after_ident, Some(Token::RBracket)) {
                    // 2026-08-01 (Phase 2): `Type[#]` (from `-> Int [#]`) is
                    // the removed entry-point marker, NOT a named array
                    // dimension. Reject it with the same clear error as
                    // parse_contract so `defn main() -> Int [#]` fails loudly
                    // instead of producing `Int[#]` (a named dimension "#").
                    if let Some(Token::Identifier(ident)) = self.peek_next() {
                        if ident == "#" {
                            return Err(crate::errors::SyntaxError::InvalidStatement {
                                reason: "'[#]' entry-point syntax removed — use the entry!/args! \
                                         macros (Phase 3) or write an explicit contract"
                                    .to_string(),
                                span: crate::errors::Span::dummy(),
                            });
                        }
                    }
                    self.pos += 1; // consume LBracket
                    let name = self.expect_identifier()?;
                    self.expect(Token::RBracket)?;
                    dims.push(Dimension::Named(name, 0));
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        if !dims.is_empty() {
            // 2026-08-22 (Phase 3): a union of array members — `Int[4] | Float[8]`.
            let first = Type::Vector(Box::new(base.1), dims);
            let mut members = vec![first];
            while self.eat(&Token::Pipe) {
                members.push(self.parse_type()?);
            }
            if members.len() == 1 {
                return Ok(members.pop().unwrap_or_else(|| Type::void()));
            }
            return Ok(Type::Union(members));
        }
        // 2026-08-22 (Phase 3): structural sums — `Int | String` is an
        // anonymous union (SPEC §8.4). `|` binds loosest in type grammar;
        // operands are full types (arrays/generics included).
        let mut members = vec![base.1.clone()];
        while self.eat(&Token::Pipe) {
            members.push(self.parse_type()?);
        }
        if members.len() == 1 {
            return Ok(base.1);
        }
        Ok(Type::Union(members))
    }

    /// Parse a named type, possibly with generic parameters or pointer prefix.
    fn parse_named_type_body(&mut self, name: &str) -> Result<Type, SyntaxError> {
        // Bit<N> — numeric bit width, no annotation = flexible.
        // 2026-08-15 (fundamentals): `Bit<N>` is the canonical spelling (the
        // unified bit type); `Bits<N>` and `bits<N>` are accepted aliases so
        // pre-2026-08-15 code keeps parsing. All three normalize to
        // `Type::Bits(N)`.
        if name == "Bit" || name == "Bits" || name == "bits" {
            if self.eat(&Token::Lt) {
                let bits = match self.peek() {
                    Some(&Token::Integer(n)) => {
                        self.pos += 1;
                        n as u64
                    }
                    _ => return self.error_at_current("expected bit count (integer) in Bit<N>"),
                };
                if !self.eat_type_close() {
                    return self.error_at_current("expected '>' or '>>' in Bit<N>");
                }
                return Ok(Type::Bits(bits));
            }
            return Ok(Type::Bits(0)); // flexible-width Bit
        }

        // Ptr<T> handling
        if name == "Ptr" {
            if self.eat(&Token::Lt) {
                let inner = self.parse_type()?;
                if !self.eat_type_close() {
                    return self.error_at_current("expected '>' or '>>' in Ptr<T>");
                }
                return Ok(Type::ptr(inner));
            }
            return Ok(Type::ptr(Type::bits(8)));
        }

        // Generic type: Foo<T, U>
        if self.eat(&Token::Lt) {
            let mut args = Vec::new();
            loop {
                // 2026-07-31 (A8): a numeric generic argument is a compile-time
                // SIZE parameter (`Stack<Int, 8>`).
                let next_is_int = matches!(self.peek(), Some(Token::Integer(_)));
                if next_is_int {
                    let n = match self.peek() {
                        Some(Token::Integer(n)) => *n,
                        _ => 0,
                    };
                    self.pos += 1;
                    args.push(Type::Number(n));
                } else {
                    args.push(self.parse_type()?);
                }
                if !self.eat(&Token::Comma) {
                    break;
                }
            }
            if !self.eat_type_close() {
                return self.error_at_current("expected '>' or '>>' to close generic type");
            }
            return Ok(Type::Applied(name.to_string(), args));
        }

        // 2026-07-25: Array syntax for custom types: MyStruct[1024].
        // 2026-07-31: Same non-greedy lookahead as keyword types — an integer
        // size or a `[Name]` generic dimension; otherwise the `[` is a
        // contract bracket.
        // 2026-08-07: MULTI-dim — accumulate dims so a generic base gets the
        // same `T[M][N]` treatment as a keyword base (SPEC §16.6).
        let mut dims: Vec<Dimension> = Vec::new();
        while self.check(&Token::LBracket) {
            if matches!(self.peek_next(), Some(Token::Integer(_))) {
                self.pos += 1; // consume LBracket
                let size = match self.peek() {
                    Some(&Token::Integer(n)) => { self.pos += 1; n as usize }
                    _ => { return self.error_at_current("expected array size (integer)"); }
                };
                self.expect(Token::RBracket)?;
                dims.push(Dimension::Anonymous(size));
            } else if let Some(Token::Identifier(_)) = self.peek_next() {
                let after_ident = self.tokens.get(self.pos + 2).map(|(t, _)| t);
                if matches!(after_ident, Some(Token::RBracket)) {
                    self.pos += 1;
                    let dim = self.expect_identifier()?;
                    self.expect(Token::RBracket)?;
                    dims.push(Dimension::Named(dim, 0));
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        if !dims.is_empty() {
            return Ok(Type::Vector(Box::new(Type::Custom(name.to_string())), dims));
        }

        Ok(Type::Custom(name.to_string()))
    }

    /// Parse a function/callback type: `fn(Int) -> Int` (or `fn(Int)` → void).
    /// 2026-08-03: crosses an FFI boundary as an opaque function pointer.
    fn parse_fn_type(&mut self) -> Result<Type, SyntaxError> {
        self.expect(Token::LParen)?;
        let mut params = Vec::new();
        if !self.check(&Token::RParen) {
            loop {
                params.push(self.parse_type()?);
                if !self.eat(&Token::Comma) {
                    break;
                }
            }
        }
        self.expect(Token::RParen)?;
        let ret = if self.check(&Token::Arrow) {
            self.pos += 1;
            Box::new(self.parse_type()?)
        } else {
            Box::new(Type::void())
        };
        Ok(Type::Function(params, ret))
    }

    /// Parse a tuple type: (Int, String)
    fn parse_tuple_type(&mut self) -> Result<Type, SyntaxError> {
        self.pos += 1; // consume LParen
        let mut types = Vec::new();
        if !self.check(&Token::RParen) {
            loop {
                types.push(self.parse_type()?);
                if !self.eat(&Token::Comma) {
                    break;
                }
            }
        }
        self.expect(Token::RParen)?;
        if types.len() == 1 {
            return Ok(types.into_iter().next().unwrap());
        }
        Ok(Type::Tuple(types))
    }

    /// Parse an optional type annotation: `: Type` or nothing.
    pub fn parse_optional_type(&mut self) -> Result<Option<Type>, SyntaxError> {
        if self.eat(&Token::Colon) {
            self.parse_type().map(Some)
        } else {
            Ok(None)
        }
    }

    /// Parse function return type: `-> Type`
    pub fn parse_return_type(&mut self) -> Result<Option<Type>, SyntaxError> {
        if self.eat(&Token::Arrow) {
            self.parse_type().map(Some)
        } else {
            Ok(None)
        }
    }

    /// Parse type parameters: `<T, U>` or nothing.
    pub fn parse_type_params(&mut self) -> Result<Vec<crate::ast::top::TypeParam>, SyntaxError> {
        if self.eat(&Token::Lt) {
            let mut params = Vec::new();
            loop {
                let name = self.expect_identifier()?;
                // 2026-07-20: Optional bound: K: #String or K: String
                let bound = if self.eat(&Token::Colon) {
                    Some(self.parse_type()?)
                } else {
                    None
                };
                params.push(crate::ast::top::TypeParam { name, bound });
                if !self.eat(&Token::Comma) {
                    break;
                }
            }
            self.expect(Token::Gt)?;
            Ok(params)
        } else {
            Ok(Vec::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Token;
    use logos::Logos;

    fn parse_type_str(src: &str) -> Type {
        let tokens: Vec<(Token, std::ops::Range<usize>)> = Token::lexer(src)
            .map(|r| (r.unwrap(), 0..0))
            .collect();
        let mut p = Parser::new(tokens, src);
        p.parse_type().expect("type should parse")
    }

    #[test]
    fn fn_type_annotation() {
        let t = parse_type_str("fn(Int) -> Int");
        assert!(matches!(t, Type::Function(params, ret) if params.len() == 1 && matches!(params[0], Type::Custom(ref n) if n == "Int") && matches!(*ret, Type::Custom(ref n) if n == "Int")));
    }

    #[test]
    fn multi_dim_array_type() {
        // 2026-08-07 (Phase 7): `T[M][N]` accumulates dims into one Vector —
        // the Matrix<T, Rows, Cols> enabler (SPEC §16.6).
        let t = parse_type_str("Int[2][3]");
        match t {
            Type::Vector(inner, dims) => {
                assert!(matches!(*inner, Type::Custom(ref n) if n == "Int"));
                assert_eq!(dims.len(), 2);
                assert!(matches!(dims[0], Dimension::Anonymous(2)));
                assert!(matches!(dims[1], Dimension::Anonymous(3)));
            }
            other => panic!("expected a 2-dim Vector, got {other:?}"),
        }
    }

    #[test]
    fn multi_dim_array_named_dims() {
        // A generic base accumulates named dims too (`T[Rows][Cols]`).
        let t = parse_type_str("T[Rows][Cols]");
        match t {
            Type::Vector(_, dims) => {
                assert_eq!(dims.len(), 2);
                assert!(matches!(dims[0], Dimension::Named(ref n, _) if n == "Rows"));
                assert!(matches!(dims[1], Dimension::Named(ref n, _) if n == "Cols"));
            }
            other => panic!("expected a 2-dim Vector, got {other:?}"),
        }
    }

    #[test]
    fn fn_type_void_return_defaults() {
        let t = parse_type_str("fn(Int)");
        assert!(matches!(t, Type::Function(params, ret) if params.len() == 1 && matches!(*ret, Type::Void)));
    }

    #[test]
    fn fn_type_multi_param() {
        let t = parse_type_str("fn(Int, Float) -> Bool");
        assert!(matches!(t, Type::Function(params, ret) if params.len() == 2 && matches!(*ret, Type::Custom(ref n) if n == "Bool")));
    }
}
