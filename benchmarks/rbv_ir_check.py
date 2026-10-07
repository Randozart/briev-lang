#!/usr/bin/env python3
"""rbv_ir_check.py — LLVM IR call/declare agreement check for the web gate.

Every direct `call <ret> @f(args)` must match @f's define/declare. A funcref
signature mismatch (e.g. `call i32 @navigate` against `declare void @navigate`)
is accepted by llc but lowers to an `unreachable` trap stub on wasm32 — only
a runtime probe or this check catches it.

Usage: python3 benchmarks/rbv_ir_check.py file.ll
Exit 1 + report on any mismatch.

2026-10-07 (plan 2026-10-07-web-surface-completion.md, W1): promoted from the
ad-hoc /tmp checker into the repo so the wasm32 signature fixes have a
mechanical guard alongside benchmarks/rbv_gate.mjs.
"""
import re
import sys

def extract_parens(text, start):
    """text[start] must be '('. Return inner content and index after ')'."""
    assert text[start] == '('
    depth = 0
    for i in range(start, len(text)):
        if text[i] == '(':
            depth += 1
        elif text[i] == ')':
            depth -= 1
            if depth == 0:
                return text[start + 1:i], i + 1
    return text[start + 1:], len(text)

def split_top(s):
    """Split on commas at paren/angle/bracket depth 0."""
    parts = []
    depth = 0
    cur = ''
    for ch in s:
        if ch in '(<[':
            depth += 1
        elif ch in ')>]':
            depth -= 1
        if ch == ',' and depth == 0:
            parts.append(cur)
            cur = ''
        else:
            cur += ch
    if cur.strip():
        parts.append(cur)
    return parts

def parse_defs(text):
    defs = {}
    # `define`/`declare [ret] @name(params)` — same signature extraction.
    for m in re.finditer(
        r'^\s*(define|declare)\b[^\n]*?\b(\S+)\s+@([\w.$-]+)\s*\(', text, re.M):
        kind, ret, name = m.groups()
        params, _ = extract_parens(text, m.end() - 1)
        ret = ret.strip()
        tys = []
        for p in split_top(params):
            p = p.strip()
            if not p:
                continue
            if p == '...':
                tys.append('...')
                continue
            toks = p.split()
            clean = []
            i = 0
            while i < len(toks):
                t = toks[i]
                if t.startswith('%'):
                    break
                if t in ('align',) and i + 1 < len(toks):
                    i += 2
                    continue
                if t.startswith(('byval', 'sret', 'dereferenceable', 'preallocated',
                                 'byref', 'elementtype', 'inalloca')):
                    i += 1
                    continue
                if t in ('noundef', 'noalias', 'nocapture', 'readonly', 'writeonly',
                         'readnone', 'returned', 'signext', 'zeroext', 'local_unnamed_addr',
                         'fast', 'nnan', 'ninf', 'nsz', 'arcp', 'contract', 'afn', 'reassoc',
                         'willreturn', 'memory', 'nounwind', 'uwtable'):
                    i += 1
                    continue
                clean.append(t)
                i += 1
            tys.append(' '.join(clean))
        defs[name] = (ret, tuple(tys))
    return defs

def parse_calls(text):
    calls = []
    for m in re.finditer(
        r'^\s*(?:%\S+\s*=\s*)?call\b[^\n]*?\b(\S+)\s+@([\w.$-]+)\s*\(',
        text, re.M):
        ret, name = m.groups()
        args, _ = extract_parens(text, m.end() - 1)
        tys = []
        for p in split_top(args):
            p = p.strip()
            if not p:
                continue
            if p == '...':
                tys.append('...')
                continue
            # call arg = [attrs] <type> <operand expr> — the TYPE is the
            # first non-attr token (compound types balance their brackets);
            # the operand may contain nested parens and is ignored.
            toks = p.split()
            i = 0
            attrs = {'noundef', 'noalias', 'nocapture', 'readonly', 'writeonly',
                     'readnone', 'signext', 'zeroext', 'returned', 'swiftself',
                     'swiftasync', 'immarg'}
            while i < len(toks):
                t = toks[i]
                if t in attrs:
                    i += 1
                    continue
                if t == 'align' and i + 1 < len(toks):
                    i += 2
                    continue
                if t.startswith(('byval', 'sret', 'dereferenceable', 'preallocated',
                                 'byref', 'elementtype', 'inalloca')):
                    i += 1
                    continue
                break
            if i >= len(toks):
                continue
            first = toks[i]
            if not first.startswith(('<', '{', '[')):
                tys.append(first)
                continue
            depth = 0
            out = []
            for t in toks[i:]:
                out.append(t)
                depth += sum(t.count(c) for c in '<{[') - sum(t.count(c) for c in '>}]')
                if depth <= 0:
                    break
            tys.append(' '.join(out))
        calls.append((m.start(), ret.strip(), name, tuple(tys)))
    return calls

def main(path):
    text = open(path).read()
    defs = parse_defs(text)
    calls = parse_calls(text)
    bad = []
    undefined = []
    line_starts = [0]
    for i, ch in enumerate(text):
        if ch == '\n':
            line_starts.append(i + 1)
    def lineno(pos):
        lo, hi = 0, len(line_starts) - 1
        while lo < hi:
            mid = (lo + hi + 1) // 2
            if line_starts[mid] <= pos:
                lo = mid
            else:
                hi = mid - 1
        return lo + 1
    for pos, ret, name, args in calls:
        if name.startswith('llvm.'):
            continue  # LLVM auto-declares intrinsics
        if name not in defs:
            undefined.append((lineno(pos), name, ret, args))
            continue
        dret, dargs = defs[name]
        norm = lambda s: ' '.join(s.split())
        a2 = tuple(norm(x) for x in args)
        d2 = tuple(norm(x) for x in dargs)
        if norm(ret) != norm(dret) or a2 != d2:
            bad.append((lineno(pos), name, ret, a2, dret, d2))
    if bad or undefined:
        print(f"MISMATCHES in {path}: {len(bad)} sig, {len(undefined)} undefined")
        for ln, name, ret, args, dret, dargs in bad:
            print(f"  L{ln}: call {ret} @{name}{args}  !=  declared {dret} @{name}{dargs}")
        for ln, name, ret, args in undefined:
            print(f"  L{ln}: call {ret} @{name}{args}  =>  UNDEFINED (no define/declare)")
        sys.exit(1)
    print(f"OK {path}: {len(calls)} calls checked, {len(defs)} local defines")

if __name__ == '__main__':
    main(sys.argv[1])
