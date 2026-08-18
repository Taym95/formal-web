use crate::content::{Command, DocumentFetchId, EventLoopId, FetchRequest, FetchResponse};
use ipc::IpcSender;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use verification::TraceSender;

/// A navigation fetch request initiated by the user agent.
/// Distinct from content-initiated document fetches (FetchRequest).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavigationFetchRequest {
    /// <https://fetch.spec.whatwg.org/#concept-request-url>
    pub url: String,
    /// <https://fetch.spec.whatwg.org/#concept-request-method>
    pub method: String,
    /// <https://fetch.spec.whatwg.org/#concept-request-body>
    pub body: Option<String>,
    /// <https://fetch.spec.whatwg.org/#concept-request-referrer>
    pub referrer: String,
    /// <https://fetch.spec.whatwg.org/#concept-request-referrer-policy>
    pub referrer_policy: String,
}

/// Specifies how net should route the fetch response back to the caller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ResponseRecipient {
    /// Net sends Command::CompleteDocumentFetch to the content process's command sender.
    ContentProcess {
        content_command_sender: IpcSender<Command>,
        handler_id: DocumentFetchId,
    },
    /// Net sends Response through the embedded sender — the user agent's
    /// event loop channel. Modelled on the fetch spec's parallel queue: the
    /// UA channel is where fetch responses are delivered.
    /// <https://fetch.spec.whatwg.org/#fetch-useparallelqueue>
    UserAgent {
        response_sender: IpcSender<Response>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    SetTraceSender(Option<TraceSender>),
    Fetch {
        /// The event loop (content process) that initiated the fetch.
        event_loop_id: EventLoopId,
        request_id: Uuid,
        request: FetchRequest,
        reply_to: ResponseRecipient,
    },
    NavigationFetch {
        /// The event loop that owns the navigable being navigated.
        event_loop_id: EventLoopId,
        request_id: Uuid,
        request: NavigationFetchRequest,
        reply_to: ResponseRecipient,
    },
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub request_id: Uuid,
    pub result: Result<FetchResponse, String>,
}
