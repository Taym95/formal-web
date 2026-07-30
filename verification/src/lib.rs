mod monitor;
mod session;
mod tracer;
mod types;
mod validate;

pub use monitor::TraceMonitor;
pub use session::VerificationRun;
pub use tracer::TLATracer;
pub use types::{LogEntry, TraceSender, VarUpdate};
pub use validate::{ValidationOptions, run_validation_from_iter, validate_and_print};

#[macro_export]
macro_rules! tla_log {
    // tla_log!(tracer)
    ($tracer:expr) => {{
        let tracer = &mut $tracer;
        if tracer.is_enabled() {
            tracer.log_silent_with_location(None, file!(), line!());
        }
    }};
    // tla_log!(tracer, "Event", args...)
    ($tracer:expr, $event:expr $(, $arg:expr)* $(,)?) => {{
        let tracer = &mut $tracer;
        if tracer.is_enabled() {
            tracer.log_with_location(
                None,
                $event,
                vec![$(::std::string::ToString::to_string(&$arg)),*],
                file!(),
                line!(),
            );
        }
    }};
    // tla_log!(tracer -> "Spec", "Event", args...)
    ($tracer:expr, -> $spec:expr, $event:expr $(, $arg:expr)* $(,)?) => {{
        let tracer = &mut $tracer;
        if tracer.is_enabled() {
            tracer.log_with_location(
                Some($spec),
                $event,
                vec![$(::std::string::ToString::to_string(&$arg)),*],
                file!(),
                line!(),
            );
        }
    }};
}
