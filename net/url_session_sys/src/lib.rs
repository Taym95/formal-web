//! Minimal Foundation URLSession bindings for formal-web.
//!
//! Provides only the URLSession surface needed by the `url_session` crate:
//! one session with no shared cache, and data-task fetches with a completion
//! callback. Only compiled on Apple targets.

#![allow(non_camel_case_types, non_snake_case)]

#[cfg(target_vendor = "apple")]
mod apple;

#[cfg(target_vendor = "apple")]
pub use apple::*;

#[cfg(not(target_vendor = "apple"))]
compile_error!("url_session_sys is only available on Apple targets");
