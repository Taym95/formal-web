use ipc_messages::graphics::{GraphicsCommand, GraphicsEvent};
use media::backend::MediaBackend;

fn main() {
    env_logger::init();
    log::info!("[graphics] starting graphics and media process");

    let token = {
        let mut args = std::env::args().skip(1);
        let mut found = None;
        while let Some(arg) = args.next() {
            if arg == "--graphics-token" {
                found = args.next();
                break;
            }
            if let Some(val) = arg.strip_prefix("--graphics-token=") {
                found = Some(val.to_owned());
                break;
            }
        }
        found.unwrap_or_default()
    };

    let result = ipc::run_extension::<GraphicsCommand, GraphicsEvent>(&token, |server| {
        let receiver = ipc::crossbeam_proxy(server.connection.receiver);
        let event_tx = server.connection.sender.clone();

        // Initialize the media backend: AVFoundation on Apple platforms
        // when it is the active backend (explicit feature, or the default
        // when no backend feature is selected); GStreamer otherwise.
        #[cfg(all(
            any(target_os = "macos", target_os = "ios"),
            any(feature = "backend-avfoundation", not(feature = "backend-gstreamer"))
        ))]
        let backend: Option<media::backend::avfoundation::AvfBackend> =
            match media::backend::avfoundation::AvfBackend::init() {
                Ok(b) => {
                    log::info!("[graphics] AVFoundation backend initialized");
                    Some(b)
                }
                Err(e) => {
                    log::error!("[graphics] AVFoundation init failed: {e}");
                    None
                }
            };

        #[cfg(not(all(
            any(target_os = "macos", target_os = "ios"),
            any(feature = "backend-avfoundation", not(feature = "backend-gstreamer"))
        )))]
        let backend: Option<media::backend::gstreamer::GStreamerBackend> =
            match media::backend::gstreamer::GStreamerBackend::init() {
                Ok(b) => {
                    log::info!("[graphics] GStreamer backend initialized");
                    Some(b)
                }
                Err(e) => {
                    log::error!("[graphics] GStreamer init failed: {e}");
                    None
                }
            };

        // The surface renderer backend is chosen at compile time by feature:
        // the zero-copy IOSurface renderer on macOS by default, the CPU
        // readback renderer off macOS and with `cpu_readback`.
        #[cfg(all(target_os = "macos", not(feature = "cpu_readback")))]
        graphics::run_graphics_process::<_, graphics::renderer::IosurfaceRenderer>(
            receiver, event_tx, backend,
        );
        #[cfg(any(not(target_os = "macos"), feature = "cpu_readback"))]
        graphics::run_graphics_process::<_, graphics::renderer::CpuRenderer>(
            receiver, event_tx, backend,
        );
        Ok(())
    });
    if let Err(error) = result {
        log::error!("[graphics] extension exited with error: {error}");
    }
}
