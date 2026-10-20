/* PdfCraft as a C library: the headless automation tools behind a small C ABI.
 *
 * Every tool `pdfcraft-cli tools` lists (`doc_open`, `doc_create`, `doc_save`, `page_render`,
 * `page_rotate`, `text_extract`, …) is reached through `pdfcraft_call` with its name and its
 * JSON arguments. Results come back as one JSON document (see README.md for the envelope).
 *
 * Strings are UTF-8 and NUL-terminated. Every `char*` this library returns is owned by the
 * caller until `pdfcraft_free_string`; never free it with the C runtime's `free`.
 */
#ifndef PDFCRAFT_H
#define PDFCRAFT_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* An automation session: its open documents, their undo history and caches. Calls on one
 * session are serialized by an internal lock, so a session may be shared between threads;
 * use one session per thread for parallel work. */
typedef struct PdfCraft PdfCraft;

/* Status codes written by `pdfcraft_call` (also `error.kind` in the JSON). */
enum {
    PDFCRAFT_OK = 0,
    PDFCRAFT_UNKNOWN_TOOL = 1, /* no tool has this name */
    PDFCRAFT_INVALID_ARGS = 2, /* the arguments don't match the tool's schema */
    PDFCRAFT_FAILED = 3,       /* the tool ran and failed; the message says why */
    PDFCRAFT_PANIC = 4,        /* a bug inside the library was caught; free the session */
    PDFCRAFT_BAD_CALL = 5      /* null handle or tool, bad UTF-8, or arguments that are not JSON */
};

/* The library version ("0.8.0"); a static string, never freed. */
const char* pdfcraft_version(void);

/* Create a session. `root_utf8` confines every path the tools read or write to that folder
 * (relative paths resolve inside it); NULL means no confinement. Returns NULL on failure and,
 * when `error` is not NULL, stores the reason there (free it with `pdfcraft_free_string`). */
PdfCraft* pdfcraft_new(const char* root_utf8, char** error);

/* Run one tool. `args_json` is a JSON object or NULL (no arguments). Always returns a JSON
 * string: `{"content":[…]}` on success, `{"error":{"kind":…,"message":…}}` otherwise; the same
 * outcome is written to `*status` when `status` is not NULL. Free the result with
 * `pdfcraft_free_string`. */
char* pdfcraft_call(PdfCraft* session, const char* tool, const char* args_json, int32_t* status);

/* Free a string returned by `pdfcraft_new` or `pdfcraft_call`. NULL is a no-op. */
void pdfcraft_free_string(char* s);

/* Close every document of the session without saving and free it. NULL is a no-op. */
void pdfcraft_free(PdfCraft* session);

#ifdef __cplusplus
}
#endif

#endif /* PDFCRAFT_H */
