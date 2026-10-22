// Copyright 2026 Randy Smits-Schreuder Goedheijt
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception

//! Concrete enum image layout (P2-1 Option B, B1).
//!
//! The image is `1 tag word + max(variant payload words)`, with a descendant
//! enum laid out inline inside the payload area. Unknown, aggregate, or
//! recursive-beyond-cap payloads consume one boxed handle word.

use crate::ast::{EnumVariant, Statement, TopLevel, Type};
use std::collections::HashMap;

#[derive(Debug, Clone)]
struct EnumDef {
    params: Vec<String>,
    variants: Vec<(String, Type)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumLayout {
    /// Total i64 words in the image (tag + payload area).
    pub image_words: usize,
    /// Per-variant payload word count, in declaration order.
    pub variant_payload_words: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct EnumLayoutPlanner {
    enums: HashMap<String, EnumDef>,
    max_depth: usize,
}

impl EnumLayoutPlanner {
    pub fn new(items: &[TopLevel], max_depth: usize) -> Self {
        let mut enums = HashMap::new();
        for item in items {
            match item {
                TopLevel::TypeDef(td) => {
                    let variants: Vec<(String, Type)> = td
                        .body
                        .slots
                        .iter()
                        .filter_map(|s| {
                            s.name.strip_prefix("__variant_").map(|n| (n.to_string(), s.ty.clone()))
                        })
                        .collect();
                    if !variants.is_empty() {
                        enums.insert(
                            td.name.clone(),
                            EnumDef {
                                params: td.type_params.iter().map(|p| p.name.clone()).collect(),
                                variants,
                            },
                        );
                    }
                }
                TopLevel::Enum(e) => {
                    let variants = e
                        .variants
                        .iter()
                        .map(|v| match v {
                            EnumVariant::Unit(n) => (n.clone(), Type::Void),
                            EnumVariant::Tuple(n, ts) => {
                                (n.clone(), if ts.len() == 1 { ts[0].clone() } else { Type::Tuple(ts.clone()) })
                            }
                            EnumVariant::Struct(n, fields) => {
                                (n.clone(), Type::Tuple(fields.iter().map(|(_, t)| t.clone()).collect()))
                            }
                        })
                        .collect();
                    enums.insert(
                        e.name.clone(),
                        EnumDef {
                            params: e.type_params.iter().map(|p| p.name.clone()).collect(),
                            variants,
                        },
                    );
                }
                _ => {}
            }
        }
        Self { enums, max_depth }
    }

    /// Layout for a concrete enum instance. `None` when the type is not a
    /// declared enum.
    pub fn layout_for(&self, ty: &Type) -> Option<EnumLayout> {
        self.layout_at(ty, 0)
    }

    fn layout_at(&self, ty: &Type, depth: usize) -> Option<EnumLayout> {
        let (name, args) = match ty {
            Type::Custom(n) => (n.as_str(), &[][..]),
            Type::Applied(n, a) => (n.as_str(), a.as_slice()),
            _ => return None,
        };
        let def = self.enums.get(name)?;
        let subst: HashMap<String, Type> = def
            .params
            .iter()
            .cloned()
            .zip(args.iter().cloned())
            .collect();
        let mut variant_payload_words = Vec::with_capacity(def.variants.len());
        for (_, payload) in &def.variants {
            let concrete = if subst.is_empty() {
                payload.clone()
            } else {
                crate::typechecker::substitute_type(payload, &subst)
            };
            variant_payload_words.push(self.payload_words(&concrete, depth + 1));
        }
        let payload_words = variant_payload_words.iter().copied().max().unwrap_or(0).max(1);
        Some(EnumLayout {
            image_words: 1 + payload_words,
            variant_payload_words,
        })
    }

    fn payload_words(&self, ty: &Type, depth: usize) -> usize {
        match ty {
            Type::Void => 0,
            Type::Tuple(elems) => elems.iter().map(|e| self.payload_words(e, depth)).sum(),
            // A concrete enum payload is inline while within the depth cap.
            Type::Custom(_) | Type::Applied(_, _) => {
                if depth < self.max_depth {
                    self.layout_at(ty, depth)
                        .map(|l| l.image_words)
                        .unwrap_or(1)
                } else {
                    1
                }
            }
            Type::Vector(inner, _) => self.payload_words(inner, depth).max(1),
            _ => 1,
        }
    }
}

// Keep this module independent of statement parsing internals.
#[allow(dead_code)]
fn _statement_marker(_: &Statement) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Vec<TopLevel> {
        let toks = crate::lexer::tokenize(src).expect("tokenize");
        crate::parser::Parser::new(toks, src)
            .parse_program()
            .expect("parse")
    }

    fn planner() -> EnumLayoutPlanner {
        let items = parse(
            "enum TcpState { Closed, Listen };\n\
             enum FsmError { Illegal };\n\
             enum WsFrame { Empty };\n\
             enum Result<T, E> { Ok(T), Err(E) };\n",
        );
        EnumLayoutPlanner::new(&items, 2)
    }

    fn applied(name: &str, args: Vec<Type>) -> Type {
        Type::Applied(name.to_string(), args)
    }

    #[test]
    fn nested_enum_payload_is_inline() {
        let p = planner();
        let layout = p
            .layout_for(&applied(
                "Result",
                vec![Type::Custom("TcpState".into()), Type::Custom("FsmError".into())],
            ))
            .expect("layout");
        // TcpState image = 2, FsmError image = 2 -> Result image = 3.
        assert_eq!(layout.image_words, 3);
        assert_eq!(layout.variant_payload_words, vec![2, 2]);
    }

    #[test]
    fn scalar_payload_is_flat() {
        let p = planner();
        let layout = p
            .layout_for(&applied("Result", vec![Type::int(), Type::int()]))
            .expect("layout");
        assert_eq!(layout.image_words, 2);
        assert_eq!(layout.variant_payload_words, vec![1, 1]);
    }

    #[test]
    fn depth_cap_boxes_deeper_nesting() {
        let p = planner();
        let deep = applied(
            "Result",
            vec![applied("Result", vec![Type::int(), Type::int()]), Type::int()],
        );
        let layout = p.layout_for(&deep).expect("layout");
        // depth 0 Result: TcpState-like nested Result at depth 1 is inline
        // (2 words); overall image = 3.
        assert_eq!(layout.image_words, 3);
        let deeper = applied("Result", vec![deep, Type::int()]);
        // With a depth cap of 1, the nested Result is a boxed handle.
        let shallow = parse(
            "enum TcpState { Closed };\n\
             enum FsmError { Illegal };\n\
             enum Result<T, E> { Ok(T), Err(E) };\n",
        );
        let p1 = EnumLayoutPlanner::new(&shallow, 1);
        let layout = p1.layout_for(&deeper).expect("layout");
        assert_eq!(layout.image_words, 2);
    }
}
