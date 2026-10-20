//! PdfCraft as a C library: the headless automation tools behind five exported functions.
//!
//! The contract for hosts is `include/pdfcraft.h`; README.md explains the result envelope, the
//! status codes, ownership and threading. This crate is the workspace's one deliberate
//! `unsafe` exception (see Cargo.toml and README.md): the exports check pointers, convert
//! strings and lock the session, and every behaviour lives in `pdfcraft-automation` and below,
//! which stay `forbid(unsafe_code)`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::any::Any;
use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::Mutex;

use base64::Engine as _;
use pdfcraft_automation::{Automation, Content, ToolError};
use serde_json::{Value, json};

/// The call succeeded.
pub const PDFCRAFT_OK: i32 = 0;
/// No tool has this name.
pub const PDFCRAFT_UNKNOWN_TOOL: i32 = 1;
/// The arguments don't match the tool's schema.
pub const PDFCRAFT_INVALID_ARGS: i32 = 2;
/// The tool ran and failed; the message says why.
pub const PDFCRAFT_FAILED: i32 = 3;
/// A bug inside the library was caught at the boundary; the host should free the session.
pub const PDFCRAFT_PANIC: i32 = 4;
/// A null session or tool, invalid UTF-8, or arguments that are not JSON.
pub const PDFCRAFT_BAD_CALL: i32 = 5;

/// An automation session behind the C ABI: one [`Automation`] (its open documents, undo history
/// and caches) under a lock, so calls from several host threads serialize instead of racing.
/// Hosts see it as an opaque pointer.
pub struct PdfCraft {
    inner: Mutex<Automation>,
}

// A session may be created on one thread and used on another, which needs `Automation: Send`.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<Automation>();
};

/// The crate version with the NUL terminator C expects.
static VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");

/// The library version, e.g. `0.8.0`. A static string: never free it.
#[unsafe(no_mangle)]
pub extern "C" fn pdfcraft_version() -> *const c_char {
    VERSION.as_ptr().cast()
}

/// Create a session. `root` confines every path the tools read or write to that folder
/// (`NULL`: no confinement). Returns `NULL` on failure and, when `error` is not `NULL`, stores
/// the reason there as a string to free with [`pdfcraft_free_string`].
///
/// # Safety
/// `root` is `NULL` or a NUL-terminated string that stays valid during the call. `error` is
/// `NULL` or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfcraft_new(root: *const c_char, error: *mut *mut c_char) -> *mut PdfCraft {
    let outcome = catch_unwind(|| -> Result<Automation, String> {
        let automation = Automation::new();
        if root.is_null() {
            return Ok(automation);
        }
        // SAFETY: the caller passes a NUL-terminated string that outlives the call (the
        // contract in pdfcraft.h); it is only read.
        let root = unsafe { CStr::from_ptr(root) }.to_str().map_err(|_| "root is not valid UTF-8".to_string())?;
        automation.with_root(root).map_err(|e| format!("root {root}: {e}"))
    });
    let result = outcome.unwrap_or_else(|p| Err(format!("panic while creating the session: {}", panic_message(p.as_ref()))));
    match result {
        Ok(automation) => {
            if !error.is_null() {
                // SAFETY: `error` is not null and the caller promised it is writable.
                unsafe { *error = ptr::null_mut() };
            }
            Box::into_raw(Box::new(PdfCraft { inner: Mutex::new(automation) }))
        }
        Err(message) => {
            if !error.is_null() {
                // SAFETY: as above.
                unsafe { *error = into_c_string(message) };
            }
            ptr::null_mut()
        }
    }
}

/// Run one tool. `args_json` is a JSON object or `NULL` (no arguments). Always returns a JSON
/// string to free with [`pdfcraft_free_string`]: `{"content":[…]}` on success,
/// `{"error":{"kind":…,"message":…}}` otherwise. The same outcome is written to `*status` when
/// `status` is not `NULL`.
///
/// # Safety
/// `session` is `NULL` or a pointer from [`pdfcraft_new`] not yet passed to [`pdfcraft_free`].
/// `tool` and `args_json` are `NULL` or NUL-terminated strings valid during the call. `status`
/// is `NULL` or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfcraft_call(session: *mut PdfCraft, tool: *const c_char, args_json: *const c_char, status: *mut i32) -> *mut c_char {
    // SAFETY: forwarded; the contract is this function's.
    let (code, body) = catch_unwind(AssertUnwindSafe(|| unsafe { call(session, tool, args_json) }))
        .unwrap_or_else(|p| error_body(PDFCRAFT_PANIC, format!("panic inside the library: {}", panic_message(p.as_ref()))));
    if !status.is_null() {
        // SAFETY: `status` is not null and the caller promised it is writable.
        unsafe { *status = code };
    }
    into_c_string(body)
}

/// Free a string returned by [`pdfcraft_new`] or [`pdfcraft_call`]. `NULL` is a no-op.
///
/// # Safety
/// `s` is `NULL` or a string this library returned, freed at most once and not read afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfcraft_free_string(s: *mut c_char) {
    if !s.is_null() {
        // SAFETY: every string this library hands out comes from `CString::into_raw`, and the
        // caller promised this is the only free.
        drop(unsafe { CString::from_raw(s) });
    }
}

/// Close every document of the session without saving and free it. `NULL` is a no-op.
///
/// # Safety
/// `session` is `NULL` or a pointer from [`pdfcraft_new`], freed at most once and not used
/// afterwards, including from other threads.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfcraft_free(session: *mut PdfCraft) {
    if !session.is_null() {
        // SAFETY: `session` came from `Box::into_raw` in `pdfcraft_new`, and the caller promised
        // this is the only free. A panic while dropping must not cross into the host either.
        let boxed = unsafe { Box::from_raw(session) };
        let _ = catch_unwind(AssertUnwindSafe(move || drop(boxed)));
    }
}

/// [`pdfcraft_call`] without the panic guard and the out-parameter: the status and the JSON body.
///
/// # Safety
/// As [`pdfcraft_call`].
unsafe fn call(session: *mut PdfCraft, tool: *const c_char, args_json: *const c_char) -> (i32, String) {
    if session.is_null() {
        return error_body(PDFCRAFT_BAD_CALL, "the session handle is null");
    }
    if tool.is_null() {
        return error_body(PDFCRAFT_BAD_CALL, "the tool name is null");
    }
    // SAFETY: `tool` is not null and, per the contract, NUL-terminated and alive for the call.
    let Ok(tool) = unsafe { CStr::from_ptr(tool) }.to_str() else {
        return error_body(PDFCRAFT_BAD_CALL, "the tool name is not valid UTF-8");
    };
    let args = if args_json.is_null() {
        Value::Null
    } else {
        // SAFETY: `args_json` is not null and, per the contract, NUL-terminated and alive.
        let Ok(text) = unsafe { CStr::from_ptr(args_json) }.to_str() else {
            return error_body(PDFCRAFT_BAD_CALL, "the arguments are not valid UTF-8");
        };
        match serde_json::from_str::<Value>(text) {
            Ok(v) => v,
            Err(e) => return error_body(PDFCRAFT_BAD_CALL, format!("the arguments are not valid JSON: {e}")),
        }
    };
    // SAFETY: a non-null `session` is, per the contract, a live pointer from `pdfcraft_new`; it
    // is only borrowed for the call.
    let session = unsafe { &*session };
    // A poisoned lock means an earlier call panicked while holding it. That call already
    // reported PDFCRAFT_PANIC; the session is still memory-safe, so keep serving rather than
    // refusing every later call (the host was told to start a new session).
    let mut automation = session.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    match automation.call(tool, &args) {
        Ok(content) => (PDFCRAFT_OK, json!({ "content": content.into_iter().map(content_json).collect::<Vec<_>>() }).to_string()),
        Err(ToolError::UnknownTool(_)) => error_body(PDFCRAFT_UNKNOWN_TOOL, format!("unknown tool {tool:?}")),
        Err(ToolError::InvalidArgs(m)) => error_body(PDFCRAFT_INVALID_ARGS, m),
        Err(ToolError::Failed(m)) => error_body(PDFCRAFT_FAILED, m),
    }
}

/// One result item of the envelope: a JSON result as it is, a PNG as base64 with its size.
fn content_json(content: Content) -> Value {
    match content {
        Content::Json(v) => v,
        Content::Png { data, width, height } => {
            json!({ "image": "png", "width": width, "height": height, "base64": base64::engine::general_purpose::STANDARD.encode(data) })
        }
    }
}

/// The name a status code has in `error.kind`.
fn kind(status: i32) -> &'static str {
    match status {
        PDFCRAFT_UNKNOWN_TOOL => "unknown_tool",
        PDFCRAFT_INVALID_ARGS => "invalid_args",
        PDFCRAFT_FAILED => "failed",
        PDFCRAFT_PANIC => "panic",
        _ => "bad_call",
    }
}

/// The status and the error envelope for it.
fn error_body(status: i32, message: impl Into<String>) -> (i32, String) {
    (status, json!({ "error": { "kind": kind(status), "message": message.into() } }).to_string())
}

/// What a caught panic said, for the error message.
fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "no message".to_string()
    }
}

/// A string for the host to own. JSON from serde never contains a raw NUL (it is escaped), but
/// an error message quoting a file name could; such bytes are dropped rather than failing.
fn into_c_string(s: String) -> *mut c_char {
    let c = CString::new(s).unwrap_or_else(|e| {
        let mut bytes = e.into_vec();
        bytes.retain(|b| *b != 0);
        // Infallible: no NUL is left, and `CString::new` fails only on an interior NUL.
        CString::new(bytes).unwrap_or_default()
    });
    c.into_raw()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_becomes_status_4_with_its_message() {
        let (status, body) = catch_unwind(|| -> (i32, String) { panic!("boom {}", 7) })
            .unwrap_or_else(|p| error_body(PDFCRAFT_PANIC, format!("panic inside the library: {}", panic_message(p.as_ref()))));
        assert_eq!(status, PDFCRAFT_PANIC);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"]["kind"], "panic");
        assert_eq!(v["error"]["message"], "panic inside the library: boom 7");
    }

    #[test]
    fn interior_nul_is_dropped_not_fatal() {
        let p = into_c_string("a\0b".to_string());
        // SAFETY: `p` was just produced by `into_c_string` and is freed once, below.
        let s = unsafe { CStr::from_ptr(p) }.to_str().unwrap().to_owned();
        // SAFETY: as above.
        unsafe { pdfcraft_free_string(p) };
        assert_eq!(s, "ab");
    }
}
