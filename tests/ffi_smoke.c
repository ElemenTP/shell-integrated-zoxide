/*
 * ffi_smoke.c — system test for the zoxide-ffi shared library.
 *
 * Builds against zsh_src/ffi.h and loads the compiled cdylib with the platform
 * dynamic loader. This verifies the real exported ABI (names, struct layout,
 * memory ownership, error-return convention, global-session lifecycle) without
 * going through zsh or pwsh.
 *
 * Error convention under test: every call that can fail returns the error
 * message itself — NULL means success, a non-NULL pointer is a Rust-allocated
 * string the caller frees with zo_free().
 */

#include "../zsh_src/ffi.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#include <windows.h>
#define DL_HANDLE HMODULE
#define DL_OPEN(path) LoadLibraryA(path)
#define DL_SYM(handle, name) GetProcAddress(handle, name)
#define DL_CLOSE(handle) FreeLibrary(handle)
#else
#include <dlfcn.h>
#define DL_HANDLE void *
#define DL_OPEN(path) dlopen(path, RTLD_NOW | RTLD_LOCAL)
#define DL_SYM(handle, name) dlsym(handle, name)
#define DL_CLOSE(handle) dlclose(handle)
#endif

#define CHECK(cond)                                                            \
  do {                                                                         \
    if (!(cond)) {                                                             \
      fprintf(stderr, "FAIL %s:%d: %s\n", __FILE__, __LINE__, #cond);          \
      goto fail;                                                               \
    }                                                                          \
  } while (0)

int main(int argc, char **argv) {
  if (argc != 2) {
    fprintf(stderr, "usage: %s /path/to/libzoxide_ffi.{so,dylib,dll}\n",
            argv[0]);
    return EXIT_FAILURE;
  }

  DL_HANDLE lib = NULL;
  char *out = NULL;
  char *target = NULL;
  char *data_dir = NULL;
  char *err = NULL;
  char version_buf[64] = {0};
  int initialized = 0;
  int rc = EXIT_FAILURE;

  typedef char *(*init_fn)(void);
  typedef char *(*shutdown_fn)(void);
  typedef char *(*add_fn)(const char *, double);
  typedef char *(*remove_fn)(const char *);
  typedef char *(*query_fn)(const zo_query_options_t *, char **);
  typedef char *(*stats_fn)(zo_stats_t *);
  typedef void (*free_fn)(char *);
  typedef const char *(*version_fn)(void);
  init_fn zo_init = NULL;
  shutdown_fn zo_shutdown = NULL;
  add_fn zo_add = NULL;
  remove_fn zo_remove = NULL;
  query_fn zo_query = NULL;
  stats_fn zo_stats = NULL;
  free_fn zo_free = NULL;
  version_fn zo_version = NULL;

#ifdef _WIN32
  char data_template[] = "zoxide_ffi_smoke_XXXXXX";
  char target_template[] = "zoxide_ffi_target_XXXXXX";
  data_dir = _mktemp(data_template);
  if (!data_dir || CreateDirectoryA(data_dir, NULL) == 0)
    goto fail;
  target = _mktemp(target_template);
  if (!target || CreateDirectoryA(target, NULL) == 0)
    goto fail;
#else
  char data_template[] = "/tmp/zoxide_ffi_smoke_XXXXXX";
  char target_template[] = "/tmp/zoxide_ffi_target_XXXXXX";
  data_dir = mkdtemp(data_template);
  CHECK(data_dir != NULL);
  target = mkdtemp(target_template);
  CHECK(target != NULL);
#endif

  lib = DL_OPEN(argv[1]);
  CHECK(lib != NULL);

  zo_init = (init_fn)DL_SYM(lib, "zo_init");
  zo_shutdown = (shutdown_fn)DL_SYM(lib, "zo_shutdown");
  zo_add = (add_fn)DL_SYM(lib, "zo_add");
  zo_remove = (remove_fn)DL_SYM(lib, "zo_remove");
  zo_query = (query_fn)DL_SYM(lib, "zo_query");
  zo_stats = (stats_fn)DL_SYM(lib, "zo_stats");
  zo_free = (free_fn)DL_SYM(lib, "zo_free");
  zo_version = (version_fn)DL_SYM(lib, "zo_version");
  CHECK(zo_init && zo_shutdown && zo_add && zo_remove && zo_query && zo_stats &&
        zo_free && zo_version);

  const char *version = zo_version();
  CHECK(version != NULL && version[0] != '\0');
  snprintf(version_buf, sizeof(version_buf), "%s", version);

#ifdef _WIN32
  CHECK(_putenv_s("_ZO_DATA_DIR", data_dir) == 0);
#else
  CHECK(setenv("_ZO_DATA_DIR", data_dir, 1) == 0);
#endif

  /* Data operations before zo_init report the missing session. */
  err = zo_add(target, 1.0);
  CHECK(err != NULL && *err != '\0');
  CHECK(strstr(err, "not initialized") != NULL);
  zo_free(err);
  err = NULL;

  /* One process-global session; initialization is a plain success/failure. */
  err = zo_init();
  CHECK(err == NULL);
  initialized = 1;

  /* A failing call returns its own message; NULL would mean success. */
  err = zo_add(NULL, 1.0);
  CHECK(err != NULL && *err != '\0');
  zo_free(err);
  err = NULL;

  CHECK(zo_add(target, 1.0) == NULL);

  const char *keyword = strrchr(target, '/');
  keyword = keyword ? keyword + 1 : target;
  const char *keywords[] = {keyword};
  zo_query_options_t options;
  memset(&options, 0, sizeof(options));
  options.keywords = keywords;
  options.keywords_len = 1;

  CHECK(zo_query(&options, &out) == NULL);
  CHECK(out != NULL && strstr(out, target) != NULL);
  zo_free(out);
  out = NULL;

  /* A failing query reports its error and clears the caller's output slot. */
  zo_query_options_t conflicting = options;
  conflicting.interactive = 1;
  conflicting.list = 1;
  err = zo_query(&conflicting, &out);
  CHECK(err != NULL && *err != '\0');
  CHECK(out == NULL);
  zo_free(err);
  err = NULL;

  zo_stats_t stats;
  memset(&stats, 0, sizeof(stats));
  CHECK(zo_stats(&stats) == NULL);
  CHECK(stats.adds == 1 && stats.queries == 1 && stats.entries == 1);

  CHECK(zo_remove(target) == NULL);
  memset(&stats, 0, sizeof(stats));
  CHECK(zo_stats(&stats) == NULL);
  CHECK(stats.removes == 1 && stats.entries == 0);

  /* zo_shutdown drops the global session and resets its counters. */
  err = zo_shutdown();
  CHECK(err == NULL);
  initialized = 0;

  err = zo_add(target, 1.0);
  CHECK(err != NULL && *err != '\0');
  CHECK(strstr(err, "not initialized") != NULL);
  zo_free(err);
  err = NULL;

  /* The module can be reloaded: init after shutdown works again, and the
   * database stays usable across the destroy/create cycle. */
  CHECK(zo_init() == NULL);
  initialized = 1;
  CHECK(zo_add(target, 1.0) == NULL);
  CHECK(zo_query(&options, &out) == NULL);
  CHECK(out != NULL && strstr(out, target) != NULL);
  zo_free(out);
  out = NULL;
  err = zo_shutdown();
  CHECK(err == NULL);
  initialized = 0;

  DL_CLOSE(lib);
  lib = NULL;

  printf("ffi smoke test passed (lib=%s, version=%s)\n", argv[1], version_buf);
  rc = EXIT_SUCCESS;

fail:
  if (out && zo_free)
    zo_free(out);
  if (err && zo_free)
    zo_free(err);
  if (initialized && zo_shutdown)
    zo_shutdown();
  if (lib)
    DL_CLOSE(lib);
  if (target) {
    remove(target);
  }
  if (data_dir) {
    char db_path[1024];
    snprintf(db_path, sizeof(db_path), "%s/db.zo", data_dir);
    remove(db_path);
    remove(data_dir);
  }
  return rc;
}
