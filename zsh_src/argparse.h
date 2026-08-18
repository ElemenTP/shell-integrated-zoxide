/*
 * argparse.h — pure C parsers for the zoxide zsh builtins.
 *
 * These functions have no zsh dependencies and are unit-tested separately in
 * tests/test_argparse.c.
 */
#ifndef ZOXIDE_ARGPARSE_H
#define ZOXIDE_ARGPARSE_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Parsed `zoxide query` arguments. String pointers refer to the original
 * argv memory and remain valid until the caller destroys argv. */
typedef struct {
  const char *exclude;
  const char *base_dir;
  const char *const *keywords;
  size_t keywords_len;
  int all;
  int interactive;
  int list;
  int score;
  int print; /* extension: also print the result to stdout */
} zo_parsed_query_t;

/* Parse `zoxide query [options] [--] KEYWORDS...`.
 * Returns 0 on success, -1 on error (message in errbuf). */
int zo_parse_query_args(char **argv, zo_parsed_query_t *out, char *errbuf,
                        size_t errbuf_size);

/* Parsed `zoxide add` arguments. */
typedef struct {
  double score;
  char *const *paths;
  size_t paths_len;
} zo_parsed_add_t;

/* Parse `zoxide add [--score N] [--] PATH...`.
 * Returns 0 on success, -1 on error (message in errbuf). */
int zo_parse_add_args(char **argv, zo_parsed_add_t *out, char *errbuf,
                      size_t errbuf_size);

#ifdef __cplusplus
}
#endif

#endif /* ZOXIDE_ARGPARSE_H */
