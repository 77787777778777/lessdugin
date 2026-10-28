// SPDX-License-Identifier: AGPL-3.0-or-later
//! The terminal's images (synthesis-v1.md F35/F36, `threat-model.md` SUP-9, `sync.md` §11): a
//! decompression bomb is refused from its HEADER and again by the decoder's own limits, the
//! decode runs off the UI thread under a timeout, a disabled client decodes nothing, and on the
//! Thread screen an image loads on `i` (or by the autoload policy) and is drawn.

#![allow(clippy::disallowed_methods)] // the image workers are threads on wall time

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{block_on, scenario};
use lessdugin_tui::image::{Images, MediaBytes, Placeholder, decode, limits};
use lessdugin_tui::thread::MediaState;

/// A PNG whose IHDR declares `w` x `h` and whose body is a single IDAT chunk. The pixels are not
/// valid zlib, which is the point: the caps must bite before any decoder reads them.
fn png_declaring(w: u32, h: u32) -> Vec<u8> {
    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut v = (data.len() as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(data);
        v.extend_from_slice(&[0, 0, 0, 0]);
        v
    }
    let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = w.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    v.extend(chunk(b"IHDR", &ihdr));
    v.extend(chunk(b"IDAT", b"not really pixels"));
    v.extend(chunk(b"IEND", b""));
    v
}

/// A real PNG, `w` x `h`, a vertical gradient (so halfblocks draw it with half blocks, not
/// with coloured spaces).
fn png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(w, h, |_, y| {
        let v = u8::try_from(y * 255 / h.max(1)).unwrap_or(255);
        image::Rgba([v, 30, 255 - v, 255])
    });
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

fn bytes(v: Vec<u8>) -> MediaBytes {
    MediaBytes(Arc::new(v))
}

/// Polls `img` until a decode reports, at most `wait`.
fn outcome(img: &mut Images, wait: Duration) -> Vec<(String, Result<(), Placeholder>)> {
    let end = Instant::now() + wait;
    loop {
        let p = img.poll();
        if !p.decoded.is_empty() || Instant::now() > end {
            return p.decoded;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn halfblocks() -> Images {
    Images::new(ratatui_image::picker::Picker::halfblocks(), Arc::new(|| {}))
}

#[test]
fn a_huge_declared_frame_is_refused_from_the_header() {
    // 100000 x 100000 declarations: the file itself is tiny, the allocation would be 40 GB
    let bomb = png_declaring(100_000, 100_000);
    assert!(bomb.len() < 1000, "the fixture must be tiny");
    assert_eq!(decode(&bomb).unwrap_err(), Placeholder::TooLarge);
    assert_eq!(Placeholder::TooLarge.text(), "[image too large]");
}

/// The side cap holds for a real image one pixel past it, and one at the cap decodes (through
/// the core's `decode_image`, F3) and is scaled down to what a terminal can show.
#[test]
fn the_decoder_applies_the_caps_itself() {
    let wide = png(limits::MAX_SIDE + 1, 1);
    assert_eq!(decode(&wide).unwrap_err(), Placeholder::TooLarge);
    let ok = decode(&png(limits::MAX_SIDE, 1)).unwrap();
    assert!(ok.width() <= limits::SHOWN_MAX_SIDE, "{}", ok.width());
}

#[test]
fn the_limits_are_the_ones_the_spec_names() {
    // synthesis-v1.md F35/F36 and limits.md §6: 64 MiB, 8192 a side, 256 MiB, 200 GIF frames,
    // 10 s
    assert_eq!(limits::MAX_INPUT_BYTES, 64 * 1024 * 1024);
    assert_eq!(limits::MAX_SIDE, 8_192);
    assert_eq!(limits::MAX_ALLOC, 256 * 1024 * 1024);
    assert_eq!(limits::MAX_GIF_FRAMES, 200);
    assert_eq!(limits::DECODE_TIMEOUT.as_secs(), 10);
}

#[test]
fn a_file_over_the_input_cap_is_refused_before_parsing() {
    // one byte over the cap, with no valid header at all: the size check must come first
    let big = vec![0u8; limits::MAX_INPUT_BYTES + 1];
    assert_eq!(decode(&big).unwrap_err(), Placeholder::TooLarge);
    let mut img = halfblocks();
    img.decode("big".into(), bytes(big));
    assert_eq!(
        img.poll().decoded,
        [("big".to_owned(), Err(Placeholder::TooLarge))]
    );
    assert!(!img.busy(), "nothing was handed to a worker");
}

#[test]
fn garbage_is_unsupported_not_a_panic() {
    assert_eq!(decode(&[0xFF; 64]).unwrap_err(), Placeholder::Unsupported);
    assert_eq!(decode(&[]).unwrap_err(), Placeholder::Unsupported);
    // a format the terminal does not decode (BMP)
    let mut bmp = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(2, 2)
        .write_to(&mut bmp, image::ImageFormat::Bmp)
        .unwrap();
    assert_eq!(decode(bmp.get_ref()).unwrap_err(), Placeholder::Unsupported);
    for p in [
        Placeholder::TooLarge,
        Placeholder::Unsupported,
        Placeholder::Loading,
        Placeholder::Failed,
    ] {
        assert!(p.text().starts_with('['), "{p:?}");
    }
}

#[test]
fn a_disabled_client_decodes_nothing() {
    let mut img = Images::disabled();
    img.decode("a".into(), bytes(png(2, 2)));
    assert_eq!(
        img.poll().decoded,
        [("a".to_owned(), Err(Placeholder::Unsupported))]
    );
    assert!(!img.has("a"));
}

/// The decode runs on a worker; the caller only polls.
#[test]
fn an_image_decodes_off_the_ui_thread() {
    let mut img = halfblocks();
    img.decode("a".into(), bytes(png(16, 16)));
    // `decode` returned at once: the worker reports through `poll`
    let got = outcome(&mut img, Duration::from_secs(10));
    assert_eq!(got, [("a".to_owned(), Ok(()))]);
    assert!(img.has("a"));
    // a corrupt file is a placeholder, and the next image still decodes
    img.decode("b".into(), bytes(png_declaring(4, 4)));
    assert_eq!(
        outcome(&mut img, Duration::from_secs(10)),
        [("b".to_owned(), Err(Placeholder::Failed))]
    );
    img.decode("c".into(), bytes(png(8, 8)));
    assert_eq!(
        outcome(&mut img, Duration::from_secs(10)),
        [("c".to_owned(), Ok(()))]
    );
}

/// A decode past its timeout is abandoned and reported as failed.
#[test]
fn a_decode_past_the_timeout_is_abandoned() {
    let mut img = halfblocks().with_timeout(Duration::from_millis(1));
    img.decode("slow".into(), bytes(png(2048, 2048)));
    assert_eq!(
        outcome(&mut img, Duration::from_secs(20)),
        [("slow".to_owned(), Err(Placeholder::Failed))]
    );
    assert!(!img.has("slow"));
}

/// A decode abandoned past its timeout keeps its slot until its decoder ends (it still holds
/// its input and its allocation): it counts against `MAX_DECODING`, and the next decode waits
/// for it rather than adding a thread.
#[test]
fn a_timed_out_decode_keeps_its_slot_until_it_ends() {
    use std::sync::{Condvar, Mutex};
    // decoders that run until the gate opens
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let g = Arc::clone(&gate);
    let mut img = halfblocks()
        .with_timeout(Duration::from_millis(20))
        .with_decoder(Arc::new(move |b: &[u8]| {
            let (open, cv) = &*g;
            let mut o = open.lock().unwrap();
            while !*o {
                o = cv.wait(o).unwrap();
            }
            drop(o);
            decode(b)
        }));
    for cid in ["slow1", "slow2"] {
        img.decode(cid.into(), bytes(png(4, 4)));
    }
    let mut got = Vec::new();
    let end = Instant::now() + Duration::from_secs(10);
    while got.len() < 2 && Instant::now() < end {
        got.extend(img.poll().decoded);
        std::thread::sleep(Duration::from_millis(5));
    }
    got.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        got,
        [
            ("slow1".to_owned(), Err(Placeholder::Failed)),
            ("slow2".to_owned(), Err(Placeholder::Failed))
        ]
    );
    // both decoders still run: they keep their slots, and the next decode waits
    assert_eq!(img.decoding(), limits::MAX_DECODING);
    img.decode("next".into(), bytes(png(4, 4)));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(img.poll().decoded, [], "it waits for a slot");
    assert_eq!(img.decoding(), limits::MAX_DECODING);
    assert!(img.busy());
    // once they end, it runs
    {
        let (open, cv) = &*gate;
        *open.lock().unwrap() = true;
        cv.notify_all();
    }
    assert_eq!(
        outcome(&mut img, Duration::from_secs(10)),
        [("next".to_owned(), Ok(()))]
    );
    let end = Instant::now() + Duration::from_secs(10);
    while img.decoding() > 0 && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(img.decoding(), 0, "every slot is handed back");
}

/// At most `MAX_SHOWN` decoded images are kept; the oldest goes first, and is reported.
#[test]
fn the_oldest_image_is_dropped_past_the_cap() {
    let mut img = halfblocks();
    let mut evicted = Vec::new();
    for i in 0..=limits::MAX_SHOWN {
        img.decode(format!("c{i}"), bytes(png(4, 4)));
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            let p = img.poll();
            evicted.extend(p.evicted);
            if !p.decoded.is_empty() || Instant::now() > end {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    assert_eq!(evicted, ["c0"]);
    assert!(!img.has("c0") && img.has(&format!("c{}", limits::MAX_SHOWN)));
}

/// On the Thread screen an image the autoload policy does not load says where it would come
/// from; `i` (the reader's consent) fetches it from the author's PDS, CID-checked, decodes it
/// off-thread and draws it.
#[test]
fn i_loads_the_selected_items_image() {
    use lessdugin_core::dto::{MediaInput, PostDraft};
    let s = scenario();
    let (bob, carol) = (&s.w.bob, &s.w.carol);
    let prepared = block_on(bob.engine.prepare_media(
        png(24, 16),
        &MediaInput {
            declared_mime: Some("image/png".into()),
            alt: Some("a red square".into()),
            name: None,
            aspect_ratio: None,
        },
    ))
    .unwrap();
    let cid = prepared.cid.clone();
    let mut d = PostDraft::article(&s.forum, "A picture", format!("Look: ![red](cid:{cid})"));
    d.body.media.push(prepared);
    let t = block_on(bob.engine.publish(&d)).unwrap();
    bob.flush();
    let post = bob.done(&t);
    carol.sync(&s.forum);

    let mut t = block_on(lessdugin_tui::Headless::new(
        carol.engine.clone(),
        lessdugin_tui::HeadlessOpts {
            width: 80,
            height: 40,
            images: true,
            ..lessdugin_tui::HeadlessOpts::default()
        },
    ))
    .unwrap();
    block_on(t.start(None));
    block_on(t.open_thread(&post));
    let media = |t: &lessdugin_tui::Headless<lessdugin_testkit::MemHost>| {
        t.state.top_thread().unwrap().media.get(&cid).cloned()
    };
    // not loaded by itself: it says where it would load from
    assert_eq!(media(&t), None);
    let screen = t.render();
    assert!(screen.contains("[image 1: a red square]"), "{screen}");
    assert!(
        screen.contains("i loads it from pds.example.com"),
        "{screen}"
    );

    block_on(t.press("i"));
    assert_eq!(media(&t), Some(MediaState::Shown), "{}", t.render());
    assert!(t.images.has(&cid));
    let screen = t.render_images();
    // halfblocks: the image is half-block cells
    assert!(
        screen.contains('\u{2580}') || screen.contains('\u{2584}'),
        "{screen}"
    );
    // `i` again does nothing more
    block_on(t.press("i"));
    assert!(
        t.state
            .status
            .as_ref()
            .is_some_and(|s| s.text.to_plain_string().contains("already")),
        "{:?}",
        t.state.status
    );
}

/// What loads without a key (`sync.md` §11.3, `threat-model.md` PRIV-1): an image the engine
/// says may autoload, of a `Visible` item, at most 5 MiB; never one of a collapsed item, even
/// expanded, nor a larger one.
#[test]
fn the_autoload_verdict_loads_only_visible_small_images() {
    use lessdugin_core::dto::{
        CollapseReason, ItemState, MediaInput, PostDraft, ThreadOpts, ThreadRoot,
    };
    use lessdugin_tui::app::ThreadData;
    use lessdugin_tui::{Action, Effect};
    let s = scenario();
    let (bob, carol) = (&s.w.bob, &s.w.carol);
    let prepared = block_on(bob.engine.prepare_media(
        png(8, 8),
        &MediaInput {
            declared_mime: Some("image/png".into()),
            alt: None,
            name: None,
            aspect_ratio: None,
        },
    ))
    .unwrap();
    let mut d = PostDraft::article(&s.forum, "Auto", format!("![x](cid:{})", prepared.cid));
    d.body.media.push(prepared);
    let t = block_on(bob.engine.publish(&d)).unwrap();
    bob.flush();
    let post = bob.done(&t);
    carol.sync(&s.forum);
    let mut t = common::tui(&s, 80, 30);
    block_on(t.start(None));
    block_on(t.open_thread(&post));
    let screen = t.state.top().unwrap().id;
    let view = block_on(carol.engine.get_thread(&post, &ThreadOpts::default())).unwrap();
    let loaded = |view: &lessdugin_core::dto::ThreadView| Action::ThreadLoaded {
        screen,
        origin: lessdugin_tui::app::Origin::Live,
        result: Ok(Box::new(ThreadData {
            view: view.clone(),
            bodies: Default::default(),
        })),
    };
    let fetches = |fx: &[Effect]| {
        fx.iter()
            .filter(|e| matches!(e, Effect::FetchMedia { .. }))
            .count()
    };
    // the engine's verdict today: click to load
    assert_eq!(fetches(&t.state.update(loaded(&view))), 0);
    let with = |f: &dyn Fn(&mut lessdugin_core::dto::PostView)| {
        let mut v = view.clone();
        let ThreadRoot::Post(p) = &mut v.post else {
            panic!("no post")
        };
        p.body.media[0].autoload = true;
        f(p);
        v
    };
    // a collapsed item stays click-to-load, and so does a large image
    let collapsed = with(&|p| {
        p.post.state = ItemState::Collapsed {
            reason: CollapseReason::NewUser,
        }
    });
    assert_eq!(fetches(&t.state.update(loaded(&collapsed))), 0);
    let large = with(&|p| p.body.media[0].size_bytes = 5 * 1024 * 1024 + 1);
    assert_eq!(fetches(&t.state.update(loaded(&large))), 0);
    // a small image of a visible item the engine allows loads by itself, once
    let ok = with(&|_| {});
    assert_eq!(fetches(&t.state.update(loaded(&ok))), 1);
    assert_eq!(
        fetches(&t.state.update(loaded(&ok))),
        0,
        "it is already loading"
    );
}

/// `i` on a comment the fold collapsed loads nothing: its body, its images and the host they
/// load from are folded, so the reader has not seen where the request would go. Space shows
/// them; then `i` loads.
#[test]
fn i_on_a_folded_comment_loads_nothing_until_it_is_expanded() {
    use lessdugin_core::dto::{
        CollapseReason, ItemState, MediaInput, PostDraft, ThreadOpts, ThreadRoot,
    };
    use lessdugin_tui::app::ThreadData;
    use lessdugin_tui::keys::parse_key;
    use lessdugin_tui::{Action, Effect};
    let s = scenario();
    let (bob, carol) = (&s.w.bob, &s.w.carol);
    // an image of bob's, on his PDS (a post uploads it)
    let prepared = block_on(bob.engine.prepare_media(
        png(8, 8),
        &MediaInput {
            declared_mime: Some("image/png".into()),
            alt: None,
            name: None,
            aspect_ratio: None,
        },
    ))
    .unwrap();
    let mut d = PostDraft::article(&s.forum, "Pic", format!("![x](cid:{})", prepared.cid));
    d.body.media.push(prepared);
    let t = block_on(bob.engine.publish(&d)).unwrap();
    bob.flush();
    let post = bob.done(&t);
    carol.sync(&s.forum);
    let pv = block_on(carol.engine.get_thread(&post, &ThreadOpts::default())).unwrap();
    let ThreadRoot::Post(p) = pv.post else {
        panic!("no post")
    };
    let media = p.body.media.clone();
    assert_eq!(media.len(), 1);

    let mut t = common::tui(&s, 80, 40);
    block_on(t.start(None));
    block_on(t.open_thread(&s.welcome));
    let screen = t.state.top().unwrap().id;
    // bob's comment carries the image, and the fold collapsed it (as it does a newcomer's,
    // for other readers)
    let mut view = block_on(carol.engine.get_thread(&s.welcome, &ThreadOpts::default())).unwrap();
    let c = view
        .comments
        .iter_mut()
        .find(|c| c.uri == s.bob_c1)
        .expect("bob's comment");
    c.state = ItemState::Collapsed {
        reason: CollapseReason::NewUser,
    };
    c.body.media = media;
    let bodies = t.state.top_thread().unwrap().data.as_ref().unwrap().bodies.clone();
    let _ = t.state.update(Action::ThreadLoaded {
        screen,
        origin: lessdugin_tui::app::Origin::Live,
        result: Ok(Box::new(ThreadData { view, bodies })),
    });
    let fetches = |fx: &[Effect]| {
        fx.iter()
            .filter(|e| matches!(e, Effect::FetchMedia { .. }))
            .count()
    };
    let key = |t: &mut lessdugin_tui::Headless<lessdugin_testkit::MemHost>, k: &str| {
        t.state.update(Action::Key(parse_key(k).unwrap()))
    };
    // select bob's comment
    while t
        .state
        .top_thread()
        .and_then(|th| th.layout.items.get(th.pos.selected))
        .is_some_and(|i| i.uri != s.bob_c1)
    {
        let _ = key(&mut t, "j");
    }
    assert!(!t.render().contains("i loads it from"), "{}", t.render());
    assert_eq!(fetches(&key(&mut t, "i")), 0, "{}", t.render());
    let st = t.state.status.as_ref().unwrap().text.to_plain_string();
    assert!(st.contains("collapsed"), "{st}");
    // expanded, it says where its image loads from, and `i` loads it
    let _ = key(&mut t, "Space");
    assert!(t.render().contains("i loads it from"), "{}", t.render());
    assert_eq!(fetches(&key(&mut t, "i")), 1);
}
