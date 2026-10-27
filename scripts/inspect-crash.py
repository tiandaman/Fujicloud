"""Shows the code at a crash address from someone's log, in the exact build
they ran: "crashed: exception 0xc0000094 in ...\\fujicloud.exe at +0x1d059f8".

Release builds carry no symbols, so this finds the function around the
address (from the exception tables every x64 Windows program has),
disassembles it, and prints the text it and the functions it calls refer
to. ONNX Runtime's checks name their source file, which usually says
whose code it is.

    python3 scripts/inspect-crash.py <fujicloud.exe> <offset>
"""

import sys

import capstone
import pefile

exe, offset = sys.argv[1], int(sys.argv[2], 16)
pe = pefile.PE(exe, fast_load=True)
pe.parse_data_directories(
    directories=[pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_EXCEPTION"]]
)
base = pe.OPTIONAL_HEADER.ImageBase
functions = sorted(
    (e.struct.BeginAddress, e.struct.EndAddress)
    for e in getattr(pe, "DIRECTORY_ENTRY_EXCEPTION", [])
)
image = pe.get_memory_mapped_image()
md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
md.detail = True


def function_at(rva):
    for begin, end in functions:
        if begin <= rva < end:
            return begin, end
    return None


def text_at(rva, limit=200):
    """A C string at `rva` in the image, if it looks like one."""
    raw = image[rva : rva + limit]
    s = raw.split(b"\0", 1)[0]
    if len(s) >= 4 and all(32 <= c < 127 or c in (9, 10) for c in s):
        return s.decode()
    return None


def instructions(begin, end):
    return list(md.disasm(image[begin:end], base + begin))


def references(code):
    """Strings the code points at, and functions it calls."""
    texts, calls = [], []
    for ins in code:
        for op in ins.operands:
            if op.type == capstone.x86.X86_OP_MEM and op.mem.base == capstone.x86.X86_REG_RIP:
                target = ins.address + ins.size + op.mem.disp - base
                text = text_at(target)
                if text:
                    texts.append(text)
            if ins.mnemonic == "call" and op.type == capstone.x86.X86_OP_IMM:
                calls.append(op.imm - base)
    return texts, calls


found = function_at(offset)
print(f"image base {base:#x}, {len(functions)} functions")
if not found:
    sys.exit(f"{offset:#x} is in no known function")
begin, end = found
print(f"crash at +{offset:#x}, in the function at +{begin:#x}..+{end:#x} ({end - begin} bytes)\n")
code = instructions(begin, end)
at = next((i for i, ins in enumerate(code) if ins.address - base == offset), None)
window = code if len(code) <= 160 else code[max(0, (at or 0) - 60) : (at or 0) + 40]
for ins in window:
    mark = ">>" if ins.address - base == offset else "  "
    print(f"{mark} +{ins.address - base:#x}  {ins.mnemonic:8} {ins.op_str}")

texts, calls = references(code)
print("\ntext it refers to:")
for t in dict.fromkeys(texts):
    print("   ", t)
print("\ntext the functions it calls refer to:")
for target in dict.fromkeys(calls):
    callee = function_at(target)
    if not callee:
        continue
    t, _ = references(instructions(*callee))
    for text in dict.fromkeys(t):
        print(f"    +{target:#x}: {text}")

# Neighbours share a source file more often than not.
print("\ntext near it (the 40 functions either side):")
i = functions.index(found)
near = []
for b, e in functions[max(0, i - 40) : i + 41]:
    near += references(instructions(b, e))[0]
for t in dict.fromkeys(near):
    print("   ", t)
