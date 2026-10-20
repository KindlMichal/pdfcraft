# pdfcraft-ffi

PdfCraft as a C library: the headless automation tools (`crates/automation`, the same table
`pdfcraft-cli run` and the MCP server use) behind five exported C functions, for C# (P/Invoke),
C and C++ hosts. No GUI code is linked.

- **Layer:** L8 app (exempt in `cargo xtask layers`), over `pdfcraft-automation` without its
  `mcp` feature.
- **Deliverable:** `pdfcraft_ffi.dll` (Windows), `libpdfcraft_ffi.dylib` (macOS),
  `libpdfcraft_ffi.so` (Linux, FreeBSD), plus [`include/pdfcraft.h`](include/pdfcraft.h).
- **Status:** the five functions exist, their tests pass on macOS, and the C# sample in
  [`examples/csharp`](examples/csharp) (a `[LibraryImport]` wrapper plus PNG → PDF) has been
  built with the .NET 10 SDK and run against the macOS library, including the refused-path
  error. The crate is a workspace member, so CI's Windows job compiles and tests it on every
  PR, but nobody has yet built `pdfcraft_ffi.dll` on a Windows machine by hand or loaded it from
  a Windows C# process. The C# files are documentation for hosts, not product code.

## Building

```sh
cargo build --release -p pdfcraft-ffi
# → target/release/pdfcraft_ffi.dll  (+ pdfcraft_ffi.dll.lib import library and .pdb)
#   target/release/libpdfcraft_ffi.dylib / libpdfcraft_ffi.so
```

Windows x64 needs the `x86_64-pc-windows-msvc` toolchain, the Visual Studio Build Tools (C++
workload) and CMake: `pdfcraft-sign` builds `aws-lc-sys` from C for RSA signing (ADR-0009);
its `prebuilt-nasm` feature means NASM is not needed. Everything else in the tree is Rust.
`CRAFT_FONTS_DIR` is optional, as for every other binary (CLAUDE.md). Building the Windows
library from another platform (for example with `cargo-xwin`) has not been tried.

## API

| Function | Does |
|---|---|
| `const char* pdfcraft_version(void)` | The library version; a static string, never freed. |
| `PdfCraft* pdfcraft_new(const char* root, char** error)` | A session. `root` confines every path the tools read or write to that folder (relative paths resolve inside it); `NULL` means no confinement. On failure returns `NULL` and, if `error` is not `NULL`, the reason. |
| `char* pdfcraft_call(PdfCraft*, const char* tool, const char* args_json, int32_t* status)` | Run one tool. `args_json` is a JSON object or `NULL`. Always returns a JSON string and writes the status (if the pointer is not `NULL`). |
| `void pdfcraft_free_string(char*)` | Free a string the library returned. `NULL` is a no-op. |
| `void pdfcraft_free(PdfCraft*)` | Close every document without saving and free the session. `NULL` is a no-op. |

Strings are UTF-8 and NUL-terminated. Every string the library returns is owned by the caller
until `pdfcraft_free_string`; never free it with the C runtime.

**Result envelope.** A successful call returns `{"content": [ … ]}`, one item per result the
tool produced (almost always exactly one). A JSON result is the tool's value as documented by
`pdfcraft-cli tools`; an image result (`page_render`, `comment_image_preview`) is
`{"image": "png", "width": W, "height": H, "base64": "…"}`. A failed call returns
`{"error": {"kind": "…", "message": "…"}}`.

**Status codes** (`include/pdfcraft.h`, also `error.kind`):

| Code | Kind | Meaning |
|---|---|---|
| 0 | | success |
| 1 | `unknown_tool` | no tool has this name |
| 2 | `invalid_args` | the arguments don't match the tool's schema |
| 3 | `failed` | the tool ran and failed; the message says what was wrong and what is allowed |
| 4 | `panic` | a bug inside the library was caught at the boundary; free the session and start another |
| 5 | `bad_call` | null session or tool, invalid UTF-8, or arguments that are not JSON |

**Threads.** A session holds its open documents, their undo history and caches behind a lock,
so a session may be shared between threads and calls on it serialize. For parallel work, use
one session per thread. A session may be created on one thread and used on another.

**Panics and crashes.** The engine never panics on purpose (AGENTS.md §4); should a bug get
through, every export catches it and reports status 4 instead of unwinding into the host, which
would abort the process. The library is built with `panic = "unwind"` (the default) so this
guard works.

**Confinement.** `root` behaves exactly like `pdfcraft-cli run --root`: every path outside it
(symlinks and `..` resolved) is refused with `… is outside the allowed directory …`, whether or
not it exists. See `crates/automation/README.md` for the full rules.

## From C#

[`examples/csharp/PdfCraft.cs`](examples/csharp/PdfCraft.cs) wraps the five functions with
`[LibraryImport]` (.NET 7+), a `SafeHandle` for the session and `System.Text.Json` for the
envelope. PNG → PDF is three lines:

```csharp
using var pdf = new PdfCraftSession(root: Environment.CurrentDirectory);
var doc = pdf.Call("doc_create", new { from = "images", paths = new[] { "scan.png" }, dpi = 72 })["doc"]!.GetValue<ulong>();
pdf.Call("doc_save", new { doc, path = "scan.pdf" });
```

Run the sample with the native library beside the executable (the project copies a release
build from `target/release` when one exists; `dotnet run` uses the project folder as the
working directory, which the sample makes the session's root, so the image path is relative
to it):

```sh
cargo build --release -p pdfcraft-ffi
cd apps/pdfcraft-ffi/examples/csharp && dotnet run -- picture.png picture.pdf
```

## Why this crate allows `unsafe`

The workspace forbids `unsafe` (`AGENTS.md` §4) and this is the one deliberate exception: a C
ABI cannot be exported without `#[unsafe(no_mangle)]`, and a pointer the host hands in cannot
be read without an `unsafe` block. The exception is kept to the boundary:

- the crate overrides only `unsafe_code` and adds `unsafe_op_in_unsafe_fn = "deny"` and
  `clippy::undocumented_unsafe_blocks = "deny"`, so every `unsafe` block names the invariant it
  relies on;
- the exports do nothing but check pointers, convert strings and lock the session; all behaviour
  is in `pdfcraft-automation` and below, which stay `forbid(unsafe_code)`;
- `tests/ffi.rs` calls every export with null pointers, invalid UTF-8, non-JSON arguments and a
  path outside the root; `tests/dylib.rs` loads the built library by file name and symbol name,
  as a host does.

## Not here yet

- A typed API (one C function per tool). The JSON surface covers everything and tracks new
  tools automatically; typed wrappers belong in the host language.
- Streaming or zero-copy bytes: an image result is base64 inside the envelope (+33 %); large
  outputs are better written by the tool itself (`doc_save`, `doc_export_images`, `image_save`).
- Progress callbacks and cancellation.
- The MCP server (use `pdfcraft-cli mcp`).
