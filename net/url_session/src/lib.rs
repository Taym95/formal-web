//! Apple URLSession-backed HTTP client used by the net crate's URLSession
//! backend.
//!
//! Wraps the raw `url_session_sys` FFI bindings in a small safe API: one
//! session with no shared cache, per fetch completion callbacks. Fetch
//! completions fire on a background queue, at any time after
//! [`UrlSession::fetch`] returns.

use std::ffi::{CStr, CString};
use std::os::raw::c_void;

use url_session_sys::FwUrlSession;

/// The callback invoked with the outcome of a fetch.
pub type FetchCompletion = Box<dyn FnOnce(Result<FetchResponse, String>) + Send>;

/// <https://fetch.spec.whatwg.org/#concept-response>
pub struct FetchResponse {
    pub status: u16,
    pub final_url: String,
    pub content_type: String,
    pub body: Vec<u8>,
}

/// One NSURLSession with no shared cache.
pub struct UrlSession {
    handle: FwUrlSession,
}

impl UrlSession {
    /// Create a session with no shared cache.
    pub fn new() -> Result<Self, String> {
        let handle = unsafe { url_session_sys::fw_url_session_create() };
        if handle.is_null() {
            return Err(String::from("failed to create NSURLSession"));
        }
        Ok(UrlSession { handle })
    }

    /// Start a fetch on this session. The completion handler is invoked
    /// exactly once, on a background queue, when the task finishes; it may
    /// fire after this method returns.
    pub fn fetch(
        &self,
        method: &str,
        url: &str,
        body: Option<&[u8]>,
        completion: impl FnOnce(Result<FetchResponse, String>) + Send + 'static,
    ) -> Result<(), String> {
        let method_c = CString::new(method).map_err(|error| format!("invalid method: {error}"))?;
        let url_c = CString::new(url).map_err(|error| format!("invalid URL: {error}"))?;

        // Double box: the outer box is a thin pointer, so it can be passed
        // through the C callback as a `void *` context and recovered as a
        // `FetchCompletion` in the trampoline.
        let callback: Box<FetchCompletion> = Box::new(Box::new(completion));
        let context = Box::into_raw(callback) as *mut c_void;

        let result = unsafe {
            url_session_sys::fw_url_session_fetch(
                self.handle,
                method_c.as_ptr(),
                url_c.as_ptr(),
                body.map_or(std::ptr::null(), |bytes| bytes.as_ptr()),
                body.map_or(0, |bytes| bytes.len()),
                context,
                Some(completion_trampoline),
            )
        };
        if result != 0 {
            // The task could not be started; the completion callback was not
            // invoked, so reclaim the callback box.
            let callback = unsafe { Box::from_raw(context as *mut FetchCompletion) };
            drop(callback);
            return Err(String::from("failed to start URLSession data task"));
        }
        Ok(())
    }
}

impl Drop for UrlSession {
    fn drop(&mut self) {
        unsafe { url_session_sys::fw_url_session_release(self.handle) };
    }
}

unsafe extern "C" fn completion_trampoline(
    context: *mut c_void,
    status_code: std::os::raw::c_int,
    final_url: *const std::os::raw::c_char,
    content_type: *const std::os::raw::c_char,
    body: *const u8,
    body_length: usize,
    error: *const std::os::raw::c_char,
) {
    let callback = unsafe { Box::from_raw(context as *mut FetchCompletion) };
    let result = if !error.is_null() {
        let message = unsafe { CStr::from_ptr(error) }
            .to_string_lossy()
            .into_owned();
        Err(message)
    } else {
        let final_url = if final_url.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(final_url) }
                .to_string_lossy()
                .into_owned()
        };
        let content_type = if content_type.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(content_type) }
                .to_string_lossy()
                .into_owned()
        };
        let body = if body.is_null() || body_length == 0 {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(body, body_length) }.to_vec()
        };
        Ok(FetchResponse {
            status: status_code as u16,
            final_url,
            content_type,
            body,
        })
    };
    callback(result);
}
