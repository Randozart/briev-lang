// ── `List<dyn Trait>` — interpreter contract pin ─────────────────────────
// 2026-10-12: the reference for the backend's `List<dyn>` element work
// (SPEC §8.6.1 Option B, derived shape). The interpreter runs heterogeneous
// dispatch from a list of differing-field concretes; whatever it computes is
// the contract the LLVM backend must reproduce.
//
// Probes drive the reactor manually and observe the `endprogram` exit value
// (the reactor propagates RuntimeError::ProgramExit(code)). Probe A exits on
// iteration 0 with handlers[0].greet(1) → 701 (Dog.base=7: 7*100+1). Probe B
// exits on iteration 1 with handlers[1].greet(1) → 91 (Cat.lives=9: 9*10+1) —
// element 1 has a DIFFERENT field layout than element 0, which is exactly the
// shape the compiled backend corrupts today (boxed-concrete store vs
// fat-pointer read mismatch).

use briev_compiler::errors::RuntimeError;
use briev_compiler::interpreter::{Interpreter, Value};
use briev_compiler::reactor::Reactor;

const PROBE_A: &str = r#"
trait Greeter {
    defn greet(me: Self, times: Int) -> Int;
};
type Dog: Greeter { base: Int; };
type Cat: Greeter { lives: Int; };
impl Dog {
    defn greet(me: Dog, times: Int) -> Int { term me.base * 100 + times; }
};
impl Cat {
    defn greet(me: Cat, times: Int) -> Int { term me.lives * 10 + times; }
};
let handlers: List<dyn Greeter> = [Dog { base: 7 }, Cat { lives: 9 }];
let i: Int = 0;
node run [i < 2][i == 2] {
  when i == 0 {
    Print#(handlers[0].greet(1));
    endprogram handlers[0].greet(1);
  };
  i = i + 1;
  term;
};
"#;

const PROBE_B: &str = r#"
trait Greeter {
    defn greet(me: Self, times: Int) -> Int;
};
type Dog: Greeter { base: Int; };
type Cat: Greeter { lives: Int; };
impl Dog {
    defn greet(me: Dog, times: Int) -> Int { term me.base * 100 + times; }
};
impl Cat {
    defn greet(me: Cat, times: Int) -> Int { term me.lives * 10 + times; }
};
let handlers: List<dyn Greeter> = [Dog { base: 7 }, Cat { lives: 9 }];
let i: Int = 0;
node run [i < 2][i == 2] {
  when i == 0 {
    i = i + 1;
  };
  when i == 1 {
    Print#(handlers[1].greet(1));
    endprogram handlers[1].greet(1);
  };
  term;
};
"#;

/// Drive the reactor to the program boundary; return the exit value.
fn run_to_exit(src: &str) -> Result<Value, String> {
    let tokens = briev_compiler::lexer::tokenize(src).map_err(|e| format!("lex: {e:?}"))?;
    let mut p = briev_compiler::parser::Parser::new(tokens, src);
    let items = p.parse_program().map_err(|e| format!("parse: {e:?}"))?;
    let mut interp = Interpreter::new();
    interp.load_program(&items);
    interp.seed_inits(&items).map_err(|e| format!("seed: {e:?}"))?;
    // Top-level `let` statements are state fields: execute each through the
    // statement path so the dyn coercion at the annotation fires (a raw
    // eval_expr would skip it). seed_inits only covers `TopLevel::Init`.
    for item in &items {
        if let briev_compiler::ast::TopLevel::Statement(s) = item {
            interp.exec_stmt(s).map_err(|err| format!("seed stmt: {err:?}"))?;
        }
    }
    let mut reactor = Reactor::new();
    reactor.build_from_program(&items);
    // build_from_program marks every reactive node dirty (the bootstrap
    // firing). Fire rounds until the program boundary; `endprogram` surfaces
    // as ProgramExit(code). No clear_dirty before the first round — the
    // bootstrap dirty set IS the initial wake.
    loop {
        match reactor.run(&mut interp) {
            Ok(true) => {}
            Ok(false) => {
                return Err("reactor reached equilibrium without endprogram".into())
            }
            Err(RuntimeError::ProgramExit(v)) => return Ok(v),
            Err(e) => return Err(format!("runtime: {e:?}")),
        }
    }
}

#[test]
fn interpreter_list_dyn_first_element_dispatch() {
    let v = run_to_exit(PROBE_A).expect("probe A runs to endprogram");
    assert_eq!(v.as_i64(), Some(701), "handlers[0].greet(1) = Dog.base*100+1");
}

#[test]
fn interpreter_list_dyn_second_element_differing_field() {
    let v = run_to_exit(PROBE_B).expect("probe B runs to endprogram");
    assert_eq!(v.as_i64(), Some(91), "handlers[1].greet(1) = Cat.lives*10+1");
}
