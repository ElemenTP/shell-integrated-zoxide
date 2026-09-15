/*
 * ffi.h — C declarations for the zoxide-ffi library (zo_* API).
 *
 * Include this header in the zsh module shim (module.c) and in C test
 * harnesses. The declarations must stay in sync with rust_src/src/ffi.rs.
 */
#ifndef ZO_FFI_H
#define ZO_FFI_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Opaque session handle. */
typedef struct zo_session zo_session_t;

/* Query parameters for zo_session_query. */
typedef struct {
  const char *const *keywords; /* NULL when keywords_len == 0 */
  size_t keywords_len;         /* Number of entries in keywords */
  const char *exclude;         /* Exact path to exclude, or NULL */
  const char *base_dir;        /* Restrict search below this dir, or NULL */
  int all;                     /* Include unavailable directories */
  int interactive;             /* Run fzf and return the selection */
  int list;                    /* Return every match */
  int score;                   /* Prefix results with frecency scores */
} zo_query_options_t;

/* Session counters exposed to shell tooling. */
typedef struct {
  unsigned long long adds;
  unsigned long long queries;
  unsigned long long removes;
  unsigned long long entries;
} zo_stats_t;

/*
 * Error reporting model (see rust_src/src/ffi.rs for the full rationale):
 *
 * Errors are RETURN VALUES, not stored state. Every call that can fail
 * returns the error message itself:
 *
 *     char *err = zo_session_add(session, path, 1.0);
 *     if (err) {
 *         fprintf(stderr, "zoxide: %s\n", err);
 *         zo_free(err);            // caller owns the message
 *     }
 *
 *   - NULL means success.
 *   - A non-NULL pointer is a Rust-allocated NUL-terminated UTF-8 message
 *     that the caller MUST release with zo_free().
 *   - An empty message ("") is NOT success: it is a failure with no text
 *     (zoxide reports fzf Ctrl-C / SilentExit that way), so callers test the
 *     pointer first and print only when *err is non-zero.
 *
 * Nothing is stored in the session and nothing is stored in a global, so
 * concurrent sessions (or one session used from different threads) can never
 * clobber each other's error, and there is no thread_local! state that could
 * outlive dlclose() of this library.
 *
 * zo_session_create is the one call whose payload is not a string: it writes
 * the handle to an out-parameter and returns the error instead.
 * zo_session_destroy returns an error string too — a panic while dropping the
 * session is the only way teardown can fail, and the message makes such a bug
 * visible for debugging. zo_free cannot fail and therefore has no error return.
 */

/* Create a session. Returns NULL on success and writes a non-NULL handle to
 * *session; on failure returns an error message (free with zo_free) and sets
 * *session to NULL. */
char *zo_session_create(zo_session_t **session);

/* Destroy a session. Returns NULL on success, or an allocated error message
 * (free with zo_free) for diagnostics. Passing NULL is a successful no-op. */
char *zo_session_destroy(zo_session_t *session);

/* Database operations. Return NULL on success, or an error message that the
 * caller must free with zo_free() — including for "path not in database". */
char *zo_session_add(zo_session_t *session, const char *path, double score);
char *zo_session_remove(zo_session_t *session, const char *path);

/* Run a query. On success (NULL return) *out is a Rust-allocated UTF-8 string
 * that the caller frees with zo_free(). On failure *out is set to NULL and the
 * error message is returned. */
char *zo_session_query(zo_session_t *session, const zo_query_options_t *options,
                       char **out);

/* Retrieve session counters. NULL on success, else an error message. */
char *zo_session_stats(zo_session_t *session, zo_stats_t *out);

/* Free a string returned by any function above (query results and error
 * messages). NULL-safe, cannot fail. */
void zo_free(char *ptr);

/* Return the library version string (static, must NOT be freed). */
const char *zo_version(void);

#ifdef __cplusplus
}
#endif

#endif /* ZO_FFI_H */
