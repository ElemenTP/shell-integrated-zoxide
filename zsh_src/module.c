/*
 * module.c — zsh loadable module for in-process zoxide directory jumping.
 *
 * The shim dynamically links against libzoxide_ffi.so (Rust cdylib). Both
 * files must be placed in the same directory so that $ORIGIN runtime linking
 * works.
 *
 * Loading in zsh:
 *   module_path+=(/path/to/zoxide_native)
 *   zmodload zoxide_native
 *   zoxide add -- /some/dir        # in-process database update
 *   zoxide query -- proj           # in-process query, result in ZOXIDE_RESULT
 */

#define MODULE
#define IMPORTING_MODULE_zshQsmain 1
#include "zsh.mdh"

#include "argparse.h"
#include "ffi.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* ------------------------------------------------------------------ */
/* Module metadata                                                    */
/* ------------------------------------------------------------------ */

#define MODNAME "zoxide_native"

/* Forward declarations */
static int bin_zo(char *name, char **argv, Options ops, int func);
static int bin_zo_add(char *name, char **argv, Options ops, int func);
static int bin_zo_query(char *name, char **argv, Options ops, int func);
static int bin_zo_remove(char *name, char **argv, Options ops, int func);
static int bin_zo_version(char *name, char **argv, Options ops, int func);
static int bin_zo_stats(char *name, char **argv, Options ops, int func);

/* Builtin table. `zoxide` is the CLI-compatible dispatch builtin; the
 * `zoxide_*` names are conveniences for callers that prefer one builtin per
 * subcommand. */
static struct builtin bintab[] = {
    BUILTIN("zoxide", 0, bin_zo, 0, -1, 0, NULL, NULL),
    BUILTIN("zoxide_add", 0, bin_zo_add, 0, -1, 0, NULL, NULL),
    BUILTIN("zoxide_query", 0, bin_zo_query, 0, -1, 0, NULL, NULL),
    BUILTIN("zoxide_remove", 0, bin_zo_remove, 0, -1, 0, NULL, NULL),
    BUILTIN("zoxide_version", 0, bin_zo_version, 0, -1, 0, NULL, NULL),
    BUILTIN("zoxide_stats", 0, bin_zo_stats, 0, -1, 0, NULL, NULL),
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

/* Write a Rust-allocated UTF-8 string into a zsh parameter.
 *
 * The raw string must be metafied for zsh's internal storage, otherwise bytes
 * above 0x80 (all multi-byte UTF-8 characters) get corrupted by zsh's
 * metafication/unmetafication round-trip. META_DUP makes metafy() allocate a
 * new string with zsh's allocator. */
static void set_str_param(const char *name, const char *val) {
  if (!val)
    return;
  setsparam((char *)name, metafy((char *)val, strlen(val), META_DUP));
}

/* Report an FFI failure using the Rust-side last-error string. */
static int report_ffi_error(const char *operation) {
  char *err = NULL;
  zo_last_error(&err);
  zwarnnam(MODNAME, "%s: %s", operation, err ? err : "unknown error");
  if (err)
    zo_free(err);
  return 1;
}

/* ------------------------------------------------------------------ */
/* Builtin implementations                                            */
/* ------------------------------------------------------------------ */

static int run_zo_add(char **argv) {
  char errbuf[256];
  zo_parsed_add_t parsed;
  if (zo_parse_add_args(argv, &parsed, errbuf, sizeof(errbuf)) != 0) {
    zwarnnam(MODNAME, "zoxide add: %s", errbuf);
    return 1;
  }
  if (parsed.paths_len == 0) {
    zwarnnam(MODNAME, "zoxide add: at least one path is required");
    return 1;
  }

  for (size_t i = 0; i < parsed.paths_len; i++) {
    if (zo_session_add(g_session, parsed.paths[i], parsed.score) != 0)
      return report_ffi_error("zoxide add");
  }
  return 0;
}

static int run_zo_query(char **argv) {
  char errbuf[256];
  zo_parsed_query_t parsed;
  if (zo_parse_query_args(argv, &parsed, errbuf, sizeof(errbuf)) != 0) {
    zwarnnam(MODNAME, "zoxide query: %s", errbuf);
    return 1;
  }

  zo_query_options_t options;
  memset(&options, 0, sizeof(options));
  options.keywords = parsed.keywords;
  options.keywords_len = parsed.keywords_len;
  options.exclude = parsed.exclude;
  options.base_dir = parsed.base_dir;
  options.all = parsed.all;
  options.interactive = parsed.interactive;
  options.list = parsed.list;
  options.score = parsed.score;

  char *out = NULL;
  if (zo_session_query(g_session, &options, &out) != 0 || !out) {
    unsetparam((char *)"ZOXIDE_RESULT");
    return report_ffi_error("zoxide query");
  }

  set_str_param("ZOXIDE_RESULT", out);
  if (parsed.print)
    printf("%s\n", out);
  zo_free(out);
  return 0;
}

static int run_zo_remove(char **argv) {
  /* `zoxide remove` has no options; `--` is accepted for CLI compatibility. */
  if (*argv && strcmp(*argv, "--") == 0)
    argv++;

  size_t path_count = 0;
  for (char **p = argv; *p != NULL; p++)
    path_count++;
  if (path_count == 0) {
    zwarnnam(MODNAME, "zoxide remove: at least one path is required");
    return 1;
  }

  for (char **p = argv; *p != NULL; p++) {
    if (zo_session_remove(g_session, *p) != 0)
      return report_ffi_error("zoxide remove");
  }
  return 0;
}

/* ------------------------------------------------------------------ */
/* Builtin: zoxide (dispatch)                                         */
/* ------------------------------------------------------------------ */

static int bin_zo(UNUSED(char *name), char **argv, UNUSED(Options ops),
                  UNUSED(int func)) {
  if (!g_session) {
    zwarnnam(MODNAME, "session not initialized; module may not have booted "
                      "correctly");
    return 1;
  }
  if (!argv || !*argv) {
    zwarnnam(MODNAME, "usage: zoxide add|query|remove|version|stats [args...]");
    return 1;
  }

  if (strcmp(argv[0], "add") == 0)
    return run_zo_add(argv + 1);
  if (strcmp(argv[0], "query") == 0)
    return run_zo_query(argv + 1);
  if (strcmp(argv[0], "remove") == 0)
    return run_zo_remove(argv + 1);
  if (strcmp(argv[0], "version") == 0)
    return bin_zo_version(name, argv + 1, ops, func);
  if (strcmp(argv[0], "stats") == 0)
    return bin_zo_stats(name, argv + 1, ops, func);

  zwarnnam(MODNAME,
           "unsupported in-process subcommand: %s "
           "(supported: add, query, remove, version, stats)",
           argv[0]);
  return 1;
}

/* ------------------------------------------------------------------ */
/* Builtin: zoxide_add                                                */
/* ------------------------------------------------------------------ */

static int bin_zo_add(UNUSED(char *name), char **argv, UNUSED(Options ops),
                      UNUSED(int func)) {
  if (!g_session) {
    zwarnnam(MODNAME, "session not initialized");
    return 1;
  }
  return run_zo_add(argv);
}

/* ------------------------------------------------------------------ */
/* Builtin: zoxide_query                                              */
/* ------------------------------------------------------------------ */

static int bin_zo_query(UNUSED(char *name), char **argv, UNUSED(Options ops),
                        UNUSED(int func)) {
  if (!g_session) {
    zwarnnam(MODNAME, "session not initialized");
    return 1;
  }
  return run_zo_query(argv);
}

/* ------------------------------------------------------------------ */
/* Builtin: zoxide_remove                                             */
/* ------------------------------------------------------------------ */

static int bin_zo_remove(UNUSED(char *name), char **argv, UNUSED(Options ops),
                         UNUSED(int func)) {
  if (!g_session) {
    zwarnnam(MODNAME, "session not initialized");
    return 1;
  }
  return run_zo_remove(argv);
}

/* ------------------------------------------------------------------ */
/* Builtin: zoxide_version                                            */
/* ------------------------------------------------------------------ */

/**
 * Print the zoxide-native FFI version and expose it as $ZOXIDE_VERSION.
 *   zoxide version       — print version
 *   zoxide version -q    — only set the parameter
 */
static int bin_zo_version(UNUSED(char *name), char **argv, UNUSED(Options ops),
                          UNUSED(int func)) {
  int quiet = 0;
  while (argv && *argv) {
    if (strcmp(*argv, "-q") == 0)
      quiet = 1;
    else if (strcmp(*argv, "--quiet") == 0)
      quiet = 1;
    else {
      zwarnnam(MODNAME, "zoxide version: unknown option: %s", *argv);
      return 1;
    }
    argv++;
  }

  const char *version = zo_version();
  version = version ? version : "unknown";
  set_str_param("ZOXIDE_VERSION", version);

  if (!quiet)
    printf("%s\n", version);
  return 0;
}

/* ------------------------------------------------------------------ */
/* Builtin: zoxide_stats                                              */
/* ------------------------------------------------------------------ */

/**
 * Expose session counters as ZOXIDE_STATS_* parameters and optionally print a
 * human-readable summary.
 */
static int bin_zo_stats(UNUSED(char *name), char **argv, UNUSED(Options ops),
                        UNUSED(int func)) {
  int quiet = 0;
  int verbose = 0;
  while (argv && *argv) {
    if (strcmp(*argv, "-q") == 0 || strcmp(*argv, "--quiet") == 0)
      quiet = 1;
    else if (strcmp(*argv, "-v") == 0 || strcmp(*argv, "--verbose") == 0)
      verbose = 1;
    else {
      zwarnnam(MODNAME, "zoxide stats: unknown option: %s", *argv);
      return 1;
    }
    argv++;
  }

  if (!g_session) {
    zwarnnam(MODNAME, "session not initialized");
    return 1;
  }

  zo_stats_t stats;
  memset(&stats, 0, sizeof(stats));
  if (zo_session_stats(g_session, &stats) != 0)
    return report_ffi_error("zoxide stats");

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
    printf("zoxide_native session stats: adds=%llu queries=%llu removes=%llu "
           "entries=%llu\n",
           stats.adds, stats.queries, stats.removes, stats.entries);
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
