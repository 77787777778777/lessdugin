// SPDX-License-Identifier: AGPL-3.0-or-later
// CIDs written in records compare by canonical form (social.md §0.4, §20.1 item 45), as
// crates/core social/full/cids.rs `same`. The spellings below are one dag-cbor sha2-256 CIDv1
// written in each multibase, computed with the multiformats 13.4.2 JavaScript library.
import { describe, expect, it } from "vitest";
import { canonicalCid, sameCid } from "./cid.ts";

const CID = "bafyreidxu3yzmtdaxrwfoy5aat26xhjjnqqovbasr6c2ppkxhe2s4mlnuy";
const OTHER = "bafyreifzvthwdovtvvxdaxve22ggopxkbajqydipahdx4nk52xo6ivoyyu";
const SPELLINGS = {
  base16: "f0171122077a6f1964c60bc6c5763a004f5eb9d296c20ea84128f85a7bd5739352e316da6",
  base16upper: "F0171122077A6F1964C60BC6C5763A004F5EB9D296C20EA84128F85A7BD5739352E316DA6",
  base32upper: "BAFYREIDXU3YZMTDAXRWFOY5AAT26XHJJNQQOVBASR6C2PPKXHE2S4MLNUY",
  base32pad: "cafyreidxu3yzmtdaxrwfoy5aat26xhjjnqqovbasr6c2ppkxhe2s4mlnuy======",
  base32padupper: "CAFYREIDXU3YZMTDAXRWFOY5AAT26XHJJNQQOVBASR6C2PPKXHE2S4MLNUY======",
  base36: "k2jvsl8df6pgo6ga0fuu0zfunbvstdhbe70wgv9510iczjy0p1usziqu",
  base36upper: "K2JVSL8DF6PGO6GA0FUU0ZFUNBVSTDHBE70WGV9510ICZJY0P1USZIQU",
  base58btc: "zdpuAtUWeK5yk4sUmYZPr3dHAdxomre84zdDURT8MMJYvyB3T",
  base64: "mAXESIHem8ZZMYLxsV2OgBPXrnSlsIOqEEo+Fp71XOTUuMW2m",
  base64url: "uAXESIHem8ZZMYLxsV2OgBPXrnSlsIOqEEo-Fp71XOTUuMW2m",
};

describe("canonicalCid", () => {
  it("reads a CIDv1 in each multibase as its base32 lowercase form", () => {
    expect(canonicalCid(CID)).toBe(CID);
    for (const [base, s] of Object.entries(SPELLINGS)) {
      expect([base, canonicalCid(s)]).toEqual([base, CID]);
    }
  });

  it("names no version for a CIDv0, a damaged string or an unread multibase", () => {
    // the same sha2-256 digest as a CIDv0
    expect(canonicalCid("QmWPisVjFYSDCbL6n5i79N9v3Gc1n1PYo2QEpFVn917x8d")).toBeNull();
    // a symbol outside the alphabet, a truncated digest, a case mixed into strict base32
    expect(canonicalCid(`${CID.slice(0, -1)}1`)).toBeNull();
    expect(canonicalCid(CID.slice(0, -8))).toBeNull();
    expect(canonicalCid(`b${CID.slice(1).toUpperCase()}`)).toBeNull();
    // base32 padding: required by `c`, refused by `b`
    expect(canonicalCid(SPELLINGS.base32pad.replace(/=+$/, ""))).toBeNull();
    expect(canonicalCid(`${CID}======`)).toBeNull();
    // the base32 encoding of a different last symbol carries nonzero trailing bits
    expect(canonicalCid(`${CID.slice(0, -1)}z`)).toBeNull();
    expect(canonicalCid("")).toBeNull();
    expect(canonicalCid("x123")).toBeNull();
  });
});

describe("sameCid", () => {
  it("compares by canonical form, so another spelling names the same version", () => {
    expect(sameCid(CID, CID)).toBe(true);
    expect(sameCid(SPELLINGS.base32upper, CID)).toBe(true);
    expect(sameCid(SPELLINGS.base58btc, CID)).toBe(true);
    expect(sameCid(OTHER, CID)).toBe(false);
    expect(sameCid(SPELLINGS.base32upper, OTHER)).toBe(false);
  });

  it("matches a string that is no CID only by equality, and nothing when absent", () => {
    expect(sameCid("not-a-cid", "not-a-cid")).toBe(true);
    expect(sameCid("not-a-cid", CID)).toBe(false);
    expect(sameCid(undefined, CID)).toBe(false);
  });
});
