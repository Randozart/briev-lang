#!/usr/bin/env python3
"""Mechanical intrinsic × lane audit (three-surfaces plan Phase 0.3).

For every registry intrinsic, emit a minimal probe on .bv and .abv and
compile it. Build success = the lane's lowering chain accepted it; failure
captures the diagnostic for classification (expected surface-own vs gap).
"""
import re, subprocess, os, sys, json, concurrent.futures as cf

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
BRIEVC = os.path.join(REPO, "target/release", "brievc")
WORK = os.environ.get("BRIEV_PROBE_WORK", "/tmp/briev-intrinsic-probe")

reg = open(os.path.join(REPO, "src/intrinsic_signatures.rs")).read()
arms = re.findall(
    r'"([A-Za-z][A-Za-z0-9_]*#)"\s*=>\s*Some\(Signature \{(.*?)\}\),', reg, re.S
)

def param_types(body):
    m = re.search(r"parameters: vec!\[(.*?)\],\s*return_kind", body, re.S)
    if not m:
        return []
    return re.findall(r"Type::([\w]+)(?:\(([^)]*)\))?", m.group(1))

def return_kind(body):
    m = re.search(r"return_kind: ReturnKind::(\w+)(?:\(([^)]*)\))?", body)
    kind = m.group(1) if m else "?"
    arg = m.group(2) if m and m.group(2) else ""
    arg = arg.strip('"')
    return kind, arg

def arg_for(t, inner):
    if t == "string":
        return '"s"'
    if t == "float":
        return "1.5"
    if t == "int":
        return "7"
    if t == "bool_":
        return "true"
    if t == "ptr":
        return None  # special-cased
    return None

specs = []
for name, body in arms:
    pts = param_types(body)
    kind, rarg = return_kind(body)
    variadic = "variadic: true" in body
    specs.append((name, pts, kind, rarg, variadic))

# ptr-param special case: only Free# takes a ptr — chain from Malloc#.
def probe_src(name, pts, kind, rarg, surface):
    kw = "async node" if surface == "abv" else "node"
    lines = ["let i: Int = 0;", "", kw + " Main [i < 1][i == 1] {"]
    args = []
    setup = []
    for i, (t, inner) in enumerate(pts):
        if t == "ptr":
            setup.append("    let _p%d = Malloc#(4);" % i)
            args.append("_p%d" % i)
        else:
            a = arg_for(t, inner)
            if a is None:
                return None  # unmapped param type — flag manually
            args.append(a)
    lines += setup
    call = "%s(%s)" % (name, ", ".join(args))
    if name == "Error#":
        return None  # Never by design — expected-fail, classified separately
    if name == "Asm#":
        call = "Asm#()"  # variadic, zero declared params
    if kind == "Exact" and "void" in rarg:
        lines.append("    %s;" % call)
    else:
        ann = ""
        if kind == "Native":
            ann = ": " + rarg
        elif kind == "Exact" and "void" not in rarg:
            if "int()" in rarg: ann = ": Int"
            elif "float()" in rarg: ann = ": Float"
            elif "bool_" in rarg: ann = ": Bool"
            elif "string()" in rarg: ann = ": String"
        lines.append("    let _v%s = %s;" % (ann, call))
    lines.append("    i = i + 1;")
    lines.append("    term;")
    lines.append("};")
    return "\n".join(lines) + "\n"

def build(surface, name, src):
    d = os.path.join(WORK, "probes", surface, name[:-1])
    os.makedirs(d, exist_ok=True)
    link = os.path.join(d, "lib")
    if not os.path.lexists(link):
        try:
            os.symlink(os.path.join(REPO, "lib"), link)
        except FileExistsError:
            pass
    path = os.path.join(d, "probe." + surface)
    open(path, "w").write(src)
    out = os.path.join(d, "out")
    os.makedirs(out, exist_ok=True)
    p = subprocess.run(
        [BRIEVC, "build", path, "--out", out],
        capture_output=True, text=True, cwd=REPO, timeout=120,
    )
    msg = "\n".join((p.stderr or p.stdout).strip().splitlines()[:12])
    return name, surface, p.returncode, msg

results = {}
jobs = []
skipped = []
with cf.ThreadPoolExecutor(max_workers=8) as ex:
    for name, pts, kind, rarg, variadic in specs:
        for surface in ("bv", "abv"):
            src = probe_src(name, pts, kind, rarg, surface)
            if src is None:
                skipped.append((name, surface))
                continue
            jobs.append(ex.submit(build, surface, name, src))
    for j in cf.as_completed(jobs):
        name, surface, rc, msg = j.result()
        results.setdefault(name, {})[surface] = (rc, msg)

fails = []
for name in sorted(results):
    row = results[name]
    bv = row.get("bv", (-1, "SKIPPED"))
    abv = row.get("abv", (-1, "SKIPPED"))
    if bv[0] != 0 or abv[0] != 0:
        fails.append((name, bv, abv))

print(f"probed: {len(results)} names x2 surfaces; skipped: {skipped}")
print(f"FAIL rows: {len(fails)}")
for name, bv, abv in fails:
    print(f"--- {name}")
    if bv[0] != 0:
        print(f"    bv  : {bv[1][:220]}")
    if abv[0] != 0:
        print(f"    abv : {abv[1][:220]}")
json.dump(
    {n: {s: [rc, msg] for s, (rc, msg) in r.items()} for n, r in results.items()},
    open(os.path.join(WORK, "probe_results.json"), "w"), indent=1,
)
print("saved probe_results.json")
