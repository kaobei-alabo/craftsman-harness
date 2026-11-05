//! Capability-checked host functions. No WASI preopens or sockets are needed.

use crate::GadgetError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Captured by the host, never written directly to the user's terminal.
pub fn log(level: Level, message: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        use crate::component::bindings::craftsman::gadget::host as raw;
        let level = match level {
            Level::Trace => raw::Level::Trace,
            Level::Debug => raw::Level::Debug,
            Level::Info => raw::Level::Info,
            Level::Warn => raw::Level::Warn,
            Level::Error => raw::Level::Error,
        };
        raw::log(level, message);
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (level, message); // Native tests do not have a runtime host.
}

pub fn read_file(path: &str) -> Result<Vec<u8>, GadgetError> {
    #[cfg(target_arch = "wasm32")]
    return crate::component::bindings::craftsman::gadget::host::read_file(path)
        .map_err(Into::into);
    #[cfg(not(target_arch = "wasm32"))]
    unavailable(path)
}

pub fn list_dir(path: &str) -> Result<Vec<DirEntry>, GadgetError> {
    #[cfg(target_arch = "wasm32")]
    return crate::component::bindings::craftsman::gadget::host::list_dir(path)
        .map(|entries| {
            entries
                .into_iter()
                .map(|e| DirEntry {
                    name: e.name,
                    is_dir: e.is_dir,
                    size: e.size,
                })
                .collect()
        })
        .map_err(Into::into);
    #[cfg(not(target_arch = "wasm32"))]
    unavailable(path)
}

pub fn write_file(path: &str, contents: &[u8]) -> Result<(), GadgetError> {
    #[cfg(target_arch = "wasm32")]
    return crate::component::bindings::craftsman::gadget::host::write_file(path, contents)
        .map_err(Into::into);
    #[cfg(not(target_arch = "wasm32"))]
    unavailable((path, contents))
}

pub fn http_request(request: Request) -> Result<Response, GadgetError> {
    #[cfg(target_arch = "wasm32")]
    {
        use crate::component::bindings::craftsman::gadget::host as raw;
        let request = raw::Request {
            method: request.method,
            url: request.url,
            headers: request.headers,
            body: request.body,
        };
        raw::http_request(&request)
            .map(|r| Response {
                status: r.status,
                headers: r.headers,
                body: r.body,
            })
            .map_err(Into::into)
    }
    #[cfg(not(target_arch = "wasm32"))]
    unavailable(request)
}

#[cfg(not(target_arch = "wasm32"))]
fn unavailable<T, A>(_: A) -> Result<T, GadgetError> {
    Err(GadgetError::new(
        "gadget host functions require a WASM runtime; use pure functions in native tests",
    ))
}
