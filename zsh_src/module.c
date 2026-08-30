/*
 * module.c — zsh loadable module for in-process zoxide directory jumping.
 *
 * The shim dynamically links against libzoxide_ffi.so (Rust cdylib). Both
 * files must be placed in the same directory so that $ORIGIN / @loader_path
 * runtime linking works.
 *
 * Each builtin corresponds to one zoxide subcommand
 * and exchanges all arguments/results through zsh variables:
 *
 *   ZOXIDE_ADD_PATH                → zoxide_add (scalar or array)
 *   ZOXIDE_ADD_SCORE               → zoxide_add (optional, default 1.0)
 *   ZOXIDE_QUERY_KEYWORDS          → zoxide_query (array)
 *   ZOXIDE_QUERY_EXCLUDE           → zoxide_query
 *   ZOXIDE_QUERY_BASE_DIR          → zoxide_query
 *   ZOXIDE_QUERY_ALL               → zoxide_query (0/1)
 *   ZOXIDE_QUERY_INTERACTIVE       → zoxide_query (0/1)
 *   ZOXIDE_QUERY_LIST              → zoxide_query (0/1)
 *   ZOXIDE_QUERY_SCORE             → zoxide_query (0/1)
 *   ZOXIDE_REMOVE_PATHS            → zoxide_remove (scalar or array)
 *   ZOXIDE_RESULT                  ← zoxide_query (unset on failure; stderr
 *                                    mirrors the original zoxide binary)
 *   ZOXIDE_VERSION                 ← zoxide_version
 *   ZOXIDE_STATS_*                 ← zoxide_stats
 *
 * Loading in zsh:
 *   module_path+=(/path/to/zoxide_native)
 *   zmodload zoxide_native
 */

#define MODULE
#define IMPORTING_MODULE_zshQsmain 1
#include "zsh.mdh"

#include "ffi.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* ------------------------------------------------------------------ */
/* Module metadata                                                    */
/* ------------------------------------------------------------------ */

#define MODNAME "zoxide_native"
#define BUILTIN_ZOXIDE_ADD "zoxide_add"
#define BUILTIN_ZOXIDE_QUERY "zoxide_query"
#define BUILTIN_ZOXIDE_REMOVE "zoxide_remove"
#define BUILTIN_ZOXIDE_VERSION "zoxide_version"
#define BUILTIN_ZOXIDE_STATS "zoxide_stats"

static int bin_zo_add(char *name, char **argv, Options ops, int func);
static int bin_zo_query(char *name, char **argv, Options ops, int func);
static int bin_zo_remove(char *name, char **argv, Options ops, int func);
static int bin_zo_version(char *name, char **argv, Options ops, int func);
static int bin_zo_stats(char *name, char **argv, Options ops, int func);

/* One builtin per upstream zoxide subcommand.
 * inputs are zsh variables and outputs are written back to zsh variables. */
static struct builtin bintab[] = {
    BUILTIN(BUILTIN_ZOXIDE_ADD, 0, bin_zo_add, 0, 0, 0, NULL, NULL),
    BUILTIN(BUILTIN_ZOXIDE_QUERY, 0, bin_zo_query, 0, 0, 0, NULL, NULL),
    BUILTIN(BUILTIN_ZOXIDE_REMOVE, 0, bin_zo_remove, 0, 0, 0, NULL, NULL),
    BUILTIN(BUILTIN_ZOXIDE_VERSION, 0, bin_zo_version, 0, -1, 0, NULL, NULL),
    BUILTIN(BUILTIN_ZOXIDE_STATS, 0, bin_zo_stats, 0, -1, 0, NULL, NULL),
};

/* Features table — only builtins, no conditions/math/params */
static struct features module_features = {
    bintab, sizeof(bintab) / sizeof(*bintab), /* builtins */
    NULL,   0,                                /* conditions */
    NULL,   0,                                /* math functions */
    NULL,   0,                                /* parameter definitions */
    0,                                        /* n_abstract */
};

/* ------------------------------------------------------------------ */
/* Session state (created in boot_, destroyed in cleanup_)            */
/* ------------------------------------------------------------------ */

static zo_session_t *g_session = NULL;

/* ------------------------------------------------------------------ */
/* Helpers                                                            */
/* ------------------------------------------------------------------ */

/* Read a zsh string parameter in UNMETAFIED form.
 *
 * getsparam() returns zsh's internal metafied representation (bytes >= 0x80
 * are escaped with the Meta character). getsparam_u() returns an unmetafied
 * copy in a STATIC buffer that is invalidated by the next getsparam_u() call,
 * so duplicate it with ztrdup() and let the caller release it with zsfree().
 * This matters for queries where two non-ASCII scalars (exclude/base_dir) are
 * read back-to-back. */
static char *get_str_param(const char *name) {
  char *val = getsparam_u((char *)name);
  if (!val || !*val)
    return NULL;
  return ztrdup(val);
}

/* Read a zsh integer parameter. Returns 0 when unset. */
static long get_int_param(const char *name) { return getiparam((char *)name); }

/* Read a zsh array parameter into a freshly allocated, unmetafied copy.
 *
 * getaparam() points into the parameter table, so mutating the entries with
 * unmetafy() would corrupt the shell parameter. zarrdup() makes a deep copy
 * and the caller must release it with freearray(). */
static char **get_arr_param(const char *name, size_t *len) {
  char **arr = getaparam((char *)name);
  if (!arr) {
    *len = 0;
    return NULL;
  }

  size_t n = arrlen(arr);
  if (n == 0) {
    *len = 0;
    return arr;
  }

  char **copy = zarrdup(arr);
  for (size_t i = 0; i < n; i++)
    unmetafy(copy[i], NULL);

  *len = n;
  return copy;
}

/* Write a Rust-allocated UTF-8 string into a zsh parameter.
 *
 * The raw string must be metafied for zsh's internal storage, otherwise bytes
 * above 0x80 (multi-byte UTF-8 characters) get corrupted by zsh's
 * metafication/unmetafication round-trip. META_DUP makes metafy() allocate a
 * new string with zsh's allocator. */
static void set_str_param(const char *name, const char *val) {
  if (!val)
    return;
  setsparam((char *)name, ztrdup_metafy(val));
}

/* Report an FFI failure using the Rust-side last-error string. */
static void report_ffi_error(const char *operation) {
  char *err = NULL;
  zo_last_error(&err);
  zwarnnam(MODNAME, "%s: %s", operation, err ? err : "unknown error");
  if (err)
    zo_free(err);
}

/* Report a failed query the same way the original `zoxide` binary does:
 * `zoxide: <error>` on stderr. SilentExit errors (e.g. fzf Ctrl-C) carry an
 * empty message and must produce no output at all. */
static void report_query_error(void) {
  char *err = NULL;
  zo_last_error(&err);
  if (err && *err) {
    fprintf(stderr, "zoxide: %s\n", err);
    fflush(stderr);
  }
  if (err)
    zo_free(err);
}

/* ------------------------------------------------------------------ */
/* Builtin: zoxide_add                                                */
/* ------------------------------------------------------------------ */

/**
 * Add $ZOXIDE_ADD_PATH to the database. The optional $ZOXIDE_ADD_SCORE is the
 * frecency increment (default 1.0).
 */
static int bin_zo_add(UNUSED(char *name), UNUSED(char **argv),
                      UNUSED(Options ops), UNUSED(int func)) {
  if (!g_session) {
    zwarnnam(MODNAME, "%s: session not initialized", BUILTIN_ZOXIDE_ADD);
    return 1;
  }

  double score = 1.0;
  char *score_str = get_str_param("ZOXIDE_ADD_SCORE");
  if (score_str) {
    char *end = NULL;
    score = strtod(score_str, &end);
    if (end == score_str || *end != '\0') {
      zwarnnam(MODNAME, "%s: invalid ZOXIDE_ADD_SCORE: %s", BUILTIN_ZOXIDE_ADD,
               score_str);
      zsfree(score_str);
      return 1;
    }
    zsfree(score_str);
  }

  /* getaparam() is NULL for scalars and unset parameters, so check it
   * first. getsparam_u() would join a multi-element array into one string,
   * which must not be treated as a single path. */
  if (getaparam((char *)"ZOXIDE_ADD_PATH")) {
    size_t paths_len = 0;
    char **paths = get_arr_param("ZOXIDE_ADD_PATH", &paths_len);
    if (paths_len == 0) {
      zwarnnam(MODNAME, "%s: ZOXIDE_ADD_PATH array is empty",
               BUILTIN_ZOXIDE_ADD);
      return 1;
    }

    int ret = 0;
    for (size_t i = 0; i < paths_len; i++) {
      if (zo_session_add(g_session, paths[i], score) != 0) {
        ret = 1;
        break;
      }
    }
    freearray(paths);
    if (ret) {
      report_ffi_error(BUILTIN_ZOXIDE_ADD);
      return 1;
    }
    return 0;
  }

  char *path = get_str_param("ZOXIDE_ADD_PATH");
  if (!path) {
    zwarnnam(MODNAME, "%s: ZOXIDE_ADD_PATH is not set",
             BUILTIN_ZOXIDE_ADD);
    return 1;
  }
  int ret = zo_session_add(g_session, path, score);
  zsfree(path);
  if (ret != 0) {
    report_ffi_error(BUILTIN_ZOXIDE_ADD);
    return 1;
  }
  return 0;
}

/* ------------------------------------------------------------------ */
/* Builtin: zoxide_query                                              */
/* ------------------------------------------------------------------ */

/**
 * Query the database using the ZOXIDE_QUERY_* variables. On success the result
 * is stored in ZOXIDE_RESULT. On failure it is unset and the builtin returns 1;
 * a non-empty native error is printed as `zoxide: <error>` while empty errors
 * (fzf Ctrl-C / SilentExit) produce no output, matching the original binary.
 */
static int bin_zo_query(UNUSED(char *name), UNUSED(char **argv),
                        UNUSED(Options ops), UNUSED(int func)) {
  if (!g_session) {
    zwarnnam(MODNAME, "%s: session not initialized", BUILTIN_ZOXIDE_QUERY);
    return 1;
  }

  size_t keywords_len = 0;
  char **keywords = get_arr_param("ZOXIDE_QUERY_KEYWORDS", &keywords_len);

  char *exclude = get_str_param("ZOXIDE_QUERY_EXCLUDE");
  char *base_dir = get_str_param("ZOXIDE_QUERY_BASE_DIR");

  zo_query_options_t options;
  memset(&options, 0, sizeof(options));
  options.keywords = (const char *const *)keywords;
  options.keywords_len = keywords_len;
  options.exclude = exclude;
  options.base_dir = base_dir;
  options.all = get_int_param("ZOXIDE_QUERY_ALL") != 0;
  options.interactive = get_int_param("ZOXIDE_QUERY_INTERACTIVE") != 0;
  options.list = get_int_param("ZOXIDE_QUERY_LIST") != 0;
  options.score = get_int_param("ZOXIDE_QUERY_SCORE") != 0;

  char *out = NULL;
  int ret = zo_session_query(g_session, &options, &out);
  zsfree(exclude);
  zsfree(base_dir);
  if (keywords_len > 0)
    freearray(keywords);
  if (ret != 0 || !out) {
    unsetparam((char *)"ZOXIDE_RESULT");
    report_query_error();
    return 1;
  }

  set_str_param("ZOXIDE_RESULT", out);
  zo_free(out);
  return 0;
}

/* ------------------------------------------------------------------ */
/* Builtin: zoxide_remove                                             */
/* ------------------------------------------------------------------ */

/**
 * Remove ZOXIDE_REMOVE_PATHS from the database. The variable may be a single
 * path scalar or an array of paths.
 */
static int bin_zo_remove(UNUSED(char *name), UNUSED(char **argv),
                         UNUSED(Options ops), UNUSED(int func)) {
  if (!g_session) {
    zwarnnam(MODNAME, "%s: session not initialized", BUILTIN_ZOXIDE_REMOVE);
    return 1;
  }

  /* Same scalar/array distinction as zoxide_add: getaparam() only returns
   * non-NULL for array parameters. */
  if (getaparam((char *)"ZOXIDE_REMOVE_PATHS")) {
    size_t paths_len = 0;
    char **paths = get_arr_param("ZOXIDE_REMOVE_PATHS", &paths_len);
    if (paths_len == 0) {
      return 0;
    }

    int ret = 0;
    for (size_t i = 0; i < paths_len; i++) {
      if (zo_session_remove(g_session, paths[i]) != 0) {
        ret = 1;
        break;
      }
    }
    freearray(paths);
    if (ret) {
      report_ffi_error(BUILTIN_ZOXIDE_REMOVE);
      return 1;
    }
    return 0;
  }

  char *path = get_str_param("ZOXIDE_REMOVE_PATHS");
  if (path) {
    int ret = zo_session_remove(g_session, path);
    zsfree(path);
    if (ret != 0) {
      report_ffi_error(BUILTIN_ZOXIDE_REMOVE);
      return 1;
    }
  }
  return 0;
}

/* ------------------------------------------------------------------ */
/* Builtin: zoxide_version                                            */
/* ------------------------------------------------------------------ */

/** Store the zoxide-native FFI version in ZOXIDE_VERSION. */
static int bin_zo_version(UNUSED(char *name), char **argv, UNUSED(Options ops),
                          UNUSED(int func)) {
  int quiet = 0;
  while (*argv) {
    if (strcmp(*argv, "-q") == 0)
      quiet = 1;
    argv++;
  }

  const char *version = zo_version();
  version = version ? version : "unknown";
  setsparam((char *)"ZOXIDE_VERSION", ztrdup(version));

  if (!quiet) {
    printf("%s\n", version);
  }

  return 0;
}

/* ------------------------------------------------------------------ */
/* Builtin: zoxide_stats                                              */
/* ------------------------------------------------------------------ */

/** Store session counters in ZOXIDE_STATS_* variables. */
static int bin_zo_stats(UNUSED(char *name), char **argv, UNUSED(Options ops),
                        UNUSED(int func)) {
  int verbose = 0, quiet = 0;
  while (*argv) {
    if (strcmp(*argv, "-v") == 0)
      verbose = 1;
    else if (strcmp(*argv, "-q") == 0)
      quiet = 1;
    argv++;
  }

  if (!g_session) {
    zwarnnam(MODNAME, "%s: session not initialized", BUILTIN_ZOXIDE_STATS);
    return 1;
  }

  zo_stats_t stats;
  memset(&stats, 0, sizeof(stats));
  if (zo_session_stats(g_session, &stats) != 0) {
    report_ffi_error(BUILTIN_ZOXIDE_STATS);
    return 1;
  }

  char buf[32];
#define SET_INT(name, val)                                                     \
  do {                                                                         \
    snprintf(buf, sizeof(buf), "%llu", (unsigned long long)(val));             \
    setsparam((char *)(name), ztrdup(buf));                                    \
  } while (0)

  SET_INT("ZOXIDE_STATS_ADDS", stats.adds);
  SET_INT("ZOXIDE_STATS_QUERIES", stats.queries);
  SET_INT("ZOXIDE_STATS_REMOVES", stats.removes);
  SET_INT("ZOXIDE_STATS_ENTRIES", stats.entries);

#undef SET_INT

  if (!quiet) {
    printf("zoxide_native session stats: adds=%llu queries=%llu removes=%llu\n",
           stats.adds, stats.queries, stats.removes);
    if (verbose) {
      printf("  database: %llu entries in memory\n", stats.entries);
    }
  }
  return 0;
}

/* ------------------------------------------------------------------ */
/* zsh module entry points                                            */
/* ------------------------------------------------------------------ */

/**/
int setup_(UNUSED(Module m)) { return 0; }

/**/
int features_(Module m, char ***features) {
  *features = featuresarray(m, &module_features);
  return 0;
}

/**/
int enables_(Module m, int **enables) {
  return handlefeatures(m, &module_features, enables);
}

/**/
int boot_(UNUSED(Module m)) {
  g_session = zo_session_create();
  if (!g_session) {
    char *err = NULL;
    zo_last_error(&err);
    zwarnnam(MODNAME, "failed to create session: %s",
             err ? err : "unknown error");
    if (err)
      zo_free(err);
    return 1;
  }
  return 0;
}

/**/
int cleanup_(Module m) {
  if (g_session) {
    zo_session_destroy(g_session);
    g_session = NULL;
  }

  /* Disable all features before teardown. */
  return setfeatureenables(m, &module_features, NULL);
}

/**/
int finish_(UNUSED(Module m)) { return 0; }
