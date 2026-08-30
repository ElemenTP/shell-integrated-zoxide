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

/* Session lifecycle. */
zo_session_t *zo_session_create(void);
void zo_session_destroy(zo_session_t *session);

/* Database operations. Return 0 on success, <0 on error. */
int zo_session_add(zo_session_t *session, const char *path, double score);
int zo_session_remove(zo_session_t *session, const char *path);

/* Run a query. On success (0), *out is a Rust-allocated UTF-8 string.
 * The caller must free it with zo_free(). On failure *out is set to NULL. */
int zo_session_query(zo_session_t *session, const zo_query_options_t *options,
                     char **out);

/* Retrieve session counters. Returns 0 on success. */
int zo_session_stats(zo_session_t *session, zo_stats_t *out);

/* Free a string returned by zo_session_query / zo_last_error. NULL-safe. */
void zo_free(char *ptr);

/* Return the library version string (static, must NOT be freed). */
const char *zo_version(void);

/* Return the last error message, or NULL if no error is recorded.
 * The caller must free the returned string with zo_free(). */
void zo_last_error(char **out);

#ifdef __cplusplus
}
#endif

#endif /* ZO_FFI_H */
