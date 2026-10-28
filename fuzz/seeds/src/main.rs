// SPDX-License-Identifier: AGPL-3.0-or-later
//! Writes the seed corpora of the fuzz targets from the repository's vectors and fixtures, and
//! copies the kept crash reproducers (`fuzz/regressions/<target>/`) into them.
//!
//! ```text
//! lessdugin-fuzz-seeds <corpus-root> [target…]     (default: every target)
//! ```
//!
//! Each seed is written as `<corpus-root>/<target>/seed-<sha256 prefix>` unless it exists, so
//! running it again only adds what is new; libFuzzer keeps adding its own inputs next to them.
//!
//! | Target | Seeds |
//! |---|---|
//! | `car` | the captured live proofs (`testdata/interop/live`), generated present/absent proof CARs and an indexer bundle |
//! | `commit` | the commit block of every seed CAR, and generated commits (unknown fields, `prev` forms, wrong version) |
//! | `dagcbor` | the interop data-model fixtures (CBOR, and the valid JSON encoded), every block of the seed CARs |
//! | `lexicon` | every record in the shared vectors (in their own and in the build namespace) and the live record |
//! | `render` | every LFM/1, XSS, terminal and MathML vector input (bodies, TeX, URLs, quote targets, names) |
//! | `mst` | none: its input is structured (`arbitrary`) |
//! | `media` | the SPA's e2e image fixture, and one small file per format the strippers walk (JPEG, PNG, WebP, GIF, MP4), each carrying the metadata they drop |

#[allow(dead_code)] // shared with the fuzz targets, which use the rest
#[path = "../../src/common.rs"]
mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use k256::ecdsa::signature::Signer as _;
use lessdugin_atp::car::{CarLimits, read_car, write_car};
use lessdugin_atp::commit::UnsignedCommit;
use lessdugin_atp::datamodel::{self, Limits};
use lessdugin_atp::syntax::{Did, Tid};
use lessdugin_atp::{Cid, Ipld, cid, dagcbor, mst};
use serde_json::Value;

const TARGETS: &[&str] = &[
    "car", "commit", "dagcbor", "lexicon", "mst", "render", "media",
];
/// The render target's `-max_len` (longer generated inputs are cut at a char boundary).
const RENDER_MAX: usize = 32 * 1024;
/// Lexicon seeds kept (the vectors hold thousands of near-identical records).
const LEXICON_MAX: usize = 3000;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> Vec<u8> {
    fs::read(root().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

fn json(rel: &str) -> Value {
    serde_json::from_slice(&read(rel)).unwrap_or_else(|e| panic!("parse {rel}: {e}"))
}

fn vector_files(dir: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = fs::read_dir(root().join(dir))
        .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    out.retain(|p| p.extension().is_some_and(|e| e == "json"));
    out.sort();
    out
}

fn sign(msg: &[u8]) -> Vec<u8> {
    let sk = k256::ecdsa::SigningKey::from_slice(&common::FUZZ_SECRET).expect("secret");
    let s: k256::ecdsa::Signature = sk.sign(msg);
    s.to_bytes().to_vec()
}

fn encode(v: &Ipld) -> Vec<u8> {
    dagcbor::encode_with(v, &Limits::uncapped(16)).expect("encodable")
}

fn map(entries: Vec<(&str, Ipld)>) -> Ipld {
    Ipld::Map(
        entries
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
    )
}

/// A generated repo: commit, MST nodes and records.
struct Repo {
    commit: Cid,
    blocks: BTreeMap<Cid, Vec<u8>>,
    entries: BTreeMap<String, Cid>,
}

fn repo(keys: &[&str]) -> Repo {
    let mut blocks = BTreeMap::new();
    let mut entries = BTreeMap::new();
    for (i, k) in keys.iter().enumerate() {
        let rec = encode(&map(vec![
            ("$type", Ipld::String("app.x.record".into())),
            ("text", Ipld::String(format!("record {i}"))),
        ]));
        let c = cid::for_dag_cbor(&rec);
        blocks.insert(c, rec);
        entries.insert((*k).to_owned(), c);
    }
    let (root, nodes) =
        mst::build(entries.iter().map(|(k, v)| (k.as_bytes(), *v))).expect("valid keys");
    blocks.extend(nodes);
    let unsigned = UnsignedCommit {
        did: Did::parse(common::FUZZ_DID).expect("DID"),
        data: root,
        rev: Tid::from_parts(common::REV_MICROS, 0).expect("TID"),
        prev: None,
    };
    let (commit, bytes) = unsigned.signed(&sign(&unsigned.signing_bytes()));
    blocks.insert(commit, bytes);
    Repo {
        commit,
        blocks,
        entries,
    }
}

fn car_of(roots: &[Cid], blocks: &BTreeMap<Cid, Vec<u8>>, first: &Cid) -> Vec<u8> {
    let mut list: Vec<(&Cid, &[u8])> = Vec::new();
    if let Some(b) = blocks.get(first) {
        list.push((first, b));
    }
    list.extend(
        blocks
            .iter()
            .filter(|(c, _)| *c != first)
            .map(|(c, b)| (c, b.as_slice())),
    );
    write_car(roots, list)
}

fn generated_cars() -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let keys = [
        "app.bsky.feed.post/self",
        "app.bsky.feed.post/3k2akbibjhm2a",
        "online.7778777.lessdugin.temp.post/self",
        "online.7778777.lessdugin.temp.comment/3jzfcijpj2z2a",
        "app.bsky.actor.profile/self",
        "a.b.c/self",
        "a.b.c/x",
    ];
    for n in [1, 3, keys.len()] {
        let r = repo(&keys[..n]);
        out.push(car_of(&[r.commit], &r.blocks, &r.commit));
    }
    // An empty repo (absence proofs only).
    let r = repo(&[]);
    out.push(car_of(&[r.commit], &r.blocks, &r.commit));
    // An indexer bundle: one true claim, one false, one absent.
    let r = repo(&keys);
    let did = common::FUZZ_DID;
    let item = |key: &str, cid: Option<Cid>| {
        map(vec![
            ("uri", Ipld::String(format!("at://{did}/{key}"))),
            ("cid", cid.map_or(Ipld::Null, Ipld::Link)),
            ("commit", Ipld::Link(r.commit)),
        ])
    };
    let first = r.entries.values().next().copied();
    let manifest = encode(&map(vec![
        ("v", Ipld::Integer(1)),
        (
            "items",
            Ipld::List(vec![
                item(keys[0], r.entries.get(keys[0]).copied()),
                item(keys[1], first),
                item("app.bsky.feed.post/absent", None),
            ]),
        ),
    ]));
    let mc = cid::for_dag_cbor(&manifest);
    let mut blocks = r.blocks.clone();
    blocks.insert(mc, manifest);
    out.push(car_of(&[mc], &blocks, &mc));
    out
}

fn generated_commits() -> Vec<Vec<u8>> {
    let data = Ipld::Link(cid::for_dag_cbor(b"\xa0"));
    let rev = Tid::from_parts(common::REV_MICROS, 0).expect("TID");
    let base = || {
        vec![
            ("did", Ipld::String(common::FUZZ_DID.into())),
            ("version", Ipld::Integer(3)),
            ("data", data.clone()),
            ("rev", Ipld::String(rev.to_string())),
            ("prev", Ipld::Null),
        ]
    };
    let mut variants = vec![base()];
    let mut v = base();
    v.push((
        "zz",
        Ipld::List(vec![Ipld::Bool(true), Ipld::Bytes(vec![1, 2])]),
    ));
    variants.push(v);
    let mut v = base();
    v.retain(|(k, _)| *k != "prev");
    variants.push(v);
    let mut v = base();
    v[4].1 = Ipld::Link(cid::for_dag_cbor(b"\xa1"));
    variants.push(v);
    let mut v = base();
    v[1].1 = Ipld::Integer(2);
    variants.push(v);
    variants
        .into_iter()
        .map(|fields| {
            let unsigned = encode(&map(fields.clone()));
            let mut fields = fields;
            fields.push(("sig", Ipld::Bytes(sign(&unsigned))));
            encode(&map(fields))
        })
        .collect()
}

fn seed_cars() -> Vec<Vec<u8>> {
    let mut cars = vec![
        read("testdata/interop/live/bsky.app-profile-self.car"),
        read("testdata/interop/live/bsky.app-profile-absent.car"),
    ];
    cars.extend(generated_cars());
    cars
}

fn seeds_car() -> Vec<Vec<u8>> {
    seed_cars()
}

fn seeds_commit() -> Vec<Vec<u8>> {
    let mut out = generated_commits();
    for car in seed_cars() {
        if let Ok(c) = read_car(&car, &CarLimits::BUNDLE) {
            for r in &c.roots {
                if let Some(b) = c.get(r) {
                    out.push(b.to_vec());
                }
            }
        }
    }
    out
}

fn seeds_dagcbor() -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    if let Value::Array(fixtures) = json("testdata/interop/data-model/data-model-fixtures.json") {
        for f in fixtures {
            if let Some(b) = f["cbor_base64"].as_str()
                && let Ok(bytes) = datamodel::decode_base64(b)
            {
                out.push(bytes);
            }
        }
    }
    if let Value::Array(valid) = json("testdata/interop/data-model/data-model-valid.json") {
        for v in valid {
            if let Ok(ipld) = datamodel::json_to_ipld(&v["json"])
                && let Ok(b) = dagcbor::encode(&ipld)
            {
                out.push(b);
            }
        }
    }
    for car in seed_cars() {
        if let Ok(c) = read_car(&car, &CarLimits::BUNDLE) {
            out.extend(c.blocks.into_values());
        }
    }
    out
}

/// Every object with a record `$type` (any namespace) in `v`.
fn records_in(v: &Value, out: &mut Vec<Value>, known: &BTreeSet<String>) {
    let mut stack = vec![v];
    while let Some(x) = stack.pop() {
        match x {
            Value::Object(o) => {
                if let Some(Value::String(t)) = o.get("$type") {
                    let build = to_build(t);
                    if known.contains(t) || known.contains(&build) {
                        out.push(x.clone());
                    }
                }
                stack.extend(o.values());
            }
            Value::Array(a) => stack.extend(a.iter()),
            _ => {}
        }
    }
}

/// The text with every final-namespace NSID moved to the build namespace.
fn to_build(s: &str) -> String {
    let fin = format!("{}.", lessdugin_lexicon::ns::NS_FINAL);
    let build = format!("{}.", lessdugin_lexicon::ns::NS);
    s.replace(&fin, &build)
        .replace(&format!("{build}temp."), &build)
}

fn seeds_lexicon() -> Vec<Vec<u8>> {
    let known: BTreeSet<String> = lessdugin_lexicon::schemas::registry()
        .record_nsids()
        .into_iter()
        .map(str::to_owned)
        .collect();
    let mut records = Vec::new();
    let mut dirs: Vec<PathBuf> = fs::read_dir(root().join("testdata/vectors"))
        .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    dirs.sort();
    for d in dirs {
        let rel = d
            .strip_prefix(root())
            .unwrap_or(&d)
            .to_string_lossy()
            .into_owned();
        for f in vector_files(&rel) {
            if let Ok(v) = serde_json::from_slice::<Value>(&fs::read(&f).unwrap_or_default()) {
                records_in(&v, &mut records, &known);
            }
        }
    }
    let mut seen = BTreeSet::new();
    let mut out = vec![read(
        "testdata/interop/live/bsky.app-profile-self.record.json",
    )];
    for r in records {
        let s = serde_json::to_string(&r).expect("JSON");
        for s in [to_build(&s), s] {
            if seen.insert(s.clone()) && out.len() < LEXICON_MAX {
                out.push(s.into_bytes());
            }
        }
    }
    out
}

fn expand_gen(g: &Value) -> Option<String> {
    let times = usize::try_from(g["times"].as_u64()?).ok()?;
    let rep = g["repeat"].as_str().unwrap_or("");
    let mut s = g["prefix"].as_str().unwrap_or("").to_owned();
    for _ in 0..times {
        if s.len() > RENDER_MAX {
            break;
        }
        s.push_str(rep);
    }
    s.push_str(g["suffix"].as_str().unwrap_or(""));
    Some(s)
}

fn cut(mut s: String) -> String {
    if s.len() > RENDER_MAX {
        let mut i = RENDER_MAX;
        while !s.is_char_boundary(i) {
            i -= 1;
        }
        s.truncate(i);
    }
    s
}

fn seeds_render() -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for dir in [
        "testdata/vectors/lfm1",
        "testdata/vectors/xss",
        "testdata/vectors/terminal",
        "testdata/vectors/mathml",
    ] {
        for f in vector_files(dir) {
            let Ok(v) = serde_json::from_slice::<Value>(&fs::read(&f).unwrap_or_default()) else {
                continue;
            };
            let Some(vectors) = v["vectors"].as_array() else {
                continue;
            };
            for vec in vectors {
                let i = &vec["input"];
                let mut texts: Vec<String> = Vec::new();
                for k in ["text", "target", "did"] {
                    if let Some(s) = i[k].as_str() {
                        texts.push(s.to_owned());
                    }
                }
                for k in ["gen", "target_gen", "raw_gen"] {
                    if let Some(s) = expand_gen(&i[k]) {
                        texts.push(s);
                    }
                }
                if let Some(raw) = i["raw"].as_str() {
                    texts.push(format!("[x]({raw}) <{raw}> {raw}"));
                }
                if let Some(q) = i["quote"]["exact"].as_str() {
                    texts.push(q.to_owned());
                }
                if let Some(tex) = i["tex"].as_str() {
                    texts.push(if i["display"].as_bool() == Some(true) {
                        format!("$${tex}$$")
                    } else {
                        format!("${tex}$")
                    });
                }
                out.extend(texts.into_iter().map(|t| cut(t).into_bytes()));
            }
        }
    }
    out
}

/// The media strippers' formats (`lessdugin_core::media`, `threat-model.md` §5.9.1), each with
/// the segments, chunks or boxes the walk drops or keeps, so that libFuzzer starts inside every
/// walker rather than at the format sniff.
fn seeds_media() -> Vec<Vec<u8>> {
    let jpeg_seg = |marker: u8, payload: &[u8]| {
        let mut v = vec![0xFF, marker];
        v.extend_from_slice(&u16::try_from(payload.len() + 2).unwrap().to_be_bytes());
        v.extend_from_slice(payload);
        v
    };
    let mut jpeg = vec![0xFF, 0xD8];
    jpeg.extend(jpeg_seg(0xE0, b"JFIF\0\x01\x02\0\0\x01\0\x01\0\0"));
    // EXIF: IFD0 with one Orientation entry (6)
    let mut exif = b"Exif\0\0II\x2a\x00\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0".to_vec();
    exif.extend_from_slice(&[0, 0, 0, 0]);
    jpeg.extend(jpeg_seg(0xE1, &exif));
    jpeg.extend(jpeg_seg(0xE2, b"ICC_PROFILE\0\x01\x01icc"));
    jpeg.extend(jpeg_seg(0xED, b"Photoshop 3.0\0IPTC"));
    jpeg.extend(jpeg_seg(0xFE, b"comment"));
    jpeg.extend(jpeg_seg(0xDB, &[0, 1, 2, 3, 4, 5, 6, 7]));
    jpeg.extend(jpeg_seg(0xC0, &[8, 0, 8, 0, 8, 1, 1, 0x11, 0]));
    jpeg.extend(jpeg_seg(0xC4, &[0, 0, 1, 0, 1, 0, 0, 0]));
    jpeg.extend(jpeg_seg(0xDA, &[1, 1, 0, 0, 63, 0]));
    jpeg.extend_from_slice(&[0x12, 0xFF, 0x00, 0x56, 0xFF, 0xD0, 0x78, 0xFF, 0xD9]);
    jpeg.extend_from_slice(b"after-eoi");

    let png_chunk = |kind: &[u8; 4], data: &[u8]| {
        let mut v = u32::try_from(data.len()).unwrap().to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(data);
        v.extend_from_slice(&[0, 0, 0, 0]);
        v
    };
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend(png_chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]));
    png.extend(png_chunk(b"tEXt", b"Author\0name"));
    png.extend(png_chunk(b"eXIf", b"II\x2a\x00"));
    png.extend(png_chunk(b"acTL", &[0, 0, 0, 1, 0, 0, 0, 0]));
    png.extend(png_chunk(b"IDAT", b"pixels"));
    png.extend(png_chunk(b"IEND", b""));
    png.extend_from_slice(b"after-iend");

    let riff_chunk = |id: &[u8; 4], data: &[u8]| {
        let mut v = id.to_vec();
        v.extend_from_slice(&u32::try_from(data.len()).unwrap().to_le_bytes());
        v.extend_from_slice(data);
        if data.len() % 2 == 1 {
            v.push(0);
        }
        v
    };
    let mut body = riff_chunk(b"VP8X", &[0x3C, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    body.extend(riff_chunk(b"ICCP", b"icc"));
    body.extend(riff_chunk(b"EXIF", b"Exif\0\0"));
    body.extend(riff_chunk(b"XMP ", b"<x/>"));
    body.extend(riff_chunk(b"ALPH", b"\x00a"));
    body.extend(riff_chunk(b"VP8 ", b"\x00\x01\x02\x03"));
    let mut webp = b"RIFF".to_vec();
    webp.extend_from_slice(&u32::try_from(body.len() + 4).unwrap().to_le_bytes());
    webp.extend_from_slice(b"WEBP");
    webp.extend_from_slice(&body);

    let mut gif = b"GIF89a\x01\0\x01\0\0\0\0".to_vec();
    gif.extend_from_slice(b"\x21\xFF\x0BNETSCAPE2.0\x03\x01\0\0\0");
    gif.extend_from_slice(b"\x21\xFE\x07comment\0");
    gif.extend_from_slice(b"\x21\xFF\x0BXMP DataXMP\x01x\0");
    gif.extend_from_slice(b"\x21\xF9\x04\0\0\0\0\0");
    gif.extend_from_slice(b"\x2C\0\0\0\0\x01\0\x01\0\0\x02\x02\x44\x01\0\x3B");
    gif.extend_from_slice(b"after-trailer");

    let mp4_box = |kind: &[u8; 4], payload: &[u8]| {
        let mut v = u32::try_from(payload.len() + 8)
            .unwrap()
            .to_be_bytes()
            .to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(payload);
        v
    };
    let mut moov = mp4_box(b"mvhd", &[0u8; 20]);
    moov.extend(mp4_box(b"udta", &mp4_box(b"meta", b"\0\0\0\0location")));
    let mut mp4 = mp4_box(b"ftyp", b"isom\0\0\x02\0isomiso2");
    mp4.extend(mp4_box(b"moov", &moov));
    mp4.extend(mp4_box(b"mdat", b"video"));

    vec![
        read("web/e2e/fixtures/pixel.png"),
        jpeg,
        png,
        webp,
        gif,
        mp4,
    ]
}

fn regressions(target: &str) -> Vec<Vec<u8>> {
    let dir = root().join("fuzz/regressions").join(target);
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    files.retain(|p| p.is_file() && p.extension().is_none_or(|e| e != "md"));
    files.sort();
    files.iter().filter_map(|p| fs::read(p).ok()).collect()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(corpus) = args.first() else {
        eprintln!("usage: lessdugin-fuzz-seeds <corpus-root> [target…]");
        std::process::exit(2);
    };
    let targets: Vec<&str> = if args.len() > 1 {
        args[1..].iter().map(String::as_str).collect()
    } else {
        TARGETS.to_vec()
    };
    for t in targets {
        let mut seeds = match t {
            "car" => seeds_car(),
            "commit" => seeds_commit(),
            "dagcbor" => seeds_dagcbor(),
            "lexicon" => seeds_lexicon(),
            "render" => seeds_render(),
            "mst" => Vec::new(),
            "media" => seeds_media(),
            other => {
                eprintln!("unknown target {other:?} (known: {})", TARGETS.join(", "));
                std::process::exit(2);
            }
        };
        seeds.extend(regressions(t));
        let dir = Path::new(corpus).join(t);
        fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
        let mut written = 0usize;
        let mut unique = BTreeSet::new();
        for s in &seeds {
            let h = cid::sha256(s);
            let name: String = h[..8].iter().map(|b| format!("{b:02x}")).collect();
            if !unique.insert(name.clone()) {
                continue;
            }
            let p = dir.join(format!("seed-{name}"));
            if !p.exists() {
                fs::write(&p, s).unwrap_or_else(|e| panic!("write {}: {e}", p.display()));
                written += 1;
            }
        }
        eprintln!(
            "seeds: {t}: {} unique, {written} new in {}",
            unique.len(),
            dir.display()
        );
    }
}
