//! The Apple URLSession network backend: one session (with no shared cache)
//! per event-loop session (network partition key), stored and reused.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use ipc_messages::content::{FetchRequest, FetchResponse};
use ipc_messages::network::ResponseRecipient;
use url_session::UrlSession;
use uuid::Uuid;

use super::{NetworkBackend, NetworkPartitionKey, handle_local_schemes, route_response};

/// <https://fetch.spec.whatwg.org/#http-network-fetch>
pub struct UrlSessionBackend {
    // One NSURLSession with no shared cache per network partition key
    // (event loop id).
    sessions: HashMap<NetworkPartitionKey, UrlSession>,
}

impl UrlSessionBackend {
    pub fn new() -> Self {
        UrlSessionBackend {
            sessions: HashMap::new(),
        }
    }

    /// <https://fetch.spec.whatwg.org/#http-network-fetch>
    fn session_for(&mut self, key: NetworkPartitionKey) -> Result<&UrlSession, String> {
        // The session for the given partition key, creating and storing one
        // on first use.
        match self.sessions.entry(key) {
            Entry::Occupied(entry) => Ok(entry.into_mut()),
            Entry::Vacant(entry) => {
                let session = UrlSession::new()
                    .map_err(|error| format!("failed to create URLSession: {error}"))?;
                Ok(entry.insert(session))
            }
        }
    }
}

impl NetworkBackend for UrlSessionBackend {
    /// <https://fetch.spec.whatwg.org/#http-network-or-cache-fetch>
    fn http_network_or_cache_fetch(
        &mut self,
        key: NetworkPartitionKey,
        request_id: Uuid,
        request: &FetchRequest,
        reply_to: ResponseRecipient,
    ) -> Result<(), String> {
        // Local schemes never touch the transport.
        match handle_local_schemes(request) {
            Ok(Some(response)) => return route_response(request_id, reply_to, Ok(response)),
            Err(error) => return route_response(request_id, reply_to, Err(error)),
            Ok(None) => {}
        }

        let session = match self.session_for(key) {
            Ok(session) => session,
            Err(error) => return route_response(request_id, reply_to, Err(error)),
        };

        let method = request.method.clone();
        let url = request.url.clone();
        let body = request.body.clone();
        let body_bytes = (!body.is_empty()).then_some(body.as_bytes());
        let completion_reply_to = reply_to.clone();
        if let Err(error) = session.fetch(&method, &url, body_bytes, move |result| {
            let result = result.map(|response| FetchResponse {
                final_url: response.final_url,
                status: response.status,
                content_type: response.content_type,
                body: response.body,
            });
            if let Err(route_error) = route_response(request_id, completion_reply_to, result) {
                log::error!("failed to route URLSession response: {route_error}");
            }
        }) {
            // The task could not be started; the completion callback was not
            // invoked, so deliver the failure to the caller now.
            let result = Err(format!("failed to start URLSession fetch: {error}"));
            return route_response(request_id, reply_to, result);
        }
        Ok(())
    }
}

impl Default for UrlSessionBackend {
    fn default() -> Self {
        Self::new()
    }
}
