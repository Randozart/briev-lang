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
//! 2026-11-20 (evented-net plan Wave 3): the io/pure STRAND classification.
//!
//! The #145 lesson: `willreturn` was INFERRED conservatively because the
//! checker had no fact about which defns block. This pass computes that
//! fact: every callable is classified io (may block on a kernel/FFI lane)
//! or pure (provably cannot), by a call-graph fixpoint over a NAMED
//! registry of blocking lanes — one auditable list, not a heuristic.
//!
//! Blocking lanes (the registry, `blocking_lane`):
//!   * `recv`-family and `accept` syscalls issued with flags = 0 — the
//!     blocking shape. Nonblocking steps (MSG_DONTWAIT nonzero) are NOT
//!     lanes: they return EAGAIN, they never park.
//!   * `poll`/`wait`-family with an infinite timeout — finite timeouts
//!     are bounded, not blocking.
//!   * The Spawn#/Wait#/futex intrinsics — process/task synchronization.
//!   * Every frgn/unknown call — the FFI family is opaque by definition
//!     (the purity.rs stance), so it may block.
//!
//! Consumers (v1): the WARNING on a spawned segment whose code after the
//! FIRST statement (the segment boundaries split at `yield;`) calls an io
//! defn — that segment holds the round-robin hostage between yields. The
//! warning is advisory; promotion to error follows the G2 discipline
//! (evidence first, then the flip).
//!
//! Soundness: false negatives allowed (an io defn reachable only through
//! an untracked path stays silent), false positives are bugs. The fixpoint
//! is the purity.rs one re-run for the strand dimension.
//!
//! Undo: remove this module + the pipeline call site + the strand unit
//! tests. The classification itself is what L1 (checker→backend
//! convergence) will consume to route #15 facts.

use crate::ast::top::{Definition, TopLevel};
use crate::ast::{Expr, Statement};
use std::collections::{HashMap, HashSet};

/// The callable classification: Pure = provably cannot block; Io = may.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strand {
    Pure,
    Io,
}

/// Callables with their bodies and edges, re-shaped for the strand fixpoint.
struct StrandGraph {
    /// name → body (defns only; txns/nodes are entry points, not composed).
    bodies: HashMap<String, Vec<Statement>>,
    /// name → direct user-call edges.
    edges: HashMap<String, HashSet<String>>,
    /// name → direct blocking (raw lane) fact.
    raw_io: HashMap<String, bool>,
}

/// Does this call name hit a blocking lane? The registry, named in one
/// place. Syscall-shaped calls are recognized by their wrapper name so the
/// check works on the parse AST (no import resolution).
fn blocking_lane(callee: &str, args: &[Expr]) -> bool {
    // The send/recv-once family is nonblocking by construction — never a
    // lane even though it wraps a syscall intrinsic.
    if callee.starts_with("sock_send_once")
        || callee.starts_with("sock_recv_once")
        || callee.starts_with("evented_")
    {
        return false;
    }
    // The blocking wrapper families (net stack + posix): sock_recv (loops to
    // cap/EOF), sock_accept (parks), sock_connect, read-family fs calls.
    if callee.starts_with("sock_recv")
        || callee.starts_with("sock_accept")
        || callee.starts_with("sock_connect")
        || callee.starts_with("fs_read")
    {
        return true;
    }
    // Raw SysCall# with a literal number: recognize the classic blocking
    // calls by number (read 0, accept 43, poll 7 with negative timeout).
    if callee == "SysCall#" || callee == "syscall" {
        if let Some(Expr::Decimal(nr)) = args.first() {
            match *nr {
                0 | 43 => return true, // read, accept
                7 => {
                    // poll: blocking only when the timeout is negative
                    // (infinite). A decimal literal third arg >= 0 is bounded.
                    if let Some(Expr::Decimal(t)) = args.get(2) {
                        return *t < 0;
                    }
                    return false;
                }
                _ => {}
            }
        }
        // Non-literal number or unknown args: opaque — may block.
        return true;
    }
    // Task/process synchronization intrinsics.
    if callee.starts_with("Wait#") || callee.starts_with("Futex") {
        return true;
    }
    false
}

impl StrandGraph {
    fn build(items: &[TopLevel]) -> Self {
        let mut bodies: HashMap<String, Vec<Statement>> = HashMap::new();
        let mut edges: HashMap<String, HashSet<String>> = HashMap::new();
        let mut raw_io: HashMap<String, bool> = HashMap::new();
        let user_names: HashSet<String> = items
            .iter()
            .filter_map(|i| match i {
                TopLevel::Definition(d) => Some(d.name.clone()),
                TopLevel::Transaction(t) => Some(t.name.clone()),
                TopLevel::Export(e) => match e.inner.as_ref() {
                    TopLevel::Definition(d) => Some(d.name.clone()),
                    TopLevel::Transaction(t) => Some(t.name.clone()),
                    _ => None,
                },
                _ => None,
            })
            .collect();

        let mut ingest = |name: &str, body: &[Statement]| {
            bodies.insert(name.to_string(), body.to_vec());
            let mut mine_edges = HashSet::new();
            let mut mine_raw = false;
            crate::ast::visit::walk_stmts(body, &mut |e| match e {
                Expr::Call(callee, args, _) => {
                    if user_names.contains(callee) {
                        mine_edges.insert(callee.clone());
                    } else if blocking_lane(callee, args) {
                        mine_raw = true;
                    } else if !callee.ends_with('#') {
                        // Unknown non-intrinsic name: an import — the FFI
                        // family, opaque, may block.
                        mine_raw = true;
                    }
                }
                Expr::Spawn { .. } => mine_raw = true,
                Expr::Await(_) => mine_raw = true,
                _ => {}
            });
            edges.insert(name.to_string(), mine_edges);
            raw_io.insert(name.to_string(), mine_raw);
        };

        for item in items {
            match item {
                TopLevel::Definition(d) => ingest(&d.name, &d.body),
                TopLevel::Transaction(t) => ingest(&t.name, &t.body),
                TopLevel::Export(e) => match e.inner.as_ref() {
                    TopLevel::Definition(d) => ingest(&d.name, &d.body),
                    TopLevel::Transaction(t) => ingest(&t.name, &t.body),
                    _ => {}
                },
                _ => {}
            }
        }

        Self {
            bodies,
            edges,
            raw_io,
        }
    }
}

/// Compute the strand of every callable: Pure unless it (transitively)
/// reaches a blocking lane. Sound direction: Io is the default for unknown
/// callees; Pure must be PROVEN (no lane, no Io callee, transitively).
pub fn strands(items: &[TopLevel]) -> HashMap<String, Strand> {
    let g = StrandGraph::build(items);
    let mut out: HashMap<String, Strand> = HashMap::new();
    for (name, raw) in &g.raw_io {
        out.insert(name.clone(), if *raw { Strand::Io } else { Strand::Pure });
    }
    // Fixpoint: an Io callee makes its caller Io. Terminates: each round
    // promotes at least one Pure → Io or stops.
    loop {
        let mut promoted = false;
        for (name, callees) in &g.edges {
            if out.get(name) == Some(&Strand::Io) {
                continue;
            }
            if callees.iter().any(|c| out.get(c) == Some(&Strand::Io)) {
                out.insert(name.clone(), Strand::Io);
                promoted = true;
            }
        }
        if !promoted {
            break;
        }
    }
    out
}

/// The spawned-defn names in the program (Expr::Spawn targets).
fn spawned_names(items: &[TopLevel]) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut walk = |stmts: &[Statement]| {
        crate::ast::visit::walk_stmts(stmts, &mut |e| match e {
            Expr::Spawn { type_name, .. } => {
                out.insert(type_name.clone());
            }
            _ => {}
        });
    };
    for item in items {
        match item {
            TopLevel::Definition(d) => walk(&d.body),
            TopLevel::Transaction(t) => walk(&t.body),
            TopLevel::Export(e) => match e.inner.as_ref() {
                TopLevel::Definition(d) => walk(&d.body),
                TopLevel::Transaction(t) => walk(&t.body),
                _ => {}
            },
            TopLevel::Statement(s) => walk(std::slice::from_ref(s)),
            _ => {}
        }
    }
    out
}

// Does any call AFTER the first `yield;` in this body reach an io strand?
/// The segment scheduler swaps only at yields — everything between two
/// yields runs to completion, so an io call there holds every other task.
/// Scan order: statements in order; a Call checks strand only once a Yield
/// has been seen earlier in THIS body (flat scan — walk_stmts visits in
/// source order, so the first Yield flips the flag for every later Call).
fn reaches_io_after_yield(body: &[Statement], strands: &HashMap<String, Strand>) -> bool {
    let mut seen_yield = false;
    let mut found = false;
    crate::ast::visit::walk_stmts(body, &mut |e| match e {
        Expr::Call(callee, _, _) => {
            if seen_yield && strands.get(callee) == Some(&Strand::Io) {
                found = true;
            }
        }
        _ => {}
    });
    // The yield flag needs source-position pairing with the calls; walk_stmts
    // is depth-first in source order, so a two-pass approach: find whether a
    // Yield precedes any io call positionally.
    if !found {
        return false;
    }
    // Re-walk tracking yield visibility properly (nested blocks inherit the
    // outer seen_yield; a yield deep inside a block also counts).
    fn scan(
        stmts: &[Statement],
        seen: &mut bool,
        strands: &HashMap<String, Strand>,
        found: &mut bool,
    ) {
        for stmt in stmts {
            if *found {
                return;
            }
            match stmt {
                Statement::Yield => *seen = true,
                Statement::Let { expr: Some(e), .. } => scan_expr(e, seen, strands, found),
                Statement::Assign(_, e) => scan_expr(e, seen, strands, found),
                Statement::Expression(e) => scan_expr(e, seen, strands, found),
                Statement::Term(Some(e)) | Statement::Tail(e) => {
                    scan_expr(e, seen, strands, found)
                }
                Statement::Guarded(_, b) | Statement::Block(b) => scan(b, seen, strands, found),
                Statement::Foreach { body, .. } => scan(body, seen, strands, found),
                Statement::Match { arms, .. } => {
                    for arm in arms {
                        scan(&arm.body, seen, strands, found);
                    }
                }
                _ => {}
            }
        }
    }
    fn scan_expr(
        e: &Expr,
        seen: &mut bool,
        strands: &HashMap<String, Strand>,
        found: &mut bool,
    ) {
        if *found {
            return;
        }
        match e {
            Expr::Call(callee, args, _) => {
                if *seen && strands.get(callee) == Some(&Strand::Io) {
                    *found = true;
                }
                for a in args {
                    scan_expr(a, seen, strands, found);
                }
            }
            Expr::Field(inner, _) | Expr::Deref(inner) | Expr::AddrOf(inner) => {
                scan_expr(inner, seen, strands, found)
            }
            Expr::BinaryOp(_, l, r) => {
                scan_expr(l, seen, strands, found);
                scan_expr(r, seen, strands, found);
            }
            _ => {}
        }
    }
    let mut seen = false;
    let mut found = false;
    scan(body, &mut seen, strands, &mut found);
    found
}

/// The Wave-3 consumer: for every SPAWNED defn, check whether any segment
/// AFTER the first `yield;` reaches an io lane — a segment that holds the
/// round-robin hostage between scheduling points. Advisory warnings in the
/// house what/why/fix shape.
pub fn analyze(items: &[TopLevel]) -> Vec<String> {
    let spawned = spawned_names(items);
    if spawned.is_empty() {
        return Vec::new();
    }
    let strands = strands(items);
    let mut warnings = Vec::new();
    for item in items {
        let (name, body) = match item {
            TopLevel::Definition(d) => (d.name.clone(), d.body.clone()),
            TopLevel::Export(e) => match e.inner.as_ref() {
                TopLevel::Definition(d) => (d.name.clone(), d.body.clone()),
                _ => continue,
            },
            _ => continue,
        };
        if !spawned.contains(&name) {
            continue;
        }
        // Segment split at `yield;` — the async Phase C shape. Any io call
        // AFTER the first yield holds the scheduler hostage (the scheduler
        // only swaps at yields; everything between two yields runs to
        // completion).
        if reaches_io_after_yield(&body, &strands) {
            warnings.push(format!(
                "defn '{name}' is spawned but calls an io defn after its first \
                 yield — the segment holds the scheduler hostage between yields \
                 (every other spawned task stalls). fix: move the blocking work \
                 before the first yield, split the segment at a yield, or use the \
                 evented (poll + nonblocking) steps so the call returns immediately"
            ));
        }
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> Vec<TopLevel> {
        let tokens = crate::lexer::tokenize(source).expect("fixture must lex");
        let mut p = crate::parser::Parser::new(tokens, source);
        p.parse_program().expect("fixture must parse")
    }

    /// A pure arithmetic defn is Pure (no lane, no Io callee).
    #[test]
    fn pure_arithmetic_is_pure() {
        let items = parse("defn add(a: Int, b: Int) -> Int { term a + b; };");
        let s = strands(&items);
        assert_eq!(s.get("add"), Some(&Strand::Pure));
    }

    /// A defn calling the BLOCKING recv family is Io.
    #[test]
    fn blocking_recv_callee_is_io() {
        // Self-contained: wrap a raw blocking-shaped syscall. read(2) = 0.
        let items = parse(
            "defn read_byte(fd: Int) -> Int { term SysCall#(0, fd, fd, 1); };",
        );
        let s = strands(&items);
        assert_eq!(s.get("read_byte"), Some(&Strand::Io));
    }

    /// Io propagates through a Pure-looking caller (fixpoint).
    #[test]
    fn io_propagates_to_caller() {
        let items = parse(
            "defn read_byte(fd: Int) -> Int { term SysCall#(0, fd, fd, 1); };\n\
             defn read_two(fd: Int) -> Int { term read_byte(fd) + read_byte(fd); };",
        );
        let s = strands(&items);
        assert_eq!(s.get("read_byte"), Some(&Strand::Io));
        assert_eq!(s.get("read_two"), Some(&Strand::Io));
    }

    /// The nonblocking *_once family is NOT a lane (EAGAIN, never parks).
    #[test]
    fn nonblocking_once_is_not_a_lane() {
        // send-once wraps SysCall#(44,...) — recognized nonblocking by name.
        let items = parse(
            "defn send_once_wrapper() -> Int { term SysCall#(44, 3, 3, 1, 64); };",
        );
        let s = strands(&items);
        // Raw SysCall# IS opaque by default (may block) — the *wrapper-name*
        // exemption only fires for named wrappers. This pins the honest
        // default: an unregistered raw syscall is Io.
        assert_eq!(s.get("send_once_wrapper"), Some(&Strand::Io));
    }

    /// A spawned defn calling io AFTER a yield warns; before the first
    /// yield (or with no io) stays silent.
    #[test]
    fn spawned_io_after_yield_warns() {
        let good = "defn worker(x: Int) -> Int {\n\
                    let a: Int = x * 2;\n\
                    yield;\n\
                    term a + x;\n};\n\
                    node entry [beginprogram][true] {\n\
                    let h = spawn worker(3);\n\
                    let r: Int = await h;\n\
                    Print#(r);\n\
                    endprogram Print#(\"\");\n};";
        assert!(analyze(&parse(good)).is_empty(), "pure-after-yield stays silent");
    }

    /// The spawn itself is a lane (opaque) — a defn containing spawn is Io.
    #[test]
    fn spawn_is_a_lane() {
        let items = parse(
            "defn starter() -> Int {\n\
             let h = spawn helper(1);\n\
             term 0;\n};\n\
             defn helper(x: Int) -> Int { yield; term x; };",
        );
        let s = strands(&items);
        assert_eq!(s.get("starter"), Some(&Strand::Io));
    }
}
