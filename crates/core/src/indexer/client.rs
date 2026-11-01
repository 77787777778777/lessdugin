// SPDX-License-Identifier: AGPL-3.0-or-later

use lessdugin_atp::syntax::{AtUri, Did};

use crate::host::Host;
use crate::limits;
use crate::sync::Session;
use crate::xrpc::{CallOpts, Service, XrpcCallError};

use super::contract::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EndpointPolicy {
    #[default]
    Production,
    Devnet,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Consent {
    pub url: url::Url,
    pub did: Option<Did>,
    pub forum: Option<String>,
}

/// A described, feature-gated indexer. Constructing one is the only operation that may call
/// `describeIndexer`; direct-mode selection happens in `Engine` before this constructor.
#[derive(Clone, Debug)]
pub struct Client {
    service: Service,
    endpoint: url::Url,
    description: Description,
}

fn invalid(message: impl Into<String>) -> XrpcCallError {
    XrpcCallError::InvalidRequest(message.into())
}

pub fn endpoint_allowed(url: &url::Url, policy: EndpointPolicy) -> bool {
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    match policy {
        EndpointPolicy::Production => {
            url.scheme() == "https" && url.port().is_none() && valid_dns_or_ip_host(host)
        }
        EndpointPolicy::Devnet => {
            (url.scheme() == "https" && valid_dns_or_ip_host(host))
                || (url.scheme() == "http" && is_loopback(host))
        }
    }
}

fn valid_dns_or_ip_host(host: &str) -> bool {
    host.parse::<std::net::IpAddr>().is_ok()
        || host == "localhost"
        || (host.len() <= 253
            && host.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                    && !label.starts_with('-')
                    && !label.ends_with('-')
            }))
}

fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

impl Client {
    pub async fn connect<H: Host>(
        session: &Session<H>,
        consent: &Consent,
        policy: EndpointPolicy,
    ) -> Result<Self, XrpcCallError> {
        if !endpoint_allowed(&consent.url, policy) {
            return Err(invalid(format!(
                "indexer endpoint {} is not allowed in {policy:?}",
                consent.url
            )));
        }
        let service = Service::Anon(consent.url.clone());
        let description: Description = session
            .xrpc()
            .query(
                &service,
                &format!("{NSID_PREFIX}describeIndexer"),
                &[],
                &CallOpts::capped(limits::DEFAULT_RESPONSE_MAX_BYTES),
            )
            .await?;
        Did::parse(&description.did)
            .map_err(|e| invalid(format!("describeIndexer returned an invalid DID: {e}")))?;
        if consent
            .did
            .as_ref()
            .is_some_and(|d| d.as_str() != description.did)
        {
            return Err(invalid("describeIndexer DID does not match the consent"));
        }
        if description.features.len() > 64 || description.forums.len() > 1_000 {
            return Err(invalid("describeIndexer exceeds its output limits"));
        }
        Ok(Self {
            service,
            endpoint: consent.url.clone(),
            description,
        })
    }

    pub async fn current_as_of<H: Host>(
        &self,
        session: &Session<H>,
    ) -> Result<AsOf, XrpcCallError> {
        let description: Description = session
            .xrpc()
            .query(
                &self.service,
                &format!("{NSID_PREFIX}describeIndexer"),
                &[],
                &CallOpts::capped(limits::DEFAULT_RESPONSE_MAX_BYTES),
            )
            .await?;
        Ok(description.as_of)
    }

    pub fn endpoint(&self) -> &url::Url {
        &self.endpoint
    }

    pub fn origin(&self) -> String {
        self.endpoint.origin().ascii_serialization()
    }

    pub fn did(&self) -> &str {
        &self.description.did
    }

    pub fn description(&self) -> &Description {
        &self.description
    }

    pub fn supports(&self, method: &str) -> bool {
        let nsid = format!("{NSID_PREFIX}{method}");
        self.description.features.contains(&nsid)
    }

    fn gate(&self, method: &str) -> Result<String, XrpcCallError> {
        self.supports(method)
            .then(|| format!("{NSID_PREFIX}{method}"))
            .ok_or_else(|| invalid(format!("indexer does not advertise {method}")))
    }

    async fn json<H: Host, T: serde::de::DeserializeOwned>(
        &self,
        session: &Session<H>,
        method: &str,
        params: &[(&str, &str)],
    ) -> Result<T, XrpcCallError> {
        let nsid = self.gate(method)?;
        session
            .xrpc()
            .query(
                &self.service,
                &nsid,
                params,
                &CallOpts::capped(limits::DEFAULT_RESPONSE_MAX_BYTES),
            )
            .await
    }

    pub async fn list_forums<H: Host>(
        &self,
        session: &Session<H>,
        query: Option<&str>,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<ForumsPage, XrpcCallError> {
        if !(1..=100).contains(&limit) || query.is_some_and(|q| q.len() > 512) {
            return Err(invalid("invalid listForums parameters"));
        }
        let limit = limit.to_string();
        let mut p = vec![("limit", limit.as_str())];
        if let Some(q) = query {
            p.push(("q", q));
        }
        if let Some(c) = cursor {
            p.push(("cursor", c));
        }
        let page: ForumsPage = self.json(session, "listForums", &p).await?;
        if page.forums.len() > 100 {
            return Err(invalid("listForums page exceeds 100 entries"));
        }
        Ok(page)
    }

    pub async fn list_participants<H: Host>(
        &self,
        session: &Session<H>,
        forum: &AtUri,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<ParticipantsPage, XrpcCallError> {
        if !(1..=1_000).contains(&limit) {
            return Err(invalid("invalid listParticipants limit"));
        }
        let forum = forum.to_string();
        let limit = limit.to_string();
        let mut p = vec![("forum", forum.as_str()), ("limit", limit.as_str())];
        if let Some(c) = cursor {
            p.push(("cursor", c));
        }
        let page: ParticipantsPage = self.json(session, "listParticipants", &p).await?;
        if page.participants.len() > 1_000 {
            return Err(invalid("listParticipants page exceeds 1000 entries"));
        }
        Ok(page)
    }

    pub async fn get_repo_heads<H: Host>(
        &self,
        session: &Session<H>,
        dids: &[Did],
    ) -> Result<RepoHeads, XrpcCallError> {
        if dids.is_empty() || dids.len() > MAX_HEAD_DIDS {
            return Err(invalid("getRepoHeads takes 1..50 DIDs"));
        }
        let params = dids
            .iter()
            .map(|d| ("dids", d.as_str()))
            .collect::<Vec<_>>();
        self.json(session, "getRepoHeads", &params).await
    }

    pub async fn get_changes<H: Host>(
        &self,
        session: &Session<H>,
        forum: Option<&AtUri>,
        since: i64,
        limit: u32,
    ) -> Result<ChangesPage, XrpcCallError> {
        if since < 0 || !(1..=1_000).contains(&limit) {
            return Err(invalid("invalid getChanges parameters"));
        }
        let since = since.to_string();
        let limit = limit.to_string();
        let forum = forum.map(|u| u.to_string());
        let mut p = vec![("since", since.as_str()), ("limit", limit.as_str())];
        if let Some(f) = &forum {
            p.push(("forum", f));
        }
        let page: ChangesPage = self.json(session, "getChanges", &p).await?;
        if page.changes.len() > MAX_CHANGES {
            return Err(invalid("getChanges page exceeds 1000 entries"));
        }
        Ok(page)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn list_posts<H: Host>(
        &self,
        session: &Session<H>,
        forum: &AtUri,
        sort: &str,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<PostsPage, XrpcCallError> {
        if !(1..=100).contains(&limit)
            || !matches!(sort, "magic" | "top" | "new" | "old" | "recentComments")
        {
            return Err(invalid("invalid listPosts parameters"));
        }
        let forum = forum.to_string();
        let limit = limit.to_string();
        let mut p = vec![
            ("forum", forum.as_str()),
            ("sort", sort),
            ("limit", limit.as_str()),
            ("hydrate", "false"),
        ];
        if let Some(c) = cursor {
            p.push(("cursor", c));
        }
        let page: PostsPage = self.json(session, "listPosts", &p).await?;
        if page.posts.len() > MAX_POSTS {
            return Err(invalid("listPosts page exceeds 100 entries"));
        }
        Ok(page)
    }

    pub async fn post_ordering<H: Host>(
        &self,
        session: &Session<H>,
        forum: &AtUri,
        sort: &str,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<OrderingHint, XrpcCallError> {
        let page = self.list_posts(session, forum, sort, limit, cursor).await?;
        Ok(OrderingHint {
            uris: page
                .posts
                .iter()
                .filter(|p| known_post_hint(p))
                .filter_map(|p| p.record.at_uri())
                .collect(),
            cursor: page.cursor,
            input_digest: Some(page.input_digest),
        })
    }

    pub async fn recent_discussion_ordering<H: Host>(
        &self,
        session: &Session<H>,
        forum: &AtUri,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<OrderingHint, XrpcCallError> {
        if !(1..=50).contains(&limit) {
            return Err(invalid("invalid listRecentDiscussion limit"));
        }
        let forum = forum.to_string();
        let limit = limit.to_string();
        let mut p = vec![("forum", forum.as_str()), ("limit", limit.as_str())];
        if let Some(c) = cursor {
            p.push(("cursor", c));
        }
        let page: DiscussionPage = self.json(session, "listRecentDiscussion", &p).await?;
        if page.items.len() > 50 || page.items.iter().any(|i| i.comments.len() > 3) {
            return Err(invalid("listRecentDiscussion exceeds its output limits"));
        }
        Ok(OrderingHint {
            uris: page
                .items
                .iter()
                .filter(|i| known_post_hint(&i.post))
                .filter_map(|i| i.post.record.at_uri())
                .collect(),
            cursor: page.cursor,
            input_digest: None,
        })
    }

    pub async fn thread_ordering<H: Host>(
        &self,
        session: &Session<H>,
        post: &AtUri,
        sort: &str,
    ) -> Result<OrderingHint, XrpcCallError> {
        if !matches!(sort, "magic" | "top" | "new" | "old" | "latestReply") {
            return Err(invalid("invalid getThread sort"));
        }
        let post = post.to_string();
        let page: ThreadHint = self
            .json(
                session,
                "getThread",
                &[("post", &post), ("sort", sort), ("hydrate", "false")],
            )
            .await?;
        if page.comments.len() > 10_000 {
            return Err(invalid("getThread exceeds 10000 comments"));
        }
        let mut uris = Vec::with_capacity(page.comments.len() + 1);
        if known_post_hint(&page.post)
            && let Some(uri) = page.post.record.at_uri()
        {
            uris.push(uri);
        }
        uris.extend(page.comments.iter().filter_map(|c| c.record.at_uri()));
        Ok(OrderingHint {
            uris,
            cursor: None,
            input_digest: Some(page.input_digest),
        })
    }

    pub async fn get_record_proofs<H: Host>(
        &self,
        session: &Session<H>,
        uris: &[AtUri],
    ) -> Result<Vec<u8>, XrpcCallError> {
        if uris.is_empty() || uris.len() > MAX_PROOF_URIS {
            return Err(invalid("getRecordProofs takes 1..25 at-uris"));
        }
        let nsid = self.gate("getRecordProofs")?;
        let params = uris
            .iter()
            .map(|u| ("uris", u.as_str()))
            .collect::<Vec<_>>();
        let qs = lessdugin_atp::xrpc::query_string(&params);
        let resp = session
            .xrpc()
            .get(
                &self.service,
                &format!("/xrpc/{nsid}?{qs}"),
                &CallOpts::capped(limits::INDEXER_BUNDLE_MAX_BYTES)
                    .with_timeout_ms(limits::CAR_TIMEOUT_MS)
                    .with_accept("application/vnd.ipld.car"),
            )
            .await?;
        if resp
            .header("content-type")
            .is_some_and(|v| !v.starts_with("application/vnd.ipld.car"))
        {
            return Err(invalid("getRecordProofs returned a non-CAR content type"));
        }
        Ok(resp.body)
    }
}

fn known_post_hint(post: &PostHint) -> bool {
    post.state
        .as_deref()
        .is_none_or(|s| matches!(s, "visible" | "hidden" | "pending" | "collapsed"))
        && post
            .tier
            .as_deref()
            .is_none_or(|s| matches!(s, "personal" | "frontpage" | "curated"))
}
