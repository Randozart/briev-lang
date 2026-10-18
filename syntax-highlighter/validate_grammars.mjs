#!/usr/bin/env node
// syntax-highlighter/validate_grammars.mjs — the extension's regression gate.
// 2026-10-17 (plan 2026-10-17-syntax-highlighter-update.md §Validation).
//
// Checks, in order:
//  1. JSON-parse every grammar, both language configurations, package.json.
//  2. Compile every match/begin/end regex with JS RegExp (the grammars use
//     only constructs both Oniguruma and JS accept — no (?i:) groups; the
//     dbriev grammar folds case with char classes for exactly this reason).
//  3. Contribution consistency: every grammar path exists, no duplicate
//     scopeNames, every grammar contribution's language id exists, every
//     icon/configuration path exists.
//  4. Corpus smoke-matches: key constructs from real files hit their rules.
//  5. The beast tag-atom list matches src/beast/serialize.rs's atom() calls
//     (data, not a hand-maintained vocabulary — Rule 3).
//
// Exit 0 = all green. Any failure prints a line and exits 1.

import { readFileSync, existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "..");
let failures = 0;
const fail = (msg) => { failures++; console.error(`FAIL ${msg}`); };
const ok = (msg) => console.log(`ok   ${msg}`);

// ── 1. JSON parse ────────────────────────────────────────────────────────
const GRAMMARS = [
  "briev", "rbv", "ebv", "sbv", "abv", "bld", "bad", "dbriev", "beast",
];
const parsed = {};
for (const g of GRAMMARS) {
  const path = join(here, "syntaxes", `${g}.tmLanguage.json`);
  if (!existsSync(path)) { fail(`grammar missing: ${path}`); continue; }
  try {
    parsed[g] = JSON.parse(readFileSync(path, "utf8"));
  } catch (e) { fail(`grammar ${g}: JSON parse: ${e.message}`); }
}
const pkg = JSON.parse(readFileSync(join(here, "package.json"), "utf8"));
for (const c of ["language-configuration.json", "language-configuration-bld.json"]) {
  try { JSON.parse(readFileSync(join(here, c), "utf8")); }
  catch (e) { fail(`${c}: JSON parse: ${e.message}`); }
}
ok(`JSON parse (${GRAMMARS.length} grammars + 2 configs + package.json)`);

// ── 2. every regex compiles ──────────────────────────────────────────────
const regexFields = ["match", "begin", "end", "while"];
const walkRegexes = (node, path, fn) => {
  if (Array.isArray(node)) return node.forEach((n, i) => walkRegexes(n, `${path}[${i}]`, fn));
  if (node && typeof node === "object") {
    for (const [k, v] of Object.entries(node)) {
      if (regexFields.includes(k) && typeof v === "string") fn(v, `${path}.${k}`);
      else walkRegexes(v, `${path}.${k}`, fn);
    }
  }
};
let regexCount = 0;
for (const [name, g] of Object.entries(parsed)) {
  walkRegexes(g, name, (re, where) => {
    regexCount++;
    try { new RegExp(re); }
    catch (e) { fail(`${where}: bad regex /${re}/ — ${e.message}`); }
  });
}
ok(`regex compile (${regexCount} patterns)`);

// ── 3. contribution consistency ──────────────────────────────────────────
const con = pkg.contributes;
const langIds = new Set(con.languages.map((l) => l.id));
const scopes = new Set();
for (const g of con.grammars) {
  if (!langIds.has(g.language)) fail(`grammar language "${g.language}" not in languages list`);
  if (scopes.has(g.scopeName)) fail(`duplicate scopeName ${g.scopeName}`);
  scopes.add(g.scopeName);
  const path = join(here, g.path);
  if (!existsSync(path)) fail(`grammar path missing: ${g.path}`);
  const disk = parsed[g.path.replace("./syntaxes/", "").replace(".tmLanguage.json", "")];
  if (disk && disk.scopeName !== g.scopeName)
    fail(`${g.path}: scopeName mismatch disk=${disk.scopeName} manifest=${g.scopeName}`);
}
for (const l of con.languages) {
  for (const f of [l.icon?.light, l.icon?.dark, l.configuration]) {
    if (f && !existsSync(join(here, f))) fail(`${l.id}: missing file ${f}`);
  }
}
if (con.comments?.lineComment !== "//")
  fail(`package.json comments.lineComment is "${con.comments?.lineComment}", must be "//"`);
const lc = JSON.parse(readFileSync(join(here, "language-configuration.json"), "utf8"));
if (lc.comments?.lineComment !== "//")
  fail(`language-configuration.json lineComment is "${lc.comments?.lineComment}", must be "//"`);
ok(`contributions (${con.grammars.length} grammars, ${con.languages.length} languages)`);

// ── 4. corpus smoke-matches ──────────────────────────────────────────────
// POSITIVE: [grammar, text, scopeFragment, label] — the grammar must own a
// rule whose `name` contains scopeFragment AND whose regex matches the text
// (matching any rule would be vacuous — numbers match everything).
// NEGATIVE: [text, regex, label] — the regex (usually a lookahead expressing
// the expected ABSENCE) must match the text.
const positive = [
  // bad dialect — real files under examples/bad/, bad-dialect.md §Grammar
  ["bad", "raw x86_64 boot_name\n", "meta.block.raw.bad", "bad: raw head + target + name"],
  ["bad", "end raw\n", "keyword.control.block.bad", "bad: end raw terminator"],
  ["bad", "    site\n", "keyword.control.block.bad", "bad: site head"],
  ["bad", "    end site\n", "keyword.control.block.bad", "bad: end site"],
  ["bad", "    ptx => add.f32 r0, r1, r2\n", "entity.name.tag.target.bad", "bad: ptx target"],
  ["bad", "    x86_64 => lea r0, [r1 + 1]\n", "entity.name.tag.target.bad", "bad: target row"],
  ["bad", "    ^^ mov r0, r1\n", "keyword.other.acknowledge.bad", "bad: ack prefix"],
  ["bad", "    ^^ mov r0, r1\n", "keyword.other.mnemonic.bad", "bad: mnemonic after ack"],
  ["bad", "defn store_pair x, addr\n", "storage.type.bad", "bad: defn head"],
  ["bad", "_start: [r10 preserved]\n", "entity.name.function.label.bad", "bad: label + contract"],
  // bld — real file examples/bld/boot_protected_x86.bld, src/parser/bld.rs
  ["bld", "bootstrap Reset() {\n", "keyword.control.bld", "bld: bootstrap entry"],
  ["bld", "        bad {\n", "keyword.control.block.bld", "bld: bad-block head"],
  ["bld", "        loop {\n", "keyword.control.bld", "bld: loop (contextual)"],
  ["bld", "            when c > 68 {\n", "keyword.control.bld", "bld: when"],
  ["bld", "            break;\n", "keyword.control.bld", "bld: break"],
  ["bld", "        DisableInterrupts();\n", "support.function.engine.bld", "bld: engine verb"],
  ["bld", "    let vga: Ptr = 0xB8000;\n", "keyword.control.bld", "bld: let"],
  ["bld", "    c += 1;\n", "keyword.operator.compound-assign.bld", "bld: compound assign"],
  ["bld", "        return;\n", "keyword.control.bld", "bld: return (contextual)"],
  // briev core
  ["briev", "state let x: Int = 0;\n", "keyword.control.briev", "briev: state assertion"],
  ["briev", "plain let y: Int = 1;\n", "keyword.control.briev", "briev: plain assertion"],
  ["briev", "op Add(Int): int_add(#Lh, #Rh);\n", "hashword.briev", "briev: #Lh/#Rh exact"],
  ["briev", "frgn f() -> Int from #Link<mylib>;\n", "hashword.briev", "briev: #Link<name>"],
  ["briev", "x += 1;\n", "keyword.operator.compound-assign.briev", "briev: compound +="],
  ["briev", "let v = y <<= 2;\n", "keyword.operator.compound-assign.briev", "briev: <<= whole"],
  ["briev", "let a = b < c;\n", "keyword.operator.relational.briev", "briev: relational < intact"],
  ["briev", "let r = 330Ohm;\n", "constant.numeric.quantity.briev", "briev: 330Ohm quantity"],
  ["briev", "spec Capacitance: 100n;\n", "constant.numeric.quantity.briev", "briev: bare prefix 100n"],
  ["briev", "let r = 4k7;\n", "constant.numeric.quantity.briev", "briev: E-series 4k7"],
  ["briev", "let b: #Float = 1.0;\n", "invalid.illegal.retired-hashword.briev", "briev: retired #Float marked"],
  ["briev", "###\n", "keyword.control.module-fence.briev", "briev: module fence"],
  ["briev", 'let raw_bytes = #b"\\x00";\n', "string.quoted.double.byte.briev", "briev: byte string"],
  ["briev", "let x = halt;\n", "keyword.control.briev", "briev: halt keyword"],
  ["briev", "break;\n", "keyword.control.briev", "briev: break keyword"],
  ["briev", "acquire l;\n", "keyword.control.briev", "briev: acquire keyword"],
  ["briev", "let s = x + 1;\n", "keyword.operator.arithmetic.briev", "briev: binary + intact"],
  ["briev", "import \"std/web/console.bv\";\n", "keyword.control.briev", "briev: import"],
  // nested bad body in .bv — both parse_bad_fn entry shapes (definitions.rs)
  ["briev", "bad add(a: Int, b: Int) -> Int [result == a + b] {\n    Add r0, r5, r4\n}\n",
    "meta.declaration.bad-body.briev", "briev: bad fn head routes to #bad-body"],
  ["briev", "bootstrap bad Reset() [true] {\n    halt\n}\n",
    "meta.declaration.bad-body.briev", "briev: bootstrap bad head routes to #bad-body"],
  // dbriev — schema/table format (src/dbriev/v2.rs)
  ["dbriev", "schema Device {\n", "keyword.control.schema.dbriev", "dbriev: schema decl"],
  ["dbriev", ">schema Device from \"map.dbv\"\n", "keyword.control.directive.dbriev", "dbriev: >schema directive"],
  ["dbriev", "UART1: 0x40011000; 0x18;\n", "entity.name.constant.dbriev", "dbriev: table key"],
  ["dbriev", ".bv: llvm; --budget 256;\n", "entity.name.constant.dbriev", "dbriev: config row key"],
  ["dbriev", "import \"path\";\n", "keyword.control.dbriev", "dbriev: bare import"],
  // beast — sexpr format (src/beast/serialize.rs)
  ["beast", "( assign lhs rhs )", "support.function.tag.beast", "beast: tag atom"],
  ["beast", '("bound" :bound)', "keyword.other.attribute.beast", "beast: colon attribute"],
  ["beast", '("a" "b")', "string.quoted.double.beast", "beast: string"],
  ["beast", "(\"alignment\" 4)", "constant.numeric.integer.beast", "beast: bare number"],
  ["beast", '(term! e)', "support.function.tag.beast", "beast: term! before term"],
];
for (const [gname, text, scope, label] of positive) {
  const g = parsed[gname];
  if (!g) continue;
  const rules = [];
  walkRegexes(g, gname, (src, where) => {
    // find the nearest enclosing rule object with a `name` — walkRegexes
    // gives leaf regexes; resolve name via a second pass below.
    rules.push([src, where]);
  });
  // name-aware walk: collect {name, regexes[]} per rule
  // TextMate scopes come from the rule `name` OR any capture name
  // (bad/bld put most block scopes on captures — both count as owning).
  const ruleList = [];
  const collect = (node) => {
    if (Array.isArray(node)) return node.forEach(collect);
    if (node && typeof node === "object") {
      const regexes = [node.match, node.begin].filter(Boolean);
      const names = [node.name];
      for (const capKey of ["captures", "beginCaptures", "endCaptures"]) {
        for (const c of Object.values(node[capKey] || {})) {
          if (c && c.name) names.push(c.name);
        }
      }
      if (regexes.length && names.some(Boolean)) {
        ruleList.push({ names: names.filter(Boolean), regexes });
      }
      Object.values(node).forEach(collect);
    }
  };
  collect(g);
  const hit = ruleList.find(
    (r) => r.names.some((n) => n.includes(scope)) &&
      r.regexes.some((re) => { try { return new RegExp(re, "m").test(text); } catch { return false; } }),
  );
  if (!hit) fail(`smoke [${label}]: no rule named *${scope}* matches — ${gname}`);
}
const negatives = [
  // bare idents named state/plain must NOT hit the assertion rule
  ["briev", "let ident_state = 3;\n",
    /^(?!.*\b(?:state|plain)(?=\s+let\b))/, "briev: bare ident_state untouched"],
  // bootstrap node is not a bad-body head
  ["briev", "bootstrap node start [true] {\n    term;\n}\n",
    /^(?!.*bootstrap\s+bad)/, "briev: bootstrap node NOT bootstrap-bad"],
  // #Int must NOT hit the valid hashword rule (it is retired → invalid rule)
  ["briev", "let t: #Int = 1;\n",
    /#(Float|Int|UInt|String|Bool|Char|Blob|Bit|Data|Bits)\b/,
    "briev: #Int routes to retired rule, not hashword rule"],
];
for (const [gname, text, re, label] of negatives) {
  if (!re.test(text)) fail(`negative [${label}]`);
}
// rbv: the <script> wrapper rule must be gone from the repository
if (parsed.rbv && ("script-tag" in parsed.rbv.repository || "script-tag" in (parsed.rbv.patterns?.[0] || {})))
  fail("rbv: script-tag rule still present (SPEC 21.1 rejects script wrappers)");
if (parsed.rbv && !parsed.rbv.patterns.some((x) => x.include === "source.briev"))
  fail("rbv: source.briev include missing — Briev-as-default must color");
ok(`corpus smoke (${positive.length} positive, ${negatives.length + 1} negative)`);

// ── 5. beast atoms stay in lockstep with the serializer ──────────────────
const serPath = join(root, "src", "beast", "serialize.rs");
if (existsSync(serPath)) {
  const ser = readFileSync(serPath, "utf8");
  const atoms = new Set(
    [...ser.matchAll(/atom\("(:?[a-z_!:]+)"\)/g)].map((m) => m[1]),
  );
  const tagRule = parsed.beast?.repository?.["tag-atoms"]?.patterns?.[0]?.match;
  const colonRule = parsed.beast?.repository?.["colon-attributes"]?.patterns?.[0]?.match;
  // colon atoms must be in the colon rule; bare atoms in the tag rule
  const bare = [...atoms].filter((a) => !a.startsWith(":"));
  const colon = [...atoms].filter((a) => a.startsWith(":")).map((a) => a.slice(1));
  // Pull the alternation out of the rule's parenthesised group:
  //   \b(a|b|c)\b → ["a","b","c"]   and   :(a|b)\b → ["a","b"]
  const altsOf = (match) => {
    const inner = match.match(/\(([^()]+)\)/);
    if (!inner) fail(`atom-diff: no alternation group in /${match}/`);
    return (inner ? inner[1] : "").split("|").map((x) => x.replace(/\?:/g, ""));
  };
  const bareAlts = altsOf(tagRule || "");
  const colonAlts = altsOf(colonRule || "");
  for (const a of bare) if (!bareAlts.includes(a)) fail(`beast: atom "${a}" in serialize.rs but not in tag-atoms rule`);
  for (const a of colon) if (!colonAlts.includes(a)) fail(`beast: atom ":${a}" in serialize.rs but not in colon-attributes rule`);
  // drift runs both ways — a dead tag the serializer stopped emitting is
  // vocabulary the grammar will highlight forever
  const serText = ser;
  for (const a of bareAlts) {
    if (!serText.includes(`atom("${a}")`)) fail(`beast: tag "${a}" in grammar but serialize.rs never emits it`);
  }
  ok(`beast atoms vs serialize.rs (${atoms.size} atoms)`);
} else {
  console.log("skip beast atom diff (src/beast/serialize.rs not found)");
}

// ── verdict ──────────────────────────────────────────────────────────────
if (failures > 0) {
  console.error(`\n${failures} failure(s)`);
  process.exit(1);
}
console.log("\nvalidate_grammars: OK");
