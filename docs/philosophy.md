# Briev: Design Philosophy

> Extracted from the README (2026-07-31).

## Briev Doesn't Break

**Status:** v0.18.0 — GLUE Bridge Protocol, TOML-Driven Export, Cross-Language FFI Pipeline

Briev is a contract-enforced language designed for building verifiable state machines. It treats program execution as a series of verified state transitions rather than sequential instructions. The file extension selects the compilation target. Each one optimizes the same contract-proven logic for a different material:

| Extension | Variant | Compiles to |
|-----------|---------|-------------|
| `.bv` | **Briev** | LLVM native binary (optional SPIR-V offload) |
| `.rbv` | **Rendered Briev** | TypeScript + frontend code + WASM sidecars |
| `.ebv` | **Embedded Briev** | LLVM microcontroller binary |
| `.abv` | **Accelerated Briev** | SPIR-V GPU kernel |
| `.sbv` | **Silicon Briev** | CIRCT hardware description (Verilog/VHDL) |
| `.dbv` / `.dbvs` / `.dbvl` | **Data Briev** | Configuration data parsed by Briev itself |

The main sources of inspiration are Rust (by Graydon Hoare and the Rust community) and Dialog (by Linus Åkesson). Specifically the fact that both have a very strict compiler, that catches bad code before it ever compiles, simply through smart conventions. Especially the declarative nature is inspired by Dialog, as a direct successor of Prolog, since Dialog showed that setting up a series of predicates could be sufficient to have a compiler figure out a complex runtime capable of simulating a world. And the reactor loop? That was inspired by, well... React. As such, everything in Briev is designed to, in some way, aid in predictable runtime cascades. You set up the first billiard ball, and based on the variables present describing the overall "state", the rest of the balls predictably scatter.

Note that much of this language design was inspired by designing a language that would be impossible for an LLM to get wrong. Therefore it feels important to me to disclaim a lot of AI has been used in building this compiler. The design is fully my own (Randy Smits-Schreuder Goedheijt), but much of the programming was handled by LLMs, and the verification by hand and a series of unit tests (which LLMs somehow manage to cut the corners of). As such, you will find comments, markdown files and many more typical signs of LLM usage in this repository. These all exist to help steer the LLM into *correctly* modifying and applying the design decisions I have made, as it would otherwise be prone to hallucinate a novel language like this. Ergo, you will find a veritable library of markdown files written by AI, just to make sure everything got documented as I went.

If you've gotten this far, I thank you for reading, and I hope you will have enjoyed your *Briev* time here so far.

Regards,

**Randy**

## The Thesis: Topology over Timing

Most programming languages are built around _operations in sequence_. Briev describes the _sequence of operations_ - the spatial connections between logical states.

*   **Logic as a Map:** Briev defines a world where roads exist all at once. The "sequence" is then can call o _connection_, not the _timing_.
*   **Physical Isomorphism:** Because the logic defines a _shape_ rather than a _schedule_, it adapts to the physics of its material:
    *   **In Software:** The compiler hires a worker (the CPU) to walk these roads in order.
    *   **In Hardware:** The compiler builds the roads directly out of copper.
*   **Variable Logic:** The logic remains invariant while the material changes. A square is a square whether it's drawn in the sand or forged in steel.

**Deep Dive:** There are several .md documents scattered across the repo with random ideas on optimizing the language. Some are outdated, some aren't, but they should show the development of the Briev philosophy over time, and ways in which the topological approach has allowed backend optimization not otherwise available.

## The Philosophical Pillars

### All operations are expressed in nodes, and only nodes and transactions can call operations. They either complete fully, or not at all.

Nodes and transactions are inherently cyclical. If you properly define a postcondition a cyclically executed transaction will eventually reach, it automatically starts behaving like a loop, but one that can predictably halt. A transaction with `[pre][post]` converges when the precondition becomes false. This means the postcondition describes the terminal state, and the precondition is the loop condition. You do not write `while (counter < 100)`, instead the precondition `[counter < 100]` already says "keep running while this holds." You do not write `for (i = 0; i < N; i++)`, here too the postcondition `[i == @i + 1]` expresses the step and the invariant all at once. The compiler proves the postcondition is reachable and that the loop terminates. This gives the contract system a role beyond *merely* serving the proof engine.

### Briev doesn't need you to be correct, it just needs you to be right.

The contract logic often just requires you to declare either the precondition or postcondition, not both. Contracts are simultaneously specification AND optimization input. In most languages, types/specs are safety rails that constrain what you can do. In Briev, they're also what the optimizer feeds on. The more you declare, the more the compiler can prove, and the faster your program runs. The file extension system (.bv → warnings, .sbv → hard errors) embodies the idea that you opt into strictness as your understanding deepens. Partial contracts compile with warnings. Full contracts with strict mode compile with proofs. This is a choice that distinguishes Briev from total languages (Coq, Agda) where you must prove everything upfront, and from mainstream languages where you prove nothing.

### Execution is inferred, not prescribed.

Programs are declared through a combination of variables, definitions and transactions. The entire program runs on a non-polling reactor loop. It indexes which variable changes lead to which `node` preconditions to be fulfilled, and fires them automatically when it's their time to act. Because these paths are laid out predictably, the compiler has great leeway in folding these paths. If X through A, B and C will always lead to Y with side-effect Z, the compiler will simply draw a short route from X to YZ.

### No magic, but I had to compromise somewhere.

Every function and keyword in Briev must be traceable to a source following the same rules as every other definition. If it looks like `foo(x)`, it is user-defined. Period. The exception is `#`-suffixed intrinsics like `print_int#`, `sqrt#`, `put_char#`. These are baked into `Expr::IntrinsicCall` in the AST, but they are *explicitly* marked with the `#` at every call site. You can never mistake `sqrt` for `sqrt#`. The `#` is the compiler saying "I have a hand in this one." It is a compromise, but an honest one.

The "coding" system, where top-level `let` declarations and guarded blocks get implicitly wrapped into a reactive transaction, is the one invisible transformation the compiler does. But the transformation is predictable and the same for every program. It is the practical muscle behind "execution is inferred, not prescribed." The compiler tells you what it inferred. You can always look at the expanded form.

Anything interacting with an external language or interrupt source must be declared explicitly. Which FFI path that takes depends on your target:
- **LLVM target** (`.bv`, `.ebv`): `frgn from "c"` resolved via `briev_rt.c`.
- **Web target** (`.rbv`): `frgn from "javascript"` inlined into generated TypeScript.
- **Hardware target** (`.sbv`): no FFI allowed. If you need something external, it comes through an intrinsic. This is the strictest tier, because you are describing copper.
- **GPU target** (`.abv`): intrinsics only, same as hardware.

### Contracts are optimization fuel, not a correctness tax.

This is an odd one I discovered I could do while optimizing Briev. In most languages, a precondition, assertion or some other safety wrapper is you doing the compiler or even just the runtime a favor to prevent messy logic from crashing the program. In Briev, the contract *is* the optimization input. The more you declare, the more the compiler proves, and the faster your program runs. Safety enables speed.

A precondition like `[x < N]` does more than guard the transaction. The compiler uses this information to emit `!range` metadata on the field load, which lets LLVM eliminate bounds checks in the loop body. More contracts means more metadata, which means more guarantees about the code. The optimizer feeds on what the prover proves.

This is why strict variants (`.sbv`) ban sugar syntax. If you are writing hardware or safety-critical code, you should not take shortcuts. The full `[pre][post]` contract is the compiler's primary optimization signal. When you omit one side, you are leaving performance on the table, but also opening yourself up to unpredictable and undefined behaviour. However, sometimes this asks too much of a programmer, which is why the file extension serves as the opt-in.

So, instead of thinking *"safety checks slow me down, I will add them later."*, think *"the compiler cannot optimize what it cannot prove."* Write the contract first. The performance follows.

### Friction is a signal...

There is no `if/else` in Briev. There are guarded blocks: `[condition] { body }`. This is not an omission. A guard forces you to ask "what must be true for this to execute?" rather than "which branch do I take?" If it feels harder than `if`, that is because you are specifying an invariant instead of a jump. The friction is the point. Operators that alter normal flow are marked with `!`: `term!` exits the program, `trg!` fires a hardware trigger, `sync!` forces a barrier, `$!` marks a high-power macro with access to `compile#`, `gensym#`, and `error#`. The `!` is the language saying "this is not a normal operation." If it feels heavy, good. It should. The strict variants (`.sbv`) exist precisely to add friction. Sugar is banned, full contracts are required. You opt into strictness as your understanding deepens. The compiler does not let you take shortcuts when the material (hardware, safety) cannot afford them.

### ...but the compiler must help you through it.

Friction without explanation is frustration. Every denied sugar, every strict-mode requirement, every full-contract demand should tell you *why* and *what to do instead*. If the compiler says "no," it should say "here is the path I can accept." This is why the language design keeps error messages concrete. A warning like `sugar syntax [[post]] not allowed in .sbv files, write [pre][post] explicitly` is better than `invalid syntax`. The friction exists to make you think, not to waste your time. The compiler's job is to make sure you know the difference.

### Operator Taxonomy

Briev's operators are organized into three conceptual groups:

| Group | Operators | Purpose |
|-------|-----------|---------|
| **Lens Operators** | `<:` (Derivation), `:>` (Projection) | Type boundaries and semantic lenses — restricts what conforms to a type, or reveals meaning through a lens |
| **Partition Operators** | `[]`, `@/` | Segment layouts into addressable sub-ranges — constrains focus to a spatial slice |
| **Transfer Operator** | `<-` | Directional data movement across layout boundaries — push, pop, discard, transfer |

The **Anchor** (`@`) is the universal symbol for spatial and temporal location, used across all groups: prior state (`@balance`), bit positions (`@/0..3`), string literals (`@"..."`), and hardware links (`trg timer @ 1kHz`).

