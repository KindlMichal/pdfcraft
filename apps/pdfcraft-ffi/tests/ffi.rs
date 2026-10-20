//! The C ABI, called from Rust through the `rlib` build: every export, its status codes, the
//! JSON envelope, root confinement and a PNG → PDF round trip. `dylib.rs` loads the real
//! shared library and checks the exported symbol names.

use std::ffi::{CStr, CString, c_char};
use std::path::Path;
use std::ptr;

use pdfcraft_ffi::{
    PDFCRAFT_BAD_CALL, PDFCRAFT_FAILED, PDFCRAFT_INVALID_ARGS, PDFCRAFT_OK, PDFCRAFT_PANIC, PDFCRAFT_UNKNOWN_TOOL, PdfCraft, pdfcraft_call,
    pdfcraft_free, pdfcraft_free_string, pdfcraft_new, pdfcraft_version,
};
use serde_json::{Value, json};

// The exports dereference caller pointers, so they are `unsafe fn`. The tests pass pointers
// to live NUL-terminated strings and handles from `pdfcraft_new`, which is the contract.
fn raw_call(h: *mut PdfCraft, tool: *const c_char, args: *const c_char, status: *mut i32) -> *mut c_char {
    // SAFETY: see above.
    unsafe { pdfcraft_call(h, tool, args, status) }
}
fn raw_new(root: *const c_char, error: *mut *mut c_char) -> *mut PdfCraft {
    // SAFETY: see above.
    unsafe { pdfcraft_new(root, error) }
}
fn free(h: *mut PdfCraft) {
    // SAFETY: `h` is a handle from `pdfcraft_new` that no test uses afterwards, or null.
    unsafe { pdfcraft_free(h) }
}
fn free_string(s: *mut c_char) {
    // SAFETY: `s` is a string the library returned and nothing reads afterwards, or null.
    unsafe { pdfcraft_free_string(s) }
}

/// Call a tool and hand back `(status, parsed JSON)`, freeing the C string.
fn call(h: *mut PdfCraft, tool: &str, args: &Value) -> (i32, Value) {
    let tool = CString::new(tool).unwrap();
    let args = CString::new(args.to_string()).unwrap();
    let mut status = -1;
    let out = raw_call(h, tool.as_ptr(), args.as_ptr(), &mut status);
    assert!(!out.is_null(), "a call always returns a string");
    // SAFETY: test code; `out` is a NUL-terminated string the library just returned.
    let text = unsafe { CStr::from_ptr(out) }.to_str().expect("UTF-8").to_owned();
    free_string(out);
    (status, serde_json::from_str(&text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}")))
}

/// A tool call that must succeed, returning its first content item.
fn ok(h: *mut PdfCraft, tool: &str, args: &Value) -> Value {
    let (status, v) = call(h, tool, args);
    assert_eq!(status, PDFCRAFT_OK, "{tool}: {v}");
    v["content"][0].clone()
}

/// The error message of a failed call, with its status.
fn err(h: *mut PdfCraft, tool: &str, args: &Value) -> (i32, String) {
    let (status, v) = call(h, tool, args);
    assert_ne!(status, PDFCRAFT_OK, "{tool} unexpectedly succeeded: {v}");
    let message = v["error"]["message"].as_str().unwrap_or_else(|| panic!("no error message: {v}")).to_owned();
    assert!(v["error"]["kind"].is_string(), "{v}");
    (status, message)
}

fn new_handle(root: Option<&Path>) -> *mut PdfCraft {
    let root = root.map(|r| CString::new(r.to_string_lossy().as_bytes()).unwrap());
    let mut error: *mut c_char = ptr::null_mut();
    let h = raw_new(root.as_ref().map_or(ptr::null(), |r| r.as_ptr()), &mut error);
    assert!(!h.is_null(), "pdfcraft_new failed: {}", c_string(error));
    assert!(error.is_null());
    h
}

fn c_string(s: *mut c_char) -> String {
    if s.is_null() {
        return "<null>".into();
    }
    // SAFETY: test code; `s` is a NUL-terminated string the library returned.
    let text = unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned();
    free_string(s);
    text
}

/// A 4×3 opaque RGB PNG without `pHYs`, so a page made from it is 4×3 points at 72 dpi.
fn tiny_png() -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, 4, 3);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().unwrap();
        w.write_image_data(&[200u8; 4 * 3 * 3]).unwrap();
    }
    out
}

#[test]
fn version_is_the_crate_version() {
    let v = pdfcraft_version();
    assert!(!v.is_null());
    // SAFETY: test code; the library returns a static NUL-terminated string.
    let v = unsafe { CStr::from_ptr(v) }.to_str().unwrap();
    assert_eq!(v, env!("CARGO_PKG_VERSION"));
}

#[test]
fn null_pointers_are_reported_not_dereferenced() {
    let mut status = -1;
    let out = raw_call(ptr::null_mut(), c"doc_list".as_ptr(), c"{}".as_ptr(), &mut status);
    assert_eq!(status, PDFCRAFT_BAD_CALL);
    let text = c_string(out);
    assert!(text.contains("handle"), "{text}");

    let h = new_handle(None);
    let out = raw_call(h, ptr::null(), c"{}".as_ptr(), &mut status);
    assert_eq!(status, PDFCRAFT_BAD_CALL);
    assert!(c_string(out).contains("tool"));
    // A null `args` means no arguments, like JSON `null` does for `Automation::call`.
    let out = raw_call(h, c"doc_list".as_ptr(), ptr::null(), &mut status);
    assert_eq!(status, PDFCRAFT_OK, "{}", c_string(out));
    free_string(out);
    // A null status pointer is allowed: the envelope still says what happened.
    let out = raw_call(h, c"nope".as_ptr(), c"{}".as_ptr(), ptr::null_mut());
    assert!(c_string(out).contains("unknown_tool"));
    free(h);
    // Freeing nothing is a no-op.
    free(ptr::null_mut());
    free_string(ptr::null_mut());
}

#[test]
fn bad_utf8_and_bad_json_are_bad_calls() {
    let h = new_handle(None);
    let mut status = -1;
    let bad = [b'd', 0xff, b'c', 0];
    let out = raw_call(h, bad.as_ptr().cast(), c"{}".as_ptr(), &mut status);
    assert_eq!(status, PDFCRAFT_BAD_CALL);
    assert!(c_string(out).contains("UTF-8"));
    let out = raw_call(h, c"doc_list".as_ptr(), c"{not json".as_ptr(), &mut status);
    assert_eq!(status, PDFCRAFT_BAD_CALL);
    assert!(c_string(out).contains("JSON"));
    // Arguments that are JSON but not an object are the tool layer's refusal, not ours.
    let (status, message) = err(h, "doc_list", &json!([1]));
    assert_eq!(status, PDFCRAFT_INVALID_ARGS);
    assert!(message.contains("object"), "{message}");
    free(h);
}

#[test]
fn tool_errors_map_to_status_codes_and_kinds() {
    let h = new_handle(None);
    let (status, v) = call(h, "nope", &json!({}));
    assert_eq!((status, v["error"]["kind"].as_str()), (PDFCRAFT_UNKNOWN_TOOL, Some("unknown_tool")));
    let (status, v) = call(h, "doc_create", &json!({ "from": "blank", "pages": "two" }));
    assert_eq!((status, v["error"]["kind"].as_str()), (PDFCRAFT_INVALID_ARGS, Some("invalid_args")));
    let (status, v) = call(h, "doc_open", &json!({ "path": "/nonexistent/for/sure.pdf" }));
    assert_eq!((status, v["error"]["kind"].as_str()), (PDFCRAFT_FAILED, Some("failed")));
    assert!(v["error"]["message"].as_str().unwrap().contains("sure.pdf"), "{v}");
    free(h);
    // The panic code exists for the host to switch on, even though no test can make a tool panic.
    assert_eq!(PDFCRAFT_PANIC, 4);
}

#[test]
fn png_to_pdf_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.png"), tiny_png()).unwrap();
    let h = new_handle(Some(dir.path()));
    let created = ok(h, "doc_create", &json!({ "from": "images", "paths": ["a.png"], "dpi": 72 }));
    assert_eq!(created["pages"], 1, "{created}");
    assert_eq!(created["name"], "a.pdf", "{created}");
    let doc = created["doc"].as_u64().unwrap();
    ok(h, "doc_save", &json!({ "doc": doc, "path": "a.pdf" }));
    free(h);

    let bytes = std::fs::read(dir.path().join("a.pdf")).unwrap();
    assert!(bytes.starts_with(b"%PDF-"));
    // A fresh handle reads it back; the page is 4×3 points, one point per pixel.
    let h = new_handle(Some(dir.path()));
    let opened = ok(h, "doc_open", &json!({ "path": "a.pdf" }));
    assert_eq!(opened["pages"], 1, "{opened}");
    let doc = opened["doc"].as_u64().unwrap();
    let info = ok(h, "doc_info", &json!({ "doc": doc }));
    let page = &info["pages"][0];
    assert_eq!((page["width"].as_f64(), page["height"].as_f64()), (Some(4.0), Some(3.0)), "{info}");
    // A PNG result travels as base64 inside the envelope.
    let (status, v) = call(h, "page_render", &json!({ "doc": doc, "page": 1, "dpi": 72 }));
    assert_eq!(status, PDFCRAFT_OK, "{v}");
    let image = &v["content"][0];
    assert_eq!((image["image"].as_str(), image["width"].as_u64(), image["height"].as_u64()), (Some("png"), Some(4), Some(3)), "{v}");
    use base64::Engine as _;
    let png = base64::engine::general_purpose::STANDARD.decode(image["base64"].as_str().unwrap()).unwrap();
    assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
    free(h);
}

#[test]
fn root_confinement_holds_through_the_c_abi() {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("root");
    std::fs::create_dir(&root).unwrap();
    let h = new_handle(Some(&root));
    // The document exists, so the refusal comes from the path check and nothing earlier.
    let doc = ok(h, "doc_create", &json!({ "from": "blank" }))["doc"].as_u64().unwrap();
    let (status, message) = err(h, "doc_save", &json!({ "doc": doc, "path": "../escape.pdf" }));
    assert_eq!(status, PDFCRAFT_FAILED);
    assert!(message.contains("outside the allowed directory"), "{message}");
    assert!(!base.path().join("escape.pdf").exists());
    ok(h, "doc_save", &json!({ "doc": doc, "path": "inside.pdf" }));
    assert!(root.join("inside.pdf").is_file());
    free(h);

    // A root that is not a folder is refused by `pdfcraft_new`, with the reason.
    let file = CString::new(root.join("inside.pdf").to_string_lossy().as_bytes()).unwrap();
    let mut error: *mut c_char = ptr::null_mut();
    let h = raw_new(file.as_ptr(), &mut error);
    assert!(h.is_null());
    assert!(c_string(error).contains("folder"));
    // So is a root that does not exist, and the caller may decline the reason.
    let missing = CString::new(root.join("missing").to_string_lossy().as_bytes()).unwrap();
    assert!(raw_new(missing.as_ptr(), ptr::null_mut()).is_null());
}

#[test]
fn documents_are_per_handle() {
    let a = new_handle(None);
    let b = new_handle(None);
    ok(a, "doc_create", &json!({ "from": "blank" }));
    assert_eq!(ok(a, "doc_list", &json!({}))["documents"].as_array().map(Vec::len), Some(1));
    assert_eq!(ok(b, "doc_list", &json!({}))["documents"].as_array().map(Vec::len), Some(0));
    free(a);
    free(b);
}

#[test]
fn the_c_header_agrees_with_the_status_constants() {
    // Three languages name these codes (lib.rs, pdfcraft.h, the C# sample); the header is the
    // contract, so it must say what the library does.
    let header = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/include/pdfcraft.h")).unwrap();
    for (name, value) in [
        ("PDFCRAFT_OK", PDFCRAFT_OK),
        ("PDFCRAFT_UNKNOWN_TOOL", PDFCRAFT_UNKNOWN_TOOL),
        ("PDFCRAFT_INVALID_ARGS", PDFCRAFT_INVALID_ARGS),
        ("PDFCRAFT_FAILED", PDFCRAFT_FAILED),
        ("PDFCRAFT_PANIC", PDFCRAFT_PANIC),
        ("PDFCRAFT_BAD_CALL", PDFCRAFT_BAD_CALL),
    ] {
        let line = header.lines().find(|l| l.trim_start().starts_with(name)).unwrap_or_else(|| panic!("{name} missing from pdfcraft.h"));
        // `    PDFCRAFT_PANIC = 4,        /* comment */`
        let declared: i32 = line
            .split('=')
            .nth(1)
            .and_then(|v| v.split_whitespace().next())
            .and_then(|v| v.trim_end_matches(',').parse().ok())
            .unwrap_or_else(|| panic!("{line:?}"));
        assert_eq!(declared, value, "{name}");
    }
    for name in ["pdfcraft_version", "pdfcraft_new", "pdfcraft_call", "pdfcraft_free_string", "pdfcraft_free"] {
        assert!(header.contains(&format!(" {name}(")), "{name} is not declared in pdfcraft.h");
    }
}
