# Legacy Surface Index — removed/old syntax still in the codebase

**Date:** 2026-09-08
**Purpose:** index every piece of removed/old language surface that still
exists in the compiler, so a cleanup pass can decide each site's fate.
**Categories:** (a) lexed · (b) parsed · (c) rejected-with-error · (d) dead
codegen/analysis · (e) documented-only.

---

## 1. Lexer tokens for removed keywords

Only three removed/reserved words still have dedicated lexer tokens.

| Token | Lexer | Vocab status | Fate |
|---|---|---|---|
| `meld` | `src/lexer.rs:212` | Removed | (c) rejected — `definitions.rs:280` |
| `pvt` | `src/lexer.rs:274` | Reserved | (c) rejected — `helpers.rs:463` |
| `sed` | `src/lexer.rs:277` | Reserved | (c) rejected — `helpers.rs:464` |

All other removed words (`sig`, `state`, `rstruct`, `uni`, `is`, `like`,
`prop`, `syscall`, `escape`, `term!`, `trg!`, `cell!`, `sync!`, `frgn!`,
`syscall!`, `Ptr!`, `Ok`, `Err`, `Some`, `None`, `some`, `none`, `cycles`,
`seconds`, `minute`, `minutes`, `nanoseconds`) lex as plain identifiers —
vocab-only classification, no parser meaning.

**Stale test data:** `lexer.rs:748-757` lists `"term!"` and `"prop"` as
keywords "the lexer recognizes" — no such tokens exist. Passes only because
`vocab.keyword_status()` returns `Removed`.

---

## 2. Parser rejections (recognize → reject with diagnostic)

| Removed form | Site | Kind |
|---|---|---|
| `meld` top-level decl | `definitions.rs:276-286` | StagedFeature error |
| `meld`/`reg`/`pvt`/`sed` as identifiers | `helpers.rs:188-193, 458-467` | reserved-word error |
| `if`/`else` statement | `statements.rs:148-167` | error → use `match`/`when` |
| `return` | `statements.rs:135-147` | error → use `term` |
| `[cond] { body }` guard block | `statements.rs:393-406` | error → use `when` |
| `[#]` entry-point marker | `definitions.rs:1526-1542`, `types.rs:130-143` | error → use `entry!` |
| `fallback` clause on `frgn` | `definitions.rs:542-551` | error |
| `frgn name @ address` (MMIO form) | `definitions.rs:511-518` | error |
| `frgn?`/`frgn!`/`frgn?!` | `definitions.rs:336,366` | dead fields (`is_fire_forget`/`is_delivery` always false) |
| `render struct`/`render obj` | `definitions.rs:439-441` | implicit — `expect_identifier` |
| `as <name>` inverted binding | `definitions.rs:502-509` | grammar change to `:` |
| old stage names `Front/Mid/Post/Back` | `definitions.rs:1346-1361` | migration error |
| `.port` on `trg` | `statements.rs:315`, `definitions.rs:1435` | grammar drop (no `port` field) |
| `<- &queue;` `&` marker | `statements.rs:111,506` | comment-only |
| duration aliases `seconds/minute/cycles` | `definitions.rs:1585-1641` | only `cyc/ns/ms/s/min` accepted |
| **`foreach (item in list)` paren form** | `statements.rs:286-290` | **(b) STILL PARSED** — tolerated legacy |

---

## 3. Dead/legacy codegen arms (LLVM intrinsics)

| Intrinsic arm | Site | Reachability |
|---|---|---|
| `Now#` | `intrinsics.rs:39-45` | (d) watchdog emits `@__briev_now` directly; no AST path |
| `GetEnv#` + `emit_get_env` | `intrinsics.rs:71, 1001-1041` | (d) typechecker rejects with migration error |
| `GetEnvInt#` + `emit_get_env_int` | `intrinsics.rs:72, 1043-1064` | (d) same |
| `Len#` (shares arm with `Length#`) | `intrinsics.rs:159` | (d) not registered; tests only |
| `Cast#` + `emit_intrinsic_cast` | `intrinsics.rs:198, 1590-1613` | (d) `(Type)expr` → `Expr::Cast`, never a call |
| duplicate unreachable `"Length#"` arm | `intrinsics.rs:161` | (d) shadowed by `Len# \| Length#` |
| `GetEnv#`/`GetEnvInt#` whitelisted in normalizer | `normalizer.rs:129` | (d) stale allow-list |
| `TypeError::RemovedIntrinsic` | `errors.rs:328-331, 381` | (d) never constructed |

---

## 4. Legacy tokens/operators — NOT lexed

`:>`, `<:`, `|>`, `++`, `#pragma`, `#!exit`, `#?`, `#[` decompose into
punctuation/identifier fragments. Confirmed by lexer tests (`lexer.rs:914-930,
1052-1068`).

- `#!exit` — comment-only in LLVM/analysis (`llvm/mod.rs:3090,3856,…`;
  `loop_shape.rs:61` "Mirrors the old synthetic-exit construction").
- `#nowake`/`#wake` — gone; `is_wake` survives as name heuristic
  `trg.name.starts_with("__wake")` (`llvm/mod.rs:2706`).
- `#dispatch`, `#io` — zero hits (except old plans).
- `#inline`/`#unroll`/`#vectorize`/`#export` — **ACTIVE** directives
  (`backend/llvm/directive.rs:18-23`), not removed.

---

## 5. Stale feature docs (document removed features as active)

| Doc | Claims | Reality |
|---|---|---|
| `features/inop.md` | implemented | `inop`/`inop!` removed |
| `features/meld.md` | core infrastructure complete | `meld` removed, parser rejects |
| `features/is-from-like.md` | `is`/`like` implemented | both removed (but `Expr::IsType` survives, §6) |
| `features/pipe.md` | `|>` complete | removed; `desugarer.rs` empty |
| `features/projection.md` | `:>` implemented (18 targets) | removed; `Expr::Reflect` replaced it |
| `features/sigcall.md` | `sig` fully parsed | removed; `SigModifier` enum unused |
| `features/match-uni-arrow.md` | `uni` match arrows | removed |
| `features/cell.md` | `cell!` syntax | removed |
| `features/frgn-pipe.md` | `frgn ... \| fallback` implemented | `fallback` clause removed |
| `features/ptr.md` | `Ptr!` projection target | partially stale |
| `features/rstruct.md` | marks `rstruct` deprecated → `render` | **accurate** |
| `features/temporal-fallback.md` | REMOVED | **accurate** |

---

## 6. Stale AST variants (no parser producer, still handled downstream)

| Variant | AST | Handled in (dead paths) |
|---|---|---|
| `Expr::If` | `expr.rs:106` | LLVM `emit_expr.rs:867`, VM `vm/emit_expr.rs:150`, SPIR-V `lower.rs:906`, interpreter, analysis, beastpack |
| `Expr::IsType` | `expr.rs:124` | LLVM (constant-true stub), interpreter, analysis, beastpack |
| `TopLevel::StateDecl` | `top.rs:46` | pipeline, view_compiler, import_resolver, compile |
| `TopLevel::Signature` | `top.rs:47` | lsp, glue/export, import_resolver |
| `TopLevel::ResourceDecl` | `top.rs:49` | import_resolver, canonical |
| `TopLevel::LinkDependency` | `top.rs:48` | import_resolver, hardware_validator, canonical |
| `TopLevel::Codec` | `top.rs:54` | canonical |
| `TopLevel::Assertion` | `top.rs:55-58` | canonical |
| `TopLevel::TriggerBinding` | `top.rs:39-45` | llvm/mod, import_resolver, dependency_graph, canonical (parser produces `Statement::TrgBinding`) |
| `SigModifier` enum | `top.rs:512-517` | zero uses (dead type) |
| `ForeignBinding.is_fire_forget`/`is_delivery` | `top.rs:836-839` | always false; read at `analysis/frgn_guard.rs:20` |
| `WatchdogSpec.fallback` | `top.rs:494` | always None; never read (dead field) |

**Stale doc comments on live variants:** `Statement::Rollback` still says
`/// escape expr;` (`top.rs:340`); `top.rs:326` + `statements.rs:259`
document `endprogram` as the rename of `term!`.

---

## 7. Removed pragmas

- `#pragma` — removed (C headers in `lib/glue/c/` only).
- `#!exit` — comment-only (§4).
- `#dispatch`, `#io` — gone.
- `#nowake`/`#wake` — `is_wake` heuristic remnant (§4).
- `#on_exit` — lexer comment (`lexer.rs:186`, defer replaced it); **still
  modeled in `lib/compiler/*.bv`** (`ast.bv:69` `StmtOnExit`,
  `backends/c.bv:163`, `rust.bv:181`, `backend_aarch64.bv:1267`).

---

## 8. Removed syntax in `lib/compiler/*.bv` (self-hosted sources)

`lib/compiler/` still uses removed syntax:
- `uni` pattern-matches (`main.bv:48,57,73,112,116,120`; `call_graph.bv:46…`)
- `Some`/`None`/`Ok`/`Err` (`lexer.bv:34-44`, `call_graph.bv:59`)
- removed keyword tokens (`token.bv:18-28,33,44,56,77-78` —
  `KeywordState/KeywordEscape/KeywordSig/KeywordRstruct/KeywordUnification/KeywordOk/KeywordErr`)
- `#!` `HashBang` (`token.bv:138`)

Not part of the current compile pipeline; active shipped surface in `lib/`.

---

## 9. Summary

| Category | Count | Notable |
|---|---|---|
| (a) lexed removed tokens | 3 | `meld`, `pvt`, `sed` |
| (b) parsed legacy forms | 2 | `foreach (x in list)`, `sync { }` block |
| (c) rejected-with-error | 15+ | §2 |
| (d) dead codegen/analysis | ~26 | 5 LLVM arms, 1 dup, ~12 AST variants, 3 dead fields, 1 dead diagnostic, 2 dead whitelist items |
| (e) documented-only | ~15 | 12 stale feature docs, `#!exit`/`#on_exit`, stale lexer test, `lib/compiler/*.bv` |

**Highest-value cleanup targets:** (d) `Expr::If`/`Expr::IsType`/
`TopLevel::StateDecl`/`Signature` dead variants and the 5 dead LLVM intrinsic
arms; (e) the 12 stale `features/*.md` docs; (b) the two tolerated legacy
parse forms.