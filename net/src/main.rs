pub mod backend;

use backend::{Backend, NetworkBackend, NetworkPartitionKey};
use ipc_messages::content::{DocumentFetchId, FetchRequest};
use ipc_messages::network::{Request, Response};
use std::env;

fn net_token_from_args() -> Result<Option<String>, String> {
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--net-token" {
            return args
                .next()
                .map(Some)
                .ok_or_else(|| String::from("missing net token value"));
        }
    }
    Ok(None)
}

pub fn run_net_process_v2(token: String) -> Result<(), String> {
    ipc::run_extension::<Request, Response>(&token, move |server| {
        let request_receiver = ipc::crossbeam_proxy(server.connection.receiver);
        let mut net_backend = Backend::new();

        while let Ok(incoming) = request_receiver.recv() {
            let request = incoming.payload;
            match request {
                Request::SetTraceSender(_) => {}
                Request::Fetch {
                    event_loop_id,
                    request_id,
                    request,
                    reply_to,
                } => {
                    log::debug!("[net] fetch event_loop={event_loop_id} url={}", request.url);
                    let key = NetworkPartitionKey(event_loop_id);
                    if let Err(error) =
                        net_backend.http_network_or_cache_fetch(key, request_id, &request, reply_to)
                    {
                        log::error!("{error}");
                        break;
                    }
                }
                Request::NavigationFetch {
                    event_loop_id,
                    request_id,
                    request,
                    reply_to,
                } => {
                    log::debug!(
                        "[net] navigation fetch event_loop={event_loop_id} url={}",
                        request.url
                    );
                    // Convert NavigationFetchRequest to FetchRequest for HTTP transport.
                    let fetch_request = FetchRequest {
                        handler_id: DocumentFetchId::new(),
                        url: request.url,
                        method: request.method,
                        body: request.body.unwrap_or_default(),
                    };
                    let key = NetworkPartitionKey(event_loop_id);
                    if let Err(error) = net_backend.http_network_or_cache_fetch(
                        key,
                        request_id,
                        &fetch_request,
                        reply_to,
                    ) {
                        log::error!("{error}");
                        break;
                    }
                }
                Request::Shutdown => break,
            }
        }

        Ok(())
    })
}

pub fn run_net_process_from_args() -> Result<(), String> {
    let token = net_token_from_args()?;
    run_net_process_v2(token.unwrap_or_default())
}
