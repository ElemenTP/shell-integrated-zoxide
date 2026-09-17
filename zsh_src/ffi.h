/*
 * ffi.h — C declarations for the zoxide-ffi library (zo_* API).
 *
 * Include this header in the zsh module shim (module.c) and in C test
 * harnesses. The declarations must stay in sync with rust_src/src/ffi.rs.
 *
 * The library exposes exactly one process-global zoxide session. It is
 * created by zo_init() and dropped again by zo_shutdown(); the data calls
 * (zo_add / zo_remove / zo_query / zo_stats) take no session handle.
 * zoxide itself is a CLI whose state is meant to die with the process, and
 * each shell host loads this module once, so a multi-session API would only
 * add handle plumbing without a consumer. Access to the global session is
 * serialized internally, so the FFI is sound even if a host calls it from
 * more than one thread.
 */
#ifndef ZO_FFI_H
#define ZO_FFI_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Query parameters for zo_query. */
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
 *     char *err = zo_add(path, 1.0);
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
 * Error values are carried by the individual call, so there is no error slot
 * to clear or stale, and there is no thread_local! state that could outlive
 * dlclose() of this library. zo_shutdown and zo_free cannot fail and
 * therefore return void.
 */

/* Initialize the process-global session. Idempotent: an already initialized
 * session makes this a successful no-op. Returns NULL on success, or an
 * allocated error message (free with zo_free). */
char *zo_init(void);

/* Drop the process-global session, resetting its counters. The on-disk
 * database is untouched. Calling this without a previous zo_init is a safe
 * no-op;*/
char *zo_shutdown(void);

/* Database operations. Return NULL on success, or an error message that the
 * caller must free with zo_free() — including for "path not in database".
 * Called before zo_init, they report that the session is not initialized. */
char *zo_add(const char *path, double score);
char *zo_remove(const char *path);

/* Run a query. On success (NULL return) *out is a Rust-allocated UTF-8 string
 * that the caller frees with zo_free(). On failure *out is set to NULL and the
 * error message is returned. */
char *zo_query(const zo_query_options_t *options, char **out);

/* Retrieve session counters. NULL on success, else an error message. The
 * adds/queries/removes counters are reset by zo_shutdown; entries is read
 * from the on-disk database for this call. */
char *zo_stats(zo_stats_t *out);

/* Free a string returned by any function above (query results and error
 * messages). NULL-safe, cannot fail. */
void zo_free(char *ptr);

/* Return the library version string (static, must NOT be freed). */
const char *zo_version(void);

#ifdef __cplusplus
}
#endif

#endif /* ZO_FFI_H */
