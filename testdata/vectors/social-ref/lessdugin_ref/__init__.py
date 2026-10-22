# SPDX-License-Identifier: CC0-1.0
"""lessdugin_ref: an independent reference model of the lessdugin social fold.

Written only from docs/spec/social.md, docs/spec/data-model.md (§3 keys, §6 records, §7
validity) and the lexicons, to generate the normative vectors in testdata/vectors/. Python 3
standard library only; run with `python3 -I`. Ambiguities and the readings chosen are listed
in ../NOTES.md.

Modules:
    common, syntax, dagcbor   primitives (keys come from testdata/vectors/keys/gen_keys.py)
    input                     FoldInput parsing (social.md §1.1)
    policy                    effective policy (§1.6)
    validity                  data-model.md §7
    digest                    inputDigest (§1.2)
    time                      §2
    authorities               L0 (§4.1)
    modfold                   L1 (§4.2–§4.8, §4.10–§4.12)
    participation             L2 (§3)
    threads                   §11 placement
    visibility                V1–V10 and the thread pass (§10.1–§10.2)
    karma, scores             L3, L3.5            (stubs)
    gates                     L4
    authormod, l5_stubs       L5                  (stubs)
    rank                      L6 ranking          (stub)
    fold                      orchestration: ForumView
    projections               vector projections per kind
    build                     builders for vector inputs
"""

import sys

sys.dont_write_bytecode = True
