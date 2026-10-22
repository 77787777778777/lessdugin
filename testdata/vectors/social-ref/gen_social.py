#!/usr/bin/env python3
# SPDX-License-Identifier: CC0-1.0
"""Generate the social fold vectors (social.md §19) from the reference model.

    python3 -I -B testdata/vectors/social-ref/gen_social.py            # (re)write every file
    python3 -I -B testdata/vectors/social-ref/gen_social.py --check    # exit 1 if a file differs
    python3 -I -B testdata/vectors/social-ref/gen_social.py --list     # files and vector counts

Every case module cases/<topic>_cases.py exposes FILES = [(path relative to testdata/vectors/,
thunk returning the file document)]. Output is deterministic: sorted keys, 2-space indent,
ASCII with \\uXXXX escapes for every non-ASCII code point, and a trailing newline. --check also
reports stale *.json files in the directories this generator owns that no case module
produces. Python 3 standard library only; the model lives in lessdugin_ref/ (see NOTES.md).
"""

from __future__ import annotations

import importlib
import json
import sys
from pathlib import Path

sys.dont_write_bytecode = True
HERE = Path(__file__).resolve().parent
VECTORS = HERE.parent
sys.path.insert(0, str(HERE))          # -I does not add the script directory

# Directories whose *.json files are all produced here (thread/ is shared with the Rust engine
# agent and is not swept for stale files).
OWNED_DIRS = ("time", "modfold", "karma", "reacts", "rank", "fold", "visibility")


JSON_SAFE = 2 ** 53 - 1      # every integer in a vector file is exact as a JavaScript number


def _check_ints(x, where: str) -> None:
    if isinstance(x, bool):
        return
    if isinstance(x, int):
        assert -JSON_SAFE <= x <= JSON_SAFE, f"{where}: integer {x} is not JSON-safe"
    elif isinstance(x, float):
        raise AssertionError(f"{where}: floats are never written to vectors (use hex bit patterns)")
    elif isinstance(x, dict):
        for k, v in x.items():
            _check_ints(v, f"{where}/{k}")
    elif isinstance(x, list):
        for i, v in enumerate(x):
            _check_ints(v, f"{where}/{i}")


def render(doc: dict) -> str:
    _check_ints(doc, "")
    return json.dumps(doc, ensure_ascii=True, indent=2, sort_keys=True) + "\n"


def case_modules() -> list:
    mods = []
    for p in sorted((HERE / "cases").glob("*_cases.py")):
        mods.append(importlib.import_module(f"cases.{p.stem}"))
    return mods


def selftest() -> None:
    from lessdugin_ref import dagcbor
    dagcbor.selftest(VECTORS.parent / "interop" / "data-model" / "data-model-fixtures.json")


def main(argv: list) -> int:
    check = "--check" in argv
    listing = "--list" in argv
    selftest()
    files = {}
    for mod in case_modules():
        for rel, thunk in mod.FILES:
            assert rel not in files, f"{rel} is produced twice"
            files[rel] = thunk
    status = 0
    for rel in sorted(files):
        doc = files[rel]()
        text = render(doc)
        path = VECTORS / rel
        if listing:
            print(f"{rel}: {len(doc['vectors'])} vectors, {len(text)} bytes")
            continue
        current = path.read_text(encoding="utf-8") if path.exists() else None
        if check:
            if current != text:
                print(f"{rel}: out of date", file=sys.stderr)
                status = 1
        elif current != text:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text, encoding="utf-8")
            print(f"wrote {rel} ({len(doc['vectors'])} vectors)")
        else:
            print(f"unchanged {rel} ({len(doc['vectors'])} vectors)")
    if check:
        for d in OWNED_DIRS:
            for p in sorted((VECTORS / d).glob("*.json")) if (VECTORS / d).is_dir() else []:
                rel = p.relative_to(VECTORS).as_posix()
                if rel not in files:
                    print(f"{rel}: not produced by any case module", file=sys.stderr)
                    status = 1
    return status


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
