import re, sys, glob
src, out = sys.argv[1], sys.argv[2]
text = open(src, encoding='utf-8', errors='replace').read()
pat = re.compile(r'@briev_kernel_(\w+) = private constant \[(\d+) x i8\] c"((?:[^"\\]|\\.)*)"', re.S)
found = 0
for name, n, body in pat.findall(text):
    if name.endswith('_ocl'):
        data = bytearray()
        i = 0
        while i < len(body):
            if body[i] == '\\':
                data.append(int(body[i+1:i+3], 16)); i += 3
            else:
                data.append(ord(body[i])); i += 1
        if len(data) > 20:
            open(f"{out}/k_{name}.spv", "wb").write(data)
            print(name, len(data))
            found += 1
print("extracted", found)
