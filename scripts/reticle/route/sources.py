#!/usr/bin/env python3
"""Lists the Verilog files a design needs, from reticle's IP library and the design's own
directory, since `reticle fpga` has no search path.

    sources.py <reticle checkout> <top file> [more files...]

Prints the files, relative to the checkout, the given ones first. A module is looked for by
its `module NAME` line; a name nothing declares is left for the device's cell library.
"""

import os
import re
import sys

DECL = re.compile(r"^\s*module\s+([A-Za-z_][A-Za-z0-9_$]*)", re.M)
# An instance: `name #(` or `name inst (` at the start of a statement.
INST = re.compile(r"^\s*([A-Za-z_][A-Za-z0-9_$]*)\s*(?:#\s*\(|[A-Za-z_][A-Za-z0-9_$]*\s*\()", re.M)
KEYWORDS = {"module", "if", "for", "case", "else", "assign", "always", "initial", "function",
            "task", "wire", "reg", "input", "output", "inout", "begin", "end", "while", "repeat",
            "generate", "localparam", "parameter", "integer", "genvar", "return"}


def strip(text):
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.S)
    return re.sub(r"//[^\n]*", "", text)


def main():
    root, files = sys.argv[1], sys.argv[2:]
    index = {}
    dirs = [os.path.join(root, "ip"), os.path.dirname(os.path.join(root, files[0]))]
    for base in dirs:
        for dirpath, _, names in os.walk(base):
            for name in sorted(names):
                if not name.endswith(".v") or name.endswith("_tb.v"):
                    continue
                path = os.path.join(dirpath, name)
                with open(path) as f:
                    for module in DECL.findall(strip(f.read())):
                        index.setdefault(module, os.path.relpath(path, root))
    out = list(files)
    at = 0
    while at < len(out):
        with open(os.path.join(root, out[at])) as f:
            text = strip(f.read())
        declared = set(DECL.findall(text))
        for name in INST.findall(text):
            if name in KEYWORDS or name in declared:
                continue
            path = index.get(name)
            if path and path not in out:
                out.append(path)
        at += 1
    print(" ".join(out))


main()
