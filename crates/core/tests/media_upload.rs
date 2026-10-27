// SPDX-License-Identifier: AGPL-3.0-or-later
//! The upload half of the media pipeline over the fake network (`data-model.md` §8.8,
//! `limits.md` §12): images are downscaled to 5 MiB when prepared, a 413 re-encodes at half the
//! last size up to three times, and the body's `cid:` URLs follow the CID the PDS stored.

mod common;

use common::block_on;
use lessdugin_core::dto::{
    CommentDraft, Draft, EditOpts, ForumDraft, MediaInput, MediaRef, PostDraft, WriteState,
    WriteTicket,
};
use lessdugin_core::engine::{AccountInfo, Engine, EngineConfig, ErrorCode};
use lessdugin_core::media::{Format, sniff};
use lessdugin_lexicon::collection::nsid;
use lessdugin_testkit::fake_pds::{AccountHandle, PdsConfig};
use lessdugin_testkit::{FakeNet, MemHost};
use serde_json::Value;

/// A PNG of `w`×`h` pixels of noise: it does not compress, so its size is about `3·w·h` bytes.
fn noise_png(w: u32, h: u32, seed: u32) -> Vec<u8> {
    let mut x = seed.wrapping_mul(2_654_435_761).max(1);
    let img = image::RgbImage::from_fn(w, h, |_, _| {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        image::Rgb([x as u8, (x >> 8) as u8, (x >> 16) as u8])
    });
    let mut out = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

fn dimensions(bytes: &[u8]) -> (u32, u32) {
    let img = image::load_from_memory(bytes).expect("the image decodes");
    (img.width(), img.height())
}

fn input(alt: &str) -> MediaInput {
    MediaInput {
        declared_mime: Some("image/png".into()),
        alt: Some(alt.into()),
        name: None,
        aspect_ratio: None,
    }
}

/// One signed-in author on a PDS with `cfg`, and the forum they created.
struct Author {
    net: FakeNet,
    engine: Engine<MemHost>,
    acct: AccountHandle,
    forum: String,
}

impl Author {
    fn new(cfg: PdsConfig) -> Self {
        let host = MemHost::new();
        let net = FakeNet::on(&host);
        let pds = net.add_pds_with("small.example.com", cfg);
        let acct = pds.add_account("alice.example.com");
        let engine = Engine::new(host, EngineConfig::default());
        block_on(engine.add_account(AccountInfo::app_password(
            acct.did().as_str(),
            acct.pds_base(),
        )))
        .unwrap();
        let t = block_on(engine.forum_create(&ForumDraft::new("pics", "Pictures"))).unwrap();
        let mut a = Self {
            net,
            engine,
            acct,
            forum: String::new(),
        };
        a.forum = a.done(&t);
        a
    }

    /// Runs the outbox until idle and returns the uri the write created.
    fn done(&self, t: &WriteTicket) -> String {
        block_on(self.engine.outbox_flush()).unwrap();
        match block_on(self.engine.write_status(&t.id))
            .unwrap()
            .unwrap()
            .state
        {
            WriteState::Done { uri, .. } => uri,
            other => panic!("write {} is {other:?}", t.id),
        }
    }

    fn state(&self, t: &WriteTicket) -> WriteState {
        block_on(self.engine.outbox_flush()).unwrap();
        block_on(self.engine.write_status(&t.id))
            .unwrap()
            .unwrap()
            .state
    }

    fn post_with(&self, m: &MediaRef) -> WriteTicket {
        let mut d = PostDraft::article(
            &self.forum,
            "A picture",
            format!("Look: ![noise](cid:{})", m.cid),
        );
        d.body.media.push(m.clone());
        block_on(self.engine.publish(&d)).unwrap()
    }

    /// The published post's value, as the PDS holds it.
    fn record(&self, uri: &str) -> Value {
        let rkey = uri.rsplit('/').next().unwrap();
        self.acct.get(nsid::POST, rkey).expect("the post is in the repo")
    }
}

#[test]
fn an_image_over_5_mib_is_downscaled_when_it_is_prepared() {
    // acc-m4-17: the first target is IMAGE_UPLOAD_TARGET_BYTES, before any upload, so the CID in
    // the draft is the one that is uploaded
    let big = noise_png(1_400, 1_400, 1);
    assert!(big.len() > 5 * 1024 * 1024, "{} bytes", big.len());
    let e = Engine::new(MemHost::new(), EngineConfig::default());
    let m = block_on(e.prepare_media(big, &input("noise"))).unwrap();
    assert!(m.size_bytes <= 5 * 1024 * 1024, "{} bytes", m.size_bytes);
    let blob = block_on(e.prepared_media(&m.cid)).unwrap().unwrap();
    assert_eq!(blob.bytes.len() as u64, m.size_bytes);
    assert_eq!(lessdugin_atp::cid::for_raw(&blob.bytes).to_string(), m.cid);
    assert_eq!(sniff(&blob.bytes), Some(Format::Png));
    assert_eq!(dimensions(&blob.bytes), (700, 700));
    // an image within the target keeps its pixels
    let small = noise_png(64, 64, 2);
    let m = block_on(e.prepare_media(small, &input("small"))).unwrap();
    let blob = block_on(e.prepared_media(&m.cid)).unwrap().unwrap();
    assert_eq!(dimensions(&blob.bytes), (64, 64));
}

#[test]
fn a_413_halves_the_image_until_the_pds_takes_it_and_the_body_follows_the_new_cid() {
    // m4-7: a PDS whose limit is far below 5 MiB. The image (about 196 KB) is under the first
    // target, so the first upload sends it whole; each 413 then halves the last encoding.
    let a = Author::new(PdsConfig {
        blob_upload_limit: 40_000,
        ..PdsConfig::default()
    });
    let m = block_on(a.engine.prepare_media(noise_png(256, 256, 3), &input("noise"))).unwrap();
    let uploads = a.net.count("com.atproto.repo.uploadBlob");
    let uri = a.done(&a.post_with(&m));
    // 196 KB refused, 49 KB refused, 12 KB stored
    assert_eq!(a.net.count("com.atproto.repo.uploadBlob") - uploads, 3);
    let rec = a.record(&uri);
    let stored = rec["body"]["media"][0]["blob"]["ref"]["$link"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(stored, m.cid, "the PDS stored the downscaled bytes");
    assert!(rec["body"]["media"][0]["blob"]["size"].as_u64().unwrap() <= 40_000);
    // m4-8: the inline image still resolves (lfm-1.md §6.1 matches `cid:` against the media)
    let text = rec["body"]["text"].as_str().unwrap();
    assert_eq!(text, format!("Look: ![noise](cid:{stored})"));
    assert!(!text.contains(&m.cid));
}

#[test]
fn an_image_still_refused_after_three_halvings_fails_the_write() {
    let a = Author::new(PdsConfig {
        blob_upload_limit: 1_000,
        ..PdsConfig::default()
    });
    let m = block_on(a.engine.prepare_media(noise_png(256, 256, 4), &input("noise"))).unwrap();
    let uploads = a.net.count("com.atproto.repo.uploadBlob");
    let t = a.post_with(&m);
    match a.state(&t) {
        WriteState::Failed { error } => assert_eq!(error.code, ErrorCode::TooLarge, "{error:?}"),
        other => panic!("the write must fail: {other:?}"),
    }
    // the first attempt and IMAGE_UPLOAD_413_RETRIES re-encodings, then the refusal
    assert_eq!(a.net.count("com.atproto.repo.uploadBlob") - uploads, 4);
}

#[test]
fn a_spilled_body_that_names_a_downscaled_image_fails_rather_than_lose_it() {
    // m4-8: a body over 64 KiB is a `textBlob` (data-model.md §5.3), whose bytes cannot follow
    // the new CID, so the write fails with a reason instead of publishing a broken image
    let a = Author::new(PdsConfig {
        blob_upload_limit: 100_000,
        ..PdsConfig::default()
    });
    let m = block_on(a.engine.prepare_media(noise_png(256, 256, 5), &input("noise"))).unwrap();
    let long = format!("![noise](cid:{})\n\n{}", m.cid, "word ".repeat(14_000));
    let mut d = PostDraft::article(&a.forum, "A long one", long);
    d.body.media.push(m);
    let t = block_on(a.engine.publish(&d)).unwrap();
    match a.state(&t) {
        WriteState::Failed { error } => {
            assert_eq!(error.code, ErrorCode::TooLarge, "{error:?}");
            assert_eq!(
                error.detail.as_ref().and_then(|d| d.get("kind")),
                Some(&serde_json::json!("mediaRejected"))
            );
        }
        other => panic!("the write must fail: {other:?}"),
    }
}

#[test]
fn a_comment_uploads_its_media() {
    // m4-17: comment() sent no blobs, so the record named a blob the PDS never received and the
    // write failed with BlobNotFound
    let a = Author::new(PdsConfig::default());
    let post = a.done(&block_on(a.engine.publish(&PostDraft::article(&a.forum, "t", "b"))).unwrap());
    let m = block_on(a.engine.prepare_media(noise_png(16, 16, 6), &input("a reply"))).unwrap();
    let mut d = CommentDraft::new(&post, None, format!("![r](cid:{})", m.cid));
    d.body.media.push(m.clone());
    let uri = a.done(&block_on(a.engine.comment(&d)).unwrap());
    let rec = a
        .acct
        .get(nsid::COMMENT, uri.rsplit('/').next().unwrap())
        .unwrap();
    assert_eq!(rec["body"]["media"][0]["blob"]["ref"]["$link"], m.cid.as_str());
    assert!(a.acct.serves_blob(&lessdugin_atp::cid::parse(&m.cid).unwrap()));
}

#[test]
fn an_edit_uploads_the_media_it_adds_and_keeps_the_blob_objects_it_had() {
    // m4-17: edit() sent no blobs either
    let a = Author::new(PdsConfig::default());
    let first = block_on(a.engine.prepare_media(noise_png(16, 16, 7), &input("one"))).unwrap();
    let uri = a.done(&a.post_with(&first));
    let before = a.record(&uri)["body"]["media"][0].clone();
    let second = block_on(a.engine.prepare_media(noise_png(16, 16, 8), &input("two"))).unwrap();
    let mut post = PostDraft::article(
        &a.forum,
        "Two pictures",
        format!("![one](cid:{}) ![two](cid:{})", first.cid, second.cid),
    );
    post.body.media = vec![first.clone(), second.clone()];
    let uploads = a.net.count("com.atproto.repo.uploadBlob");
    let edit = Draft::Post { id: None, post };
    let t = block_on(a.engine.edit(&uri, &edit, &EditOpts::default())).unwrap();
    assert_eq!(a.done(&t), uri);
    // only the new image went up
    assert_eq!(a.net.count("com.atproto.repo.uploadBlob") - uploads, 1);
    let rec = a.record(&uri);
    assert_eq!(rec["title"], "Two pictures");
    assert_eq!(rec["body"]["media"][0]["blob"], before["blob"], "kept verbatim");
    assert_eq!(rec["body"]["media"][1]["blob"]["ref"]["$link"], second.cid.as_str());
    assert!(a.acct.serves_blob(&lessdugin_atp::cid::parse(&second.cid).unwrap()));
}
