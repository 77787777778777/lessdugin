// SPDX-License-Identifier: AGPL-3.0-or-later
// CID comparison (social.md §0.4, §20.1 item 45), as crates/core social/full/cids.rs `same`: a
// CID written in a record names a version when it is the same string, or when both are CIDv1s
// with the same canonical form (base32 lowercase, multibase prefix `b`). G2's `cid` format
// accepts other multibases and letter cases. The multibases decoded here are base16, base32
// (with or without padding), base36, base58btc, base64 and base64url, in both letter cases; a
// string in another multibase, or one that is not a CIDv1, equals only itself.

const B16 = "0123456789abcdef";
const B32 = "abcdefghijklmnopqrstuvwxyz234567";
const B36 = "0123456789abcdefghijklmnopqrstuvwxyz";
const B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const B64URL = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/** RFC 4648 decoding with `bits` per symbol: strict alphabet, `=` padding to whole blocks
 * exactly when `pad`, no partial symbol and zero trailing bits. */
function decodeBits(s: string, alphabet: string, bits: number, pad: boolean): Uint8Array | null {
  let body = s;
  if (pad) {
    const block = bits === 5 ? 8 : 4;
    body = s.replace(/=+$/, "");
    if (
      s.length % block !== 0 ||
      body.length + ((block - (body.length % block)) % block) !== s.length
    ) {
      return null;
    }
  }
  const out: number[] = [];
  let acc = 0;
  let n = 0;
  for (const ch of body) {
    const v = alphabet.indexOf(ch);
    if (v < 0) return null;
    acc = (acc << bits) | v;
    n += bits;
    if (n >= 8) {
      n -= 8;
      out.push((acc >> n) & 0xff);
    }
    acc &= (1 << n) - 1;
  }
  return n >= bits || acc !== 0 ? null : Uint8Array.from(out);
}

/** Base-x decoding (base36, base58): a big number, each leading zero symbol a zero byte. */
function decodeBaseX(s: string, alphabet: string): Uint8Array | null {
  const bytes: number[] = [];
  for (const ch of s) {
    let carry = alphabet.indexOf(ch);
    if (carry < 0) return null;
    for (let i = 0; i < bytes.length; i++) {
      carry += (bytes[i] as number) * alphabet.length;
      bytes[i] = carry & 0xff;
      carry >>= 8;
    }
    for (; carry > 0; carry >>= 8) bytes.push(carry & 0xff);
  }
  for (const ch of s) {
    if (ch !== alphabet[0]) break;
    bytes.push(0);
  }
  return Uint8Array.from(bytes.reverse());
}

/** The bytes of a multibase string, for the multibases this module reads. */
function multibase(s: string): Uint8Array | null {
  const rest = s.slice(1);
  switch (s[0]) {
    case "f":
      return decodeBits(rest, B16, 4, false);
    case "F":
      return decodeBits(rest, B16.toUpperCase(), 4, false);
    case "b":
      return decodeBits(rest, B32, 5, false);
    case "B":
      return decodeBits(rest, B32.toUpperCase(), 5, false);
    case "c":
      return decodeBits(rest, B32, 5, true);
    case "C":
      return decodeBits(rest, B32.toUpperCase(), 5, true);
    case "k":
    case "K":
      return decodeBaseX(rest.toLowerCase(), B36);
    case "z":
      return decodeBaseX(rest, B58);
    case "m":
      return decodeBits(rest, B64, 6, false);
    case "M":
      return decodeBits(rest, B64, 6, true);
    case "u":
      return decodeBits(rest, B64URL, 6, false);
    case "U":
      return decodeBits(rest, B64URL, 6, true);
    default:
      return null;
  }
}

/** An unsigned varint at `at` (minimal, at most 10 bytes) and the position after it. */
function varint(b: Uint8Array, at: number): [bigint, number] | null {
  let v = 0n;
  for (let i = 0; i < 10; i++) {
    const byte = b[at + i];
    if (byte === undefined) return null;
    v |= BigInt(byte & 0x7f) << BigInt(7 * i);
    if ((byte & 0x80) === 0) return i > 0 && byte === 0 ? null : [v, at + i + 1];
  }
  return null;
}

function base32(bytes: Uint8Array): string {
  let out = "";
  let acc = 0;
  let n = 0;
  for (const byte of bytes) {
    acc = ((acc << 8) | byte) & 0xfff;
    n += 8;
    while (n >= 5) {
      n -= 5;
      out += B32[(acc >> n) & 31];
    }
  }
  return n > 0 ? out + B32[(acc << (5 - n)) & 31] : out;
}

/** The canonical form of a CIDv1 written in one of the multibases above, else null. As the
 * `cid` crate does, bytes after the multihash are not read. */
export function canonicalCid(s: string): string | null {
  const bytes = multibase(s);
  if (!bytes) return null;
  const version = varint(bytes, 0);
  const codec = version && varint(bytes, version[1]);
  // a CIDv0 (`0x12 0x20`, a bare sha2-256 multihash) or another version names no version
  if (!version || !codec || version[0] !== 1n) return null;
  const code = varint(bytes, codec[1]);
  const size = code && varint(bytes, code[1]);
  if (!code || !size || size[0] > 64n) return null;
  const end = size[1] + Number(size[0]);
  return end > bytes.length ? null : `b${base32(bytes.subarray(0, end))}`;
}

/** `written` (a CID in a record; absent names nothing) and a version's `cid` name the same
 * version (social.md §0.4). */
export function sameCid(written: string | undefined, cid: string): boolean {
  if (written === undefined) return false;
  if (written === cid) return true;
  const a = canonicalCid(written);
  return a !== null && a === canonicalCid(cid);
}
