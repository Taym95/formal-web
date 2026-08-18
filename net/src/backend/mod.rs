//! Pluggable network backends for the net crate.
//!
//! The [`NetworkBackend`] trait hides the transport behind a single coarse
//! method modelled on the fetch spec's HTTP-network-or-cache fetch. One
//! concrete backend exists per build:
//!
//! - `url_session` — Apple URLSession behind [`url_session::UrlSessionBackend`]
//!   (macOS/iOS only). The default backend on Apple, selected via the
//!   `url_session_default` cfg emitted by build.rs when no backend feature
//!   is enabled; wins over `tokio` when both are enabled.
//! - `tokio` — reqwest (blocking) behind [`tokio::TokioBackend`]. The
//!   default (and only) backend on non-Apple platforms; opt-in on Apple.
//!
//! Sessions are partitioned by event loop: each backend keeps one session
//! (a reqwest client, or an NSURLSession with no shared cache) per
//! [`NetworkPartitionKey`], the event loop id of the fetching event loop.
//! Fetch outcomes are delivered on the [`FetchReplySender`] channel passed
//! to each fetch call, so a backend may deliver a reply at any time after
//! its fetch method returns: the tokio backend sends it before returning,
//! the URLSession backend sends it from the data task's completion handler,
//! on a background queue. The net process main loop receives those replies
//! and routes them to the request's reply_to recipient.

use ipc_messages::content::{EventLoopId, FetchRequest, FetchResponse};
use std::fs;
use url::Url;
use uuid::Uuid;

/// The outcome of a fetch: the request id the outcome belongs to, and the
/// response or error. Delivered on a [`FetchReplySender`].
pub type FetchReply = (Uuid, Result<FetchResponse, String>);

/// Reply channel handed to the backends for a fetch; the main loop routes
/// the delivered outcome to the request's reply_to recipient.
pub type FetchReplySender = crossbeam_channel::Sender<FetchReply>;

/// <https://fetch.spec.whatwg.org/#network-partition-key>
// Note: the spec key is a (topLevelSite, secondKey) tuple; this
// implementation uses the event loop id of the fetching event loop alone
// as the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NetworkPartitionKey(pub EventLoopId);

/// The network mechanism behind the net process: executes fetches and
/// delivers the outcomes on the reply channel passed to each call. One
/// implementation exists per build feature.
pub trait NetworkBackend {
    /// <https://fetch.spec.whatwg.org/#http-network-or-cache-fetch>
    // Note: coarse-grained — the backend runs the fetch for the request and
    // delivers the outcome on `reply_sender`; the individual steps of the
    // algorithm are not implemented.
    fn http_network_or_cache_fetch(
        &mut self,
        network_partition_key: NetworkPartitionKey,
        request_id: Uuid,
        request: &FetchRequest,
        reply_sender: FetchReplySender,
    ) -> Result<(), String>;

    /// <https://fetch.spec.whatwg.org/#fetch-controller-abort>
    // Note: not implemented — the backends do not yet track in-flight
    // fetches to abort.
    fn abort(&mut self, _request_id: Uuid) {
        unimplemented!("NetworkBackend::abort")
    }
}

/// <https://fetch.spec.whatwg.org/#scheme-fetch>
fn handle_local_schemes(request: &FetchRequest) -> Result<Option<FetchResponse>, String> {
    // Ok(Some(response)) when the request was handled locally, Ok(None)
    // when it must go to the transport, Err(..) when the request could not
    // be processed at all.
    let parsed = Url::parse(&request.url).map_err(|error| format!("invalid URL: {error}"))?;
    if parsed.scheme() == "file" {
        return fetch_file_url(&request.url).map(Some);
    }
    if parsed.scheme() == "about" && parsed.path() == "blank" {
        return Ok(Some(FetchResponse {
            final_url: String::from("about:blank"),
            status: 200,
            content_type: String::from("text/html; charset=utf-8"),
            body: Vec::new(),
        }));
    }
    Ok(None)
}

/// <https://fetch.spec.whatwg.org/#scheme-fetch>
fn fetch_file_url(url: &str) -> Result<FetchResponse, String> {
    let parsed = Url::parse(url).map_err(|error| format!("invalid file URL: {error}"))?;
    let path = parsed
        .to_file_path()
        .map_err(|_| String::from("failed to convert file URL to local path"))?;
    let body = fs::read(&path).map_err(|error| format!("failed to read file URL body: {error}"))?;
    let content_type = mime_guess::from_path(&path)
        .first_raw()
        .unwrap_or("application/octet-stream")
        .to_owned();
    Ok(FetchResponse {
        final_url: url.to_owned(),
        status: 200,
        content_type,
        body,
    })
}

// ── Backend selection ───────────────────────────────────────────────────────
//
// Exactly one backend exists per build. On Apple, the URLSession backend is
// the default (`url_session_default` from build.rs) and wins over `tokio`
// when both are available; on other platforms the tokio backend is always
// compiled.

// tokio: always compiled on non-Apple platforms (reqwest is non-optional
// there); on Apple, gated by the tokio feature.
#[cfg(any(feature = "tokio", not(target_vendor = "apple")))]
pub mod tokio;

#[cfg(all(
    any(feature = "url_session", url_session_default),
    target_vendor = "apple"
))]
pub mod url_session;

#[cfg(all(
    any(feature = "url_session", url_session_default),
    target_vendor = "apple"
))]
pub type Backend = url_session::UrlSessionBackend;

#[cfg(all(
    not(all(
        any(feature = "url_session", url_session_default),
        target_vendor = "apple"
    )),
    any(feature = "tokio", not(target_vendor = "apple"))
))]
pub type Backend = tokio::TokioBackend;

#[cfg(all(
    not(all(
        any(feature = "url_session", url_session_default),
        target_vendor = "apple"
    )),
    not(any(feature = "tokio", not(target_vendor = "apple")))
))]
compile_error!("net requires at least one network backend: `tokio` or `url_session`");
