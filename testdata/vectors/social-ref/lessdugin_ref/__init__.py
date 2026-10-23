# SPDX-License-Identifier: CC0-1.0
"""lessdugin_ref: an independent reference model of the lessdugin social fold.

Written only from docs/spec/social.md, docs/spec/data-model.md (§3 keys, §6 records, §7
validity) and the lexicons, to generate the normative vectors in testdata/vectors/. Python 3
standard library only; run with `python3 -I`. Ambiguities and the readings chosen are listed
in ../NOTES.md.

Modules (bottom-up; "stub" = exact only where its subject matter is absent, else NotModelled):
    common, syntax, dagcbor   primitives (keys come from testdata/vectors/keys/gen_keys.py)
    input                     FoldInput parsing (social.md §1.1)
    policy                    effective policy (§1.6)
    validity                  data-model.md §7
    digest                    inputDigest (§1.2)
    time                      §2
    authorities               L0 (§4.1)
    modfold                   L1 (§4.2–§4.8, §4.10–§4.12)
    participation             L2 (§3)
    opinions                  L3 opinion validity (§5)
    karma                     L3 vote power, karma/1 (§6)
    scores                    L3.5 (§7)
    gates                     L4 (§8); registers the gateValues and passes fields
    authormod                 L5 author tier (§4.9)
    reacts                    L5 (§9.2), qkey (lfm-1.md §11)
    tags                      L5 (§13): wiki heads, relevance, applies
    sequences                 L5 (§14)                     stub
    threads                   §11 placement, steps 1–4
    versions                  §11 step 6 versions ("on revision n") and the edited badge
    cids                      canonical CID form (§0.4): CIDs written in records compare by it
    visibility                L6 V1–V10 and the thread pass (§10.1–§10.2); L7 lens (§10.3)
    rank                      L6 ranking, sorts, filters, Recent Discussion (§12)
    fold                      orchestration: ForumView (see its docstring for the layer table)
    projections               vector projection fields (one global namespace)
    build                     builders for vector inputs (valid records, canonical keys, real CIDs)
"""

import sys

# Keep the repository free of __pycache__: this stops the submodules' bytecode, but Python
# writes this file's own .pyc before running it. gen_social.py sets the flag before its first
# import; any other importer must run with -B (`python3 -I -B`) or set
# sys.dont_write_bytecode = True itself before importing lessdugin_ref.
sys.dont_write_bytecode = True
