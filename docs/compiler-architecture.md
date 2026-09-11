# Briev Compiler Architecture

> Extracted from the README (2026-07-31). The pipeline diagram below is the
> conceptual data flow; see `docs/architecture/` for the deep backend and
> casting-graph documentation, and `docs/plans/2026-07-31-frontend-driven-
> dispatch.md` for the frontend-driven analysis the LLVM backend consumes.

```mermaid
graph TD
    S["Source<br>(.bv/.sbv/.rbv/.ebv/.abv/.dbv)"] --> Lex[Lexer: src/lexer.rs]
    Lex -->|Token stream| Par[Parser: src/parser/]
    Par -->|AST| Imp[Import Resolver: import_resolver.rs]
    Imp -->|Resolved AST| UB[TypeUniverse: type_universe.rs]
    UB -->|Frozen universe| NT[NormalizeTypes: normalize_types.rs]
    NT -->|Normalized AST| TC[Type Checker: src/typechecker/]
    TC -->|Typed AST| SA[Shared Analysis: src/analysis/]

    SA --> LS[Loop Shapes]
    SA --> SG[Swan Songs]
    SA --> CD[Computed Density]
    SA --> MP[Modulo Partitions]
    SA --> CS[Cast Graph]

    SA --> Backends[Backends]

    subgraph Backends[Backends]
        LLVM[LLVM Backend: llvm/]
        Web[Webstack Backend]
        CIRCT[CIRCT Backend]

        LLVM -->|.bv -> LLVM IR| Native[Native binary]
        LLVM -->|.ebv -> MCU bin| MCU[Microcontroller binary]
        LLVM -->|.abv -> SPIR-V| GPU[GPU kernel]

        Web -->|.rbv -> TS + WASM| WebApp[Web frontend]
        CIRCT -->|.sbv -> MLIR| HDL["Verilog / VHDL"]
    end

    Backends --> LSP[LSP Server]

    style Backends fill:#484,color:#fff
```

## The frontend-driven principle

The LLVM backend CONSUMES decisions computed once in `AnalysisResults`
(loop shapes, swan-song hoists, density, modulo partitions, inline decisions,
batch shapes) and derives type knowledge from the casting graph — it never
re-derives a decision from a body re-walk, and never matches Briev type names.
Tunables live in `config/targets.toml` + `config/ir-lowering.toml`. See
`docs/architecture/backend-architecture.md`.
