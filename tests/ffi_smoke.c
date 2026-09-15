/*
 * ffi_smoke.c — system test for the zoxide-ffi shared library.
 *
 * Builds against zsh_src/ffi.h and loads the compiled cdylib with the platform
 * dynamic loader. This verifies the real exported ABI (names, struct layout,
 * memory ownership, error-return convention) without going through zsh or
 * pwsh.
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
  zo_session_t *session = NULL;
  char *out = NULL;
  char *target = NULL;
  char *data_dir = NULL;
  char *err = NULL;
  char version_buf[64] = {0};
  int rc = EXIT_FAILURE;

  typedef char *(*session_create_fn)(zo_session_t **);
  typedef char *(*session_destroy_fn)(zo_session_t *);
  typedef char *(*session_add_fn)(zo_session_t *, const char *, double);
  typedef char *(*session_query_fn)(zo_session_t *, const zo_query_options_t *,
                                    char **);
  typedef char *(*session_stats_fn)(zo_session_t *, zo_stats_t *);
  typedef void (*free_fn)(char *);
  typedef const char *(*version_fn)(void);
  session_create_fn session_create = NULL;
  session_destroy_fn session_destroy = NULL;
  session_add_fn session_add = NULL;
  session_query_fn session_query = NULL;
  session_stats_fn session_stats = NULL;
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

  session_create = (session_create_fn)DL_SYM(lib, "zo_session_create");
  session_destroy = (session_destroy_fn)DL_SYM(lib, "zo_session_destroy");
  session_add = (session_add_fn)DL_SYM(lib, "zo_session_add");
  session_query = (session_query_fn)DL_SYM(lib, "zo_session_query");
  session_stats = (session_stats_fn)DL_SYM(lib, "zo_session_stats");
  zo_free = (free_fn)DL_SYM(lib, "zo_free");
  zo_version = (version_fn)DL_SYM(lib, "zo_version");
  CHECK(session_create && session_destroy && session_add && session_query &&
        session_stats && zo_free && zo_version);

  const char *version = zo_version();
  CHECK(version != NULL && version[0] != '\0');
  snprintf(version_buf, sizeof(version_buf), "%s", version);

#ifdef _WIN32
  CHECK(_putenv_s("_ZO_DATA_DIR", data_dir) == 0);
#else
  CHECK(setenv("_ZO_DATA_DIR", data_dir, 1) == 0);
#endif

  /* The handle is the payload, so it comes back through the out-parameter
   * and the return value is the error. */
  err = session_create(&session);
  CHECK(err == NULL);
  CHECK(session != NULL);

  /* A failing call returns its own message; NULL would mean success. */
  err = session_add(session, NULL, 1.0);
  CHECK(err != NULL && *err != '\0');
  zo_free(err);
  err = NULL;

  /* Same for a call with no session at all. */
  err = session_add(NULL, target, 1.0);
  CHECK(err != NULL && *err != '\0');
  zo_free(err);
  err = NULL;

  CHECK(session_add(session, target, 1.0) == NULL);

  const char *keyword = strrchr(target, '/');
  keyword = keyword ? keyword + 1 : target;
  const char *keywords[] = {keyword};
  zo_query_options_t options;
  memset(&options, 0, sizeof(options));
  options.keywords = keywords;
  options.keywords_len = 1;

  CHECK(session_query(session, &options, &out) == NULL);
  CHECK(out != NULL && strstr(out, target) != NULL);
  zo_free(out);
  out = NULL;

  /* A failing query reports its error and clears the caller's output slot. */
  zo_query_options_t conflicting = options;
  conflicting.interactive = 1;
  conflicting.list = 1;
  err = session_query(session, &conflicting, &out);
  CHECK(err != NULL && *err != '\0');
  CHECK(out == NULL);
  zo_free(err);
  err = NULL;

  zo_stats_t stats;
  memset(&stats, 0, sizeof(stats));
  CHECK(session_stats(session, &stats) == NULL);
  CHECK(stats.adds == 1 && stats.queries == 1 && stats.entries == 1);

  /* Teardown reports errors as well (a panic in the destructor is the only
   * cause), so the message is checked and released like any other. */
  err = session_destroy(session);
  session = NULL; /* set before CHECK so the fail path cannot double-destroy */
  CHECK(err == NULL);
  zo_free(err);
  err = NULL;

  DL_CLOSE(lib);
  lib = NULL;

  printf("ffi smoke test passed (lib=%s, version=%s)\n", argv[1], version_buf);
  rc = EXIT_SUCCESS;

fail:
  if (out && zo_free)
    zo_free(out);
  if (err && zo_free)
    zo_free(err);
  if (session && session_destroy && zo_free) {
    char *destroy_err = session_destroy(session);
    if (destroy_err)
      zo_free(destroy_err);
  }
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
