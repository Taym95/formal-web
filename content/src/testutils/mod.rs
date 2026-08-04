//! <https://testutils.spec.whatwg.org/#the-testutils-namespace>
//!
//! The `TestUtils` namespace provides testing-only APIs (garbage collection
//! trigger, etc.) exposed on `Window` and `Worker` globals for WPT and
//! developer tooling.

/// <https://testutils.spec.whatwg.org/#the-testutils-namespace>
pub(crate) struct TestUtils;

impl TestUtils {
    /// <https://testutils.spec.whatwg.org/#dom-testutils-gc>
    ///
    /// Perform a garbage collection covering at least the entry realm.
    /// Returns a promise that resolves after GC completes.
    pub(crate) fn gc(
        ec: &mut dyn ExecutionContext<Types>,
    ) -> Completion<<Types as JsTypes>::JsValue, Types> {
        // Step 1: "Let p be a new promise."
        let (promise, resolvers) = ec.new_promise_pending()?;

        // Step 2: "Run the following in parallel:"
        // Note: In a single-threaded content process the collection runs on
        // the main thread, approximating the spec's "in parallel". The
        // collection is queued as a job so it runs at the next microtask
        // checkpoint rather than synchronously: browsers defer the
        // in-parallel steps past the current microtask checkpoint, and a
        // collection must not reap objects that queued reaction jobs still
        // reference (e.g. a stream's start reaction, which sets `started`
        // and performs the first pull).
        let realm = ec.current_realm();
        ec.enqueue_job_with_realm(
            realm,
            Box::new(move |job_context| {
                // Step 2.1: "Run implementation-defined steps to perform a
                // garbage collection covering at least the entry Realm."
                job_context.gc();

                // Step 2.2: "Resolve p."
                let undefined = job_context.value_undefined();
                if let Err(error) = job_context.call(&resolvers.resolve, &undefined, &[]) {
                    job_context.report_exception(error);
                }
            }),
        );

        Ok(promise)
    }
}

use js_engine::{Completion, ExecutionContext, JsTypes};

use crate::js::Types;
