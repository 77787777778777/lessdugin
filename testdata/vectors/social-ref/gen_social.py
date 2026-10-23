#!/usr/bin/env python3
# SPDX-License-Identifier: CC0-1.0
"""Generate the social fold vectors (social.md §19) from the reference model.

    python3 -I -B testdata/vectors/social-ref/gen_social.py                 # (re)write every file
    python3 -I -B testdata/vectors/social-ref/gen_social.py --check         # exit 1 if a file differs
    python3 -I -B testdata/vectors/social-ref/gen_social.py --list          # files and vector counts
    python3 -I -B testdata/vectors/social-ref/gen_social.py --emit-shared DIR
        # also write the model's own cases for the shared directories (none at present) under DIR

Every case module cases/<topic>_cases.py exposes FILES = [(path relative to testdata/vectors/,
thunk returning the file document)], and may expose SHARED in the same form for a directory
this generator does not own. Output is deterministic: sorted keys, 2-space indent, ASCII with
\\uXXXX escapes for every non-ASCII code point, and a trailing newline.

--check also reports stale *.json files in the directories this generator owns that no case
module produces.

Shared directories (SHARED_DIRS; none at present, thread/ was one until M3) are read-only here:
they may be written by another producer. Every mode cross-checks every vector of every *.json file there: it evaluates the
vector's input with the model and compares each listed `expected` field with the model's
projection. A disagreement fails the run; a field or input the model does not model is reported
and skipped. The model's own cases for those directories are built (and self-checked) on every
run but written only with --emit-shared DIR. Python 3 standard library only; the model lives in
lessdugin_ref/ (see NOTES.md).
"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True           # before any local import (lessdugin_ref/__init__.py)

import importlib  # noqa: E402
import json  # noqa: E402
from pathlib import Path  # noqa: E402

HERE = Path(__file__).resolve().parent
VECTORS = HERE.parent
sys.path.insert(0, str(HERE))          # -I does not add the script directory

# Directories whose *.json files are all produced here. thread/ was shared with the Rust engine
# agent until M3; its files were always the model's own cases, and the generator owns it now.
OWNED_DIRS = ("time", "modfold", "karma", "reacts", "rank", "fold", "visibility", "thread")
# Directories this generator never writes; their vectors are cross-checked against the model.
# None at present; the mechanism stays for a directory another producer may share.
SHARED_DIRS = ()


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
    from lessdugin_ref import cids, dagcbor
    dagcbor.selftest(VECTORS.parent / "interop" / "data-model" / "data-model-fixtures.json")
    cids.selftest()


def cross_check(path: Path) -> tuple:
    """Evaluate every vector of a shared vector file with the model.

    Returns (disagreements, skipped, checked): lists of messages, and the number of fields
    compared. Only kinds the model projects are evaluated.
    """
    from lessdugin_ref.common import NS, InputError, NotModelled
    from lessdugin_ref.fold import fold_vector_input
    from lessdugin_ref.projections import DEFAULT_FIELDS, FIELD_KINDS, project

    rel = path.relative_to(VECTORS).as_posix() if path.is_relative_to(VECTORS) else str(path)
    bad, skipped, checked = [], [], 0
    try:
        doc = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as e:
        return [f"{rel}: unreadable ({e})"], [], 0
    kind = doc.get("kind")
    if kind not in DEFAULT_FIELDS:
        return [], [f"{rel}: kind {kind!r} is not a fold kind of the model; not cross-checked"], 0
    if doc.get("ns", NS) != NS:
        return [], [f"{rel}: ns {doc.get('ns')!r} differs from the model's {NS}; not cross-checked"], 0
    for vec in doc.get("vectors", []):
        name = f"{rel} :: {vec.get('name')}"
        try:
            view = fold_vector_input(vec["input"])
        except NotModelled as e:
            skipped.append(f"{name}: input not modelled ({e})")
            continue
        except (InputError, KeyError, TypeError, ValueError) as e:
            bad.append(f"{name}: the model rejects the input ({e!r})")
            continue
        for fname, want in sorted(vec.get("expected", {}).items()):
            if FIELD_KINDS.get(fname) not in ("*", kind):
                skipped.append(f"{name}: field {fname} is not a {kind} field of the model")
                continue
            try:
                got = project(view, kind, [fname])[fname]
            except NotModelled as e:
                skipped.append(f"{name}: field {fname} not modelled ({e})")
                continue
            checked += 1
            if got != want:
                bad.append(f"{name}: field {fname} differs\n    file:  {json.dumps(want)[:400]}\n"
                           f"    model: {json.dumps(got)[:400]}")
    return bad, skipped, checked


def main(argv: list) -> int:
    check = "--check" in argv
    listing = "--list" in argv
    emit_dir = None
    if "--emit-shared" in argv:
        i = argv.index("--emit-shared")
        if i + 1 >= len(argv):
            print("--emit-shared needs a directory", file=sys.stderr)
            return 2
        emit_dir = Path(argv[i + 1]).resolve()
    selftest()
    files, shared = {}, {}
    for mod in case_modules():
        for table, src in ((files, getattr(mod, "FILES", [])), (shared, getattr(mod, "SHARED", []))):
            for rel, thunk in src:
                assert rel not in files and rel not in shared, f"{rel} is produced twice"
                assert (rel.split("/")[0] in SHARED_DIRS) == (table is shared), \
                    f"{rel}: shared directories are listed in SHARED, owned ones in FILES"
                table[rel] = thunk
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

    # The model's own cases for the shared directories: always built (their hand expectations
    # are asserted), compared with the shared files for information, written only on request.
    for rel in sorted(shared):
        doc = shared[rel]()
        text = render(doc)
        path = VECTORS / rel
        current = path.read_text(encoding="utf-8") if path.exists() else None
        same = "identical to" if current == text else "absent from" if current is None else "differs from"
        if listing:
            print(f"{rel}: {len(doc['vectors'])} vectors, {len(text)} bytes (model's own cases; "
                  f"{same} the shared file)")
        elif not check:
            print(f"model cases for {rel}: {len(doc['vectors'])} vectors, {same} the shared file")
        if emit_dir is not None:
            out = emit_dir / rel
            out.parent.mkdir(parents=True, exist_ok=True)
            out.write_text(text, encoding="utf-8")
            print(f"emitted {out}")

    if not listing:
        for d in SHARED_DIRS:
            paths = sorted((VECTORS / d).glob("*.json")) if (VECTORS / d).is_dir() else []
            for p in paths:
                bad, skipped, checked = cross_check(p)
                rel = p.relative_to(VECTORS).as_posix()
                for msg in skipped:
                    print(f"cross-check skipped: {msg}", file=sys.stderr)
                for msg in bad:
                    print(f"cross-check DISAGREES: {msg}", file=sys.stderr)
                if bad:
                    status = 1
                elif not check:
                    print(f"cross-checked {rel}: {checked} fields agree with the model")

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
