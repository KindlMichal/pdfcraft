//! Load the built shared library the way a C# host does: by file name and symbol name, with no
//! Rust type information. This pins the exported names and the C calling convention.

use std::ffi::{CStr, c_char, c_void};
use std::path::PathBuf;

/// The shared library for this test's profile, built on demand: `cargo test` compiles the
/// crate as an `rlib` for the tests but does not link the `cdylib`, so the test asks Cargo for
/// it the way a host would (`cargo build -p pdfcraft-ffi`), into the same target directory.
fn library_path() -> PathBuf {
    let exe = std::env::current_exe().expect("test executable path");
    // target/<profile>/deps/<test>-<hash> → target/<profile>/ → target/
    let profile_dir = exe.parent().and_then(|p| p.parent()).expect("target profile directory");
    let target_dir = profile_dir.parent().expect("target directory");
    let mut build = std::process::Command::new(env!("CARGO"));
    build.args(["build", "-p", "pdfcraft-ffi", "--lib", "--target-dir"]).arg(target_dir);
    if profile_dir.file_name().is_some_and(|n| n == "release") {
        build.arg("--release");
    }
    let output = build.output().expect("run cargo build");
    assert!(output.status.success(), "cargo build -p pdfcraft-ffi failed:\n{}", String::from_utf8_lossy(&output.stderr));
    let name = format!("{}pdfcraft_ffi{}", std::env::consts::DLL_PREFIX, std::env::consts::DLL_SUFFIX);
    profile_dir.join(name)
}

#[test]
fn exported_symbols_work_through_dynamic_loading() {
    let path = library_path();
    assert!(path.is_file(), "no shared library at {}", path.display());
    // SAFETY: test code loading the crate's own cdylib, whose initializers are Rust's.
    let lib = unsafe { libloading::Library::new(&path) }.unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    // SAFETY: the signatures match `include/pdfcraft.h`.
    unsafe {
        let version: libloading::Symbol<unsafe extern "C" fn() -> *const c_char> = lib.get(b"pdfcraft_version\0").unwrap();
        let new: libloading::Symbol<unsafe extern "C" fn(*const c_char, *mut *mut c_char) -> *mut c_void> = lib.get(b"pdfcraft_new\0").unwrap();
        let call: libloading::Symbol<unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char, *mut i32) -> *mut c_char> =
            lib.get(b"pdfcraft_call\0").unwrap();
        let free_string: libloading::Symbol<unsafe extern "C" fn(*mut c_char)> = lib.get(b"pdfcraft_free_string\0").unwrap();
        let free: libloading::Symbol<unsafe extern "C" fn(*mut c_void)> = lib.get(b"pdfcraft_free\0").unwrap();

        assert_eq!(CStr::from_ptr(version()).to_str().unwrap(), env!("CARGO_PKG_VERSION"));
        let h = new(std::ptr::null(), std::ptr::null_mut());
        assert!(!h.is_null());
        let mut status = -1;
        let out = call(h, c"doc_create".as_ptr(), c"{\"from\":\"blank\",\"pages\":2}".as_ptr(), &mut status);
        let text = CStr::from_ptr(out).to_str().unwrap().to_owned();
        free_string(out);
        assert_eq!(status, 0, "{text}");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["content"][0]["pages"], 2, "{text}");
        free(h);
    }
}
