// SPDX-License-Identifier: AGPL-3.0-or-later
//! Unauthenticated `NS.index.*` XRPC service. Responses are hint-shaped DTOs, not client DTO
//! bytes; the only authoritative content crossing this API is independently verifiable CAR proof.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{ConnectInfo, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode, header};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use lessdugin_atp::syntax::{AtUri, Datetime, Did};
use lessdugin_core::discovery::ForumKey;
use lessdugin_core::dto::{self, CommentSort, ItemState, PostFilter, PostSort, Tier};
use lessdugin_core::host::Host;
use lessdugin_core::model::ForumModel;
use lessdugin_core::model::render::RenderCache;
use lessdugin_core::model::views::{Viewer, Views, post_filter, post_sort};
use lessdugin_core::social::full::query::{PostQuery, Query};
use lessdugin_core::store::records::RecordStore;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::time::Instant;
use tower_http::cors::{Any, CorsLayer};

use crate::store::ForumRow;
use crate::{FEATURES, Indexer, ingest, proofs};

type Result<T> = std::result::Result<T, ApiError>;

pub struct ApiError {
    status: StatusCode,
    name: &'static str,
    message: String,
}
impl ApiError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            name: "InvalidRequest",
            message: message.into(),
        }
    }
    fn not_indexed(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            name: "NotIndexed",
            message: message.into(),
        }
    }
    fn too_old() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            name: "CursorTooOld",
            message: "the requested cursor predates the 30-day change log".into(),
        }
    }
    fn internal(e: impl std::fmt::Display) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            name: "InternalError",
            message: e.to_string(),
        }
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({"error":self.name,"message":self.message})),
        )
            .into_response()
    }
}

async fn rate_limit(State(indexer): State<Indexer>, req: Request<Body>, next: Next) -> Response {
    indexer.metrics.requests.fetch_add(1, Ordering::Relaxed);
    let ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip().to_string())
        .unwrap_or_else(|| "local".into());
    let allowed = {
        let mut windows = indexer
            .rate_windows
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        windows.retain(|_, (start, _)| now.duration_since(*start) < Duration::from_secs(2));
        let (start, count) = windows.entry(ip).or_insert((now, 0));
        if now.duration_since(*start) >= Duration::from_secs(1) {
            *start = now;
            *count = 0;
        }
        *count += 1;
        *count <= 20
    };
    if !allowed {
        indexer.metrics.rate_limited.fetch_add(1, Ordering::Relaxed);
        return (StatusCode::TOO_MANY_REQUESTS,[(header::RETRY_AFTER,"1")],Json(json!({"error":"RateLimitExceeded","message":"at most 20 requests per second per IP"}))).into_response();
    }
    next.run(req).await
}

pub fn router(indexer: Indexer) -> Router {
    let base = format!("/xrpc/{}.index", crate::NS);
    Router::new()
        .route("/", get(source))
        .route("/_health", get(health))
        .route("/metrics", get(metrics))
        .route(&format!("{base}.describeIndexer"), get(describe))
        .route(&format!("{base}.listForums"), get(list_forums))
        .route(&format!("{base}.listParticipants"), get(list_participants))
        .route(&format!("{base}.getRepoHeads"), get(get_repo_heads))
        .route(&format!("{base}.getChanges"), get(get_changes))
        .route(&format!("{base}.listPosts"), get(list_posts))
        .route(
            &format!("{base}.listRecentDiscussion"),
            get(recent_discussion),
        )
        .route(&format!("{base}.getThread"), get(get_thread))
        .route(&format!("{base}.getRecordProofs"), get(get_record_proofs))
        .layer(from_fn_with_state(indexer.clone(), rate_limit))
        .with_state(indexer)
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([axum::http::Method::GET])
                .allow_headers(Any),
        )
}

async fn source(State(i): State<Indexer>) -> Response {
    (StatusCode::OK,[(header::CONTENT_TYPE,"text/html; charset=utf-8")],format!("<!doctype html><html><head><title>lessdugin indexer</title></head><body><h1>lessdugin indexer {}</h1><p>Optional hint and proof service. All non-proof responses are untrusted hints.</p><p>Source and corresponding AGPLv3 source: <a href=\"{}\">{}</a></p><p><a href=\"/_health\">health</a> · <a href=\"/metrics\">metrics</a></p></body></html>",env!("CARGO_PKG_VERSION"),html(&i.config.source_url),html(&i.config.source_url))).into_response()
}
fn html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
async fn health(State(i): State<Indexer>) -> Json<Value> {
    Json(
        json!({"ok":true,"version":env!("CARGO_PKG_VERSION"),"forums":i.store.forum_count(),"latestSeq":i.store.latest_seq()}),
    )
}
async fn metrics(State(i): State<Indexer>) -> Response {
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        i.metrics.render(true),
    )
        .into_response()
}

fn params(raw: Option<String>) -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    if let Some(raw) = raw {
        for (k, v) in url::form_urlencoded::parse(raw.as_bytes()) {
            out.entry(k.into_owned()).or_default().push(v.into_owned());
        }
    }
    out
}
fn one<'a>(p: &'a HashMap<String, Vec<String>>, name: &str) -> Option<&'a str> {
    p.get(name).and_then(|v| v.first()).map(String::as_str)
}
fn required<'a>(p: &'a HashMap<String, Vec<String>>, name: &str) -> Result<&'a str> {
    one(p, name)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::invalid(format!("missing {name}")))
}
fn many(p: &HashMap<String, Vec<String>>, name: &str) -> Vec<String> {
    p.get(name)
        .into_iter()
        .flatten()
        .flat_map(|s| s.split(','))
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}
fn limit(p: &HashMap<String, Vec<String>>, default: usize, max: usize) -> Result<usize> {
    let n = one(p, "limit")
        .map(|s| {
            s.parse::<usize>()
                .map_err(|_| ApiError::invalid("bad limit"))
        })
        .transpose()?
        .unwrap_or(default);
    if !(1..=max).contains(&n) {
        return Err(ApiError::invalid(format!("limit must be 1..={max}")));
    }
    Ok(n)
}
fn offset(p: &HashMap<String, Vec<String>>) -> Result<usize> {
    one(p, "cursor")
        .map(|c| c.parse().map_err(|_| ApiError::invalid("bad cursor")))
        .transpose()
        .map(|v| v.unwrap_or(0))
}
/// Drops the null members of a JSON object: absent optional fields are left out, never null
/// (`data-model.md` §11.3; lexicon strings are not nullable).
fn compact(mut value: Value) -> Value {
    if let Some(map) = value.as_object_mut() {
        map.retain(|_, v| !v.is_null());
    }
    value
}
fn page<T: Clone>(items: &[T], offset: usize, limit: usize) -> (Vec<T>, Option<String>) {
    let v = items
        .iter()
        .skip(offset)
        .take(limit)
        .cloned()
        .collect::<Vec<_>>();
    let end = offset + v.len();
    (v, (end < items.len()).then(|| end.to_string()))
}
fn bool_param(p: &HashMap<String, Vec<String>>, name: &str) -> Result<bool> {
    match one(p, name) {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        _ => Err(ApiError::invalid(format!("bad {name}"))),
    }
}

fn response(i: &Indexer, value: Value, list: bool, headers: HeaderMap) -> Response {
    let value = i.with_meta(value);
    let bytes = serde_json::to_vec(&value).unwrap_or_else(|_| b"{}".to_vec());
    let etag = format!("\"{:x}\"", Sha256::digest(&bytes));
    let mut out = if list
        && headers
            .get(header::IF_NONE_MATCH)
            .and_then(|v| v.to_str().ok())
            == Some(etag.as_str())
    {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            bytes,
        )
            .into_response()
    };
    if list {
        out.headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("max-age=5"));
        out.headers_mut()
            .insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    }
    out
}
fn json_result(i: &Indexer, value: Result<Value>, list: bool, headers: HeaderMap) -> Response {
    match value {
        Ok(v) => response(i, v, list, headers),
        Err(e) => e.into_response(),
    }
}

async fn describe(State(i): State<Indexer>, headers: HeaderMap) -> Response {
    let forums=i.store.indexed_forums(None).into_iter().filter(|f|!i.blocked_forum(f)).take(1000).map(|f|json!({"uri":f.uri,"lagMs":(i.now_ms()-f.synced_ms).max(0),"backfilled":f.synced_ms>0})).collect::<Vec<_>>();
    json_result(
        &i,
        Ok(
            json!({"did":i.config.did,"version":env!("CARGO_PKG_VERSION"),"coreVersion":env!("CARGO_PKG_VERSION"),"policies":["karma/1","magic/1","proofs/1"],"features":FEATURES,"forums":forums}),
        ),
        true,
        headers,
    )
}

async fn list_forums(
    State(i): State<Indexer>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Response {
    let result = (|| {
        let p = params(raw);
        let (rows, cursor) = page(
            &i.store
                .indexed_forums(one(&p, "q"))
                .into_iter()
                .filter(|f| !i.blocked_forum(f))
                .collect::<Vec<_>>(),
            offset(&p)?,
            limit(&p, 50, 100)?,
        );
        let forums=rows.into_iter().map(|f|json!({"ref":compact(json!({"uri":f.uri,"cid":f.cid,"firstSeen":(!f.created_at.is_empty()).then_some(f.created_at)})),"participants":f.participants,"posts":f.posts})).collect::<Vec<_>>();
        Ok(compact(json!({"forums":forums,"cursor":cursor})))
    })();
    json_result(&i, result, true, headers)
}

async fn list_participants(
    State(i): State<Indexer>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Response {
    let result = (|| {
        let p = params(raw);
        let forum = required(&p, "forum")?;
        known_forum(&i, forum)?;
        let (rows, cursor) = page(
            &i.store
                .participants(forum, one(&p, "status"))
                .into_iter()
                .filter(|p| !i.blocked_did(&p.did))
                .collect::<Vec<_>>(),
            offset(&p)?,
            limit(&p, 500, 1000)?,
        );
        let participants=rows.into_iter().map(|p|compact(json!({"did":p.did,"status":p.status,"rev":p.rev,"lastSeen":dt_ms(p.last_seen_ms)}))).collect::<Vec<_>>();
        Ok(compact(
            json!({"participants":participants,"cursor":cursor}),
        ))
    })();
    json_result(&i, result, true, headers)
}

async fn get_repo_heads(
    State(i): State<Indexer>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Response {
    let result = (|| {
        let p = params(raw);
        let dids = many(&p, "dids");
        if dids.is_empty() || dids.len() > 50 {
            return Err(ApiError::invalid("dids requires 1..=50 entries"));
        }
        for d in &dids {
            Did::parse(d).map_err(|e| ApiError::invalid(e.to_string()))?;
        }
        // A blocked host hides the repository entirely, like its records (INFRA-12).
        let heads=i.store.heads(&dids).into_iter().filter(|h|!i.blocked(&h.did)&&!h.pds.as_deref().is_some_and(|pds|i.blocked_host(pds))).map(|h|compact(json!({"did":h.did,"rev":h.rev,"commit":h.commit,"pds":h.pds,"checkedAt":dt_ms(h.checked_ms),"status":h.status}))).collect::<Vec<_>>();
        Ok(json!({"heads":heads}))
    })();
    json_result(&i, result, true, headers)
}

async fn get_changes(
    State(i): State<Indexer>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Response {
    let result = (|| {
        let p = params(raw);
        let since = required(&p, "since")?
            .parse::<i64>()
            .map_err(|_| ApiError::invalid("bad since"))?;
        let forum = one(&p, "forum");
        if let Some(f) = forum {
            known_forum(&i, f)?;
        }
        let oldest = i.store.oldest_seq();
        if since < i.store.change_floor() || (oldest > 0 && since < oldest - 1) {
            return Err(ApiError::too_old());
        }
        let rows = i.store.changes(forum, since, limit(&p, 500, 1000)?);
        // The cursor describes the consumed source page, not the last visible entry. A blocked
        // final entry must not leave clients polling the same page forever.
        let cursor = rows.last().map_or(since, |c| c.seq);
        let changes = rows
            .into_iter()
            .filter(|c| !i.blocked_change(c))
            .map(|c| {
                compact(
                    json!({"seq":c.seq,"uri":c.uri,"cid":c.cid,"did":c.did,"rev":c.rev,"op":c.op}),
                )
            })
            .collect::<Vec<_>>();
        Ok(json!({"changes":changes,"cursor":cursor}))
    })();
    json_result(&i, result, true, headers)
}

/// An indexed forum: one primary-key lookup, never a scan of the forum table.
fn known_forum(i: &Indexer, forum: &str) -> Result<(ForumKey, ForumRow)> {
    if i.blocked(forum) {
        return Err(ApiError::not_indexed("forum is not indexed"));
    }
    let uri = AtUri::parse(forum).map_err(|e| ApiError::invalid(e.to_string()))?;
    let Some(row) = i.store.forum(forum).filter(|row| !i.blocked_forum(row)) else {
        return Err(ApiError::not_indexed("forum is not indexed"));
    };
    let key = ForumKey::of_uri(&uri).map_err(|e| ApiError::invalid(e.to_string()))?;
    Ok((key, row))
}
/// The indexed forum whose key range holds `uri` (an indexed lookup by its fk).
fn forum_of(i: &Indexer, uri: &AtUri) -> Result<(ForumKey, ForumRow)> {
    let rkey = uri.rkey().as_str();
    if let Some(fk) = lessdugin_lexicon::keys::fk_of(rkey)
        && rkey[fk.len()..].starts_with('-')
    {
        for f in i.store.forums_by_fk(fk) {
            if let Ok(found) = known_forum(i, &f) {
                return Ok(found);
            }
        }
    }
    Err(ApiError::not_indexed("the record's forum is not indexed"))
}
/// The forum's projected model, cached between projections (never refolded per request).
async fn model(i: &Indexer, key: &ForumKey) -> Result<std::sync::Arc<ForumModel>> {
    ingest::model(i, key).await.map_err(ApiError::internal)
}
fn views<'a>(
    m: &'a ForumModel,
    values: &'a BTreeMap<String, Value>,
    empty: &'a Empty,
) -> Views<'a> {
    Views {
        model: m,
        actors: &empty.actors,
        pds: &empty.pds,
        values,
        renders: &empty.renders,
        pending: &empty.pending,
        completeness: &empty.completeness,
        viewer: &empty.viewer,
    }
}
/// What hint views need besides the model: no actor, PDS, viewer or render state.
#[derive(Default)]
struct Empty {
    actors: BTreeMap<String, dto::ActorView>,
    pds: BTreeMap<String, String>,
    renders: RenderCache,
    pending: BTreeSet<String>,
    completeness: dto::Completeness,
    viewer: Viewer,
}
fn parse_enum<T: serde::de::DeserializeOwned>(
    p: &HashMap<String, Vec<String>>,
    name: &str,
    default: T,
) -> Result<T> {
    one(p, name)
        .map(|s| {
            serde_json::from_value(Value::String(s.into()))
                .map_err(|_| ApiError::invalid(format!("bad {name}")))
        })
        .transpose()
        .map(|v| v.unwrap_or(default))
}
fn dt_ms(ms: i64) -> Option<String> {
    Datetime::from_micros(ms.saturating_mul(1000))
        .ok()
        .map(|d| d.to_string())
}

fn tally(subject: &str, score: &dto::ScoreView) -> Value {
    json!({"subject":subject,"policy":"karma/1","score":score.score,"agreement":score.agreement,"unweighted":score.unweighted,"reacts":[]})
}
fn state(s: &ItemState) -> &'static str {
    match s {
        ItemState::Visible => "visible",
        ItemState::Collapsed { .. } => "collapsed",
        ItemState::Hidden { .. } | ItemState::Tombstone { .. } => "hidden",
    }
}
fn tier(t: &Tier) -> &'static str {
    match t {
        Tier::Personal => "personal",
        Tier::Frontpage => "frontpage",
        Tier::Curated => "curated",
    }
}
fn post_hint(p: &dto::PostSummary, value: Option<Value>, hydrate: bool) -> Value {
    let mut ref_ = compact(json!({"uri":p.uri,"cid":p.cid,"firstSeen":dt_ms(p.created_at_ms)}));
    if hydrate && let Some(v) = value {
        ref_["value"] = v;
    }
    compact(
        json!({"ref":ref_,"tally":tally(&p.uri,&p.score),"state":state(&p.state),"tier":tier(&p.tier),"tags":p.tags,"comments":p.comment_count,"lastCommentAt":p.last_comment_at_ms.and_then(dt_ms),"effectiveAt":dt_ms(p.t_rank_ms)}),
    )
}
fn comment_hint(c: &dto::CommentNode, hydrate: bool, value: Option<Value>) -> Value {
    let mut ref_ = compact(json!({"uri":c.uri,"cid":c.cid,"firstSeen":dt_ms(c.created_at_ms)}));
    if hydrate && let Some(v) = value {
        ref_["value"] = v;
    }
    compact(
        json!({"ref":ref_,"parent":c.parent,"depth":c.depth,"tally":tally(&c.uri,&c.score),"state":state(&c.state)}),
    )
}
async fn stored_value(i: &Indexer, uri: &str) -> Option<Value> {
    let u = AtUri::parse(uri).ok()?;
    RecordStore::new(i.engine.host().cache())
        .get(&u)
        .await
        .ok()
        .flatten()
        .and_then(|r| lessdugin_atp::datamodel::record_cbor_to_json(&r.cbor).ok())
}

async fn list_posts(
    State(i): State<Indexer>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Response {
    let result = async {
        let p = params(raw);
        let forum = required(&p, "forum")?;
        let (key, row) = known_forum(&i, forum)?;
        let sort: PostSort = parse_enum(&p, "sort", PostSort::Magic)?;
        let filter = match one(&p, "tier") {
            Some("frontpage") => PostFilter::Frontpage,
            Some("curated") => PostFilter::Curated,
            Some("personal") | None => PostFilter::All,
            _ => return Err(ApiError::invalid("bad tier")),
        };
        if let Some(author) = one(&p, "author") {
            Did::parse(author).map_err(|e| ApiError::invalid(e.to_string()))?;
        }
        let kind = one(&p, "kind")
            .map(|k| {
                serde_json::from_value::<dto::PostKind>(Value::String(k.into()))
                    .map_err(|_| ApiError::invalid("bad kind"))
                    .map(|_| k.to_owned())
            })
            .transpose()?;
        let hydrate = bool_param(&p, "hydrate")?;
        let (offset, limit) = (offset(&p)?, limit(&p, 25, 100)?);
        // Views come from the cached projection: a request never refolds the forum.
        let m = model(&i, &key).await?;
        let query = Query {
            thread: None,
            posts: Some(PostQuery {
                sort: post_sort(sort),
                filter: post_filter(filter),
                tag: one(&p, "tag").map(str::to_owned),
                kind,
                author: one(&p, "author").map(str::to_owned),
            }),
            comments: None,
        };
        let order = m
            .orderings(&query)
            .map_err(|e| ApiError::internal(format!("the fold layer is pending: {e}")))?
            .post_order
            .unwrap_or_default();
        let (uris, cursor) = page(&order, offset, limit);
        let (values, empty) = (BTreeMap::new(), Empty::default());
        let summaries = {
            let v = views(&m, &values, &empty);
            uris.iter()
                .filter_map(|u| v.post_summary(u))
                .collect::<Vec<_>>()
        };
        let mut posts = Vec::new();
        for post in summaries {
            if i.blocked_record(&row, &post.uri) || i.blocked(&post.author.did) {
                continue;
            }
            let value = if hydrate {
                stored_value(&i, &post.uri).await
            } else {
                None
            };
            posts.push(post_hint(&post, value, hydrate));
        }
        Ok(compact(
            json!({"posts":posts,"cursor":cursor,"policy":{"karma":"karma/1","ranking":"magic/1"},"inputDigest":m.view.input_digest}),
        ))
    }
    .await;
    json_result(&i, result, true, headers)
}

async fn recent_discussion(
    State(i): State<Indexer>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Response {
    let result = async {
        let p = params(raw);
        let forum = required(&p, "forum")?;
        let (key, row) = known_forum(&i, forum)?;
        let (offset, limit) = (offset(&p)?, limit(&p, 20, 50)?);
        let m = model(&i, &key).await?;
        let entries = m
            .view
            .orderings
            .as_ref()
            .map_err(|e| ApiError::internal(format!("the fold layer is pending: {e}")))?
            .recent_discussion
            .clone();
        let (page, cursor) = page(&entries, offset, limit);
        let (values, empty) = (BTreeMap::new(), Empty::default());
        let v = views(&m, &values, &empty);
        let mut items = Vec::new();
        for entry in page {
            let Some(post) = v.post_summary(&entry.post) else {
                continue;
            };
            if i.blocked_record(&row, &post.uri) || i.blocked(&post.author.did) {
                continue;
            }
            let comments = entry
                .comments
                .iter()
                .take(lessdugin_core::limits::DISCUSSION_COMMENTS)
                .filter_map(|c| v.comment_node(c, &entry.post))
                .filter(|c| !i.blocked_record(&row, &c.uri) && !i.blocked(&c.author.did))
                .map(|c| comment_hint(&c, false, None))
                .collect::<Vec<_>>();
            items.push(json!({"post":post_hint(&post,None,false),"comments":comments}));
        }
        Ok(compact(json!({"items":items,"cursor":cursor})))
    }
    .await;
    json_result(&i, result, true, headers)
}

async fn get_thread(
    State(i): State<Indexer>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Response {
    let result = async {
        let p = params(raw);
        let post = AtUri::parse(required(&p, "post")?)
            .map_err(|e| ApiError::invalid(e.to_string()))?;
        if i.blocked(post.as_str()) {
            return Err(ApiError::not_indexed("post is not indexed"));
        }
        let (key, row) = forum_of(&i, &post)?;
        let sort: CommentSort = parse_enum(&p, "sort", CommentSort::Magic)?;
        let hydrate = bool_param(&p, "hydrate")?;
        let m = model(&i, &key).await?;
        let root = m
            .comment_root(post.as_str())
            .unwrap_or(post.as_str())
            .to_owned();
        let mut values = BTreeMap::new();
        if let Some(v) = stored_value(&i, &root).await {
            values.insert(root.clone(), v);
        }
        let empty = Empty::default();
        let thread = views(&m, &values, &empty).thread_view(&root, sort);
        let dto::ThreadRoot::Post(post_view) = thread.post else {
            return Err(ApiError::not_indexed("thread root is not indexed"));
        };
        if i.blocked_record(&row, &post_view.post.uri) || i.blocked(&post_view.post.author.did) {
            return Err(ApiError::not_indexed("post is not indexed"));
        }
        let value = if hydrate {
            stored_value(&i, &post_view.post.uri).await
        } else {
            None
        };
        let post_out = post_hint(&post_view.post, value, hydrate);
        let truncated = thread.comments.len() > 10000;
        let mut comments = Vec::new();
        for c in thread.comments.into_iter().take(10000) {
            if i.blocked_record(&row, &c.uri) || i.blocked(&c.author.did) {
                continue;
            }
            let value = if hydrate {
                stored_value(&i, &c.uri).await
            } else {
                None
            };
            comments.push(comment_hint(&c, hydrate, value));
        }
        Ok(
            json!({"post":post_out,"comments":comments,"truncated":truncated,"inputDigest":m.view.input_digest}),
        )
    }
    .await;
    json_result(&i, result, false, headers)
}

async fn get_record_proofs(State(i): State<Indexer>, RawQuery(raw): RawQuery) -> Result<Response> {
    let p = params(raw);
    let uris = many(&p, "uris");
    if uris.is_empty() || uris.len() > 25 {
        return Err(ApiError::invalid("uris requires 1..=25 entries"));
    }
    for u in &uris {
        AtUri::parse(u).map_err(|e| ApiError::invalid(e.to_string()))?;
    }
    let car = proofs::bundle(&i, &uris)
        .await
        .map_err(ApiError::internal)?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/vnd.ipld.car"),
            (header::CACHE_CONTROL, "max-age=5"),
        ],
        car,
    )
        .into_response())
}
