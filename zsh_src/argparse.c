/*
 * argparse.c — pure C parsers for the zoxide zsh builtins.
 *
 * Kept free of zsh headers so the parsing logic can be unit-tested with a
 * plain C compiler (see tests/test_argparse.c).
 */

#include "argparse.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int is_long_option(const char *arg, const char *name) {
  size_t len = strlen(name);
  return strncmp(arg, name, len) == 0 && arg[len] == '=';
}

static int take_value(char **argv, int *index, const char *inline_value,
                      const char *option, const char **out, char *errbuf,
                      size_t errbuf_size) {
  if (inline_value != NULL) {
    *out = inline_value;
    return 0;
  }
  if (argv[*index + 1] == NULL) {
    snprintf(errbuf, errbuf_size, "option '%s' requires a value", option);
    return -1;
  }
  (*index)++;
  *out = argv[*index];
  return 0;
}

int zo_parse_query_args(char **argv, zo_parsed_query_t *out, char *errbuf,
                        size_t errbuf_size) {
  memset(out, 0, sizeof(*out));
  int i = 0;

  for (; argv[i] != NULL; i++) {
    const char *arg = argv[i];

    if (strcmp(arg, "--") == 0) {
      i++;
      break;
    }

    if (strncmp(arg, "--", 2) == 0) {
      if (strcmp(arg, "--all") == 0) {
        out->all = 1;
      } else if (strcmp(arg, "--interactive") == 0) {
        out->interactive = 1;
      } else if (strcmp(arg, "--list") == 0) {
        out->list = 1;
      } else if (strcmp(arg, "--score") == 0) {
        out->score = 1;
      } else if (strcmp(arg, "--print") == 0) {
        out->print = 1;
      } else if (strcmp(arg, "--exclude") == 0) {
        if (take_value(argv, &i, NULL, "--exclude", &out->exclude, errbuf,
                       errbuf_size) != 0)
          return -1;
      } else if (is_long_option(arg, "--exclude")) {
        out->exclude = arg + strlen("--exclude=");
      } else if (strcmp(arg, "--base-dir") == 0) {
        if (take_value(argv, &i, NULL, "--base-dir", &out->base_dir, errbuf,
                       errbuf_size) != 0)
          return -1;
      } else if (is_long_option(arg, "--base-dir")) {
        out->base_dir = arg + strlen("--base-dir=");
      } else {
        snprintf(errbuf, errbuf_size, "unknown option: %s", arg);
        return -1;
      }
      continue;
    }

    if (arg[0] == '-' && arg[1] != '\0') {
      for (const char *p = arg + 1; *p != '\0'; p++) {
        switch (*p) {
        case 'a':
          out->all = 1;
          break;
        case 'i':
          out->interactive = 1;
          break;
        case 'l':
          out->list = 1;
          break;
        case 's':
          out->score = 1;
          break;
        case 'p':
          out->print = 1;
          break;
        case 'e':
          if (p[1] != '\0') {
            out->exclude = p + 1;
          } else if (take_value(argv, &i, NULL, "-e", &out->exclude, errbuf,
                                errbuf_size) != 0) {
            return -1;
          }
          p = arg + strlen(arg) - 1;
          break;
        default:
          snprintf(errbuf, errbuf_size, "unknown option: -%c", *p);
          return -1;
        }
      }
      continue;
    }

    /* First non-option: remaining arguments are keywords. */
    break;
  }

  out->keywords = (const char *const *)&argv[i];
  out->keywords_len = 0;
  while (argv[i] != NULL) {
    i++;
    out->keywords_len++;
  }

  if (out->interactive && out->list) {
    snprintf(errbuf, errbuf_size,
             "options '--interactive' and '--list' cannot be used together");
    return -1;
  }

  return 0;
}

int zo_parse_add_args(char **argv, zo_parsed_add_t *out, char *errbuf,
                      size_t errbuf_size) {
  memset(out, 0, sizeof(*out));
  out->score = 1.0;
  int i = 0;

  for (; argv[i] != NULL; i++) {
    const char *arg = argv[i];

    if (strcmp(arg, "--") == 0) {
      i++;
      break;
    }

    if (strncmp(arg, "--", 2) == 0) {
      if (strcmp(arg, "--score") == 0) {
        if (argv[i + 1] == NULL) {
          snprintf(errbuf, errbuf_size, "option '--score' requires a value");
          return -1;
        }
        i++;
        const char *value = argv[i];
        char *end = NULL;
        out->score = strtod(value, &end);
        if (end == value || *end != '\0') {
          snprintf(errbuf, errbuf_size, "invalid score: %s", value);
          return -1;
        }
      } else if (is_long_option(arg, "--score")) {
        const char *value = arg + strlen("--score=");
        char *end = NULL;
        out->score = strtod(value, &end);
        if (end == value || *end != '\0') {
          snprintf(errbuf, errbuf_size, "invalid score: %s", value);
          return -1;
        }
      } else {
        snprintf(errbuf, errbuf_size, "unknown option: %s", arg);
        return -1;
      }
      continue;
    }

    if (arg[0] == '-' && arg[1] != '\0') {
      if (arg[1] == 's' && arg[2] == '\0') {
        if (argv[i + 1] == NULL) {
          snprintf(errbuf, errbuf_size, "option '-s' requires a value");
          return -1;
        }
        i++;
      } else if (arg[1] == 's') {
        /* -s1.5 form */
      } else {
        snprintf(errbuf, errbuf_size, "unknown option: %s", arg);
        return -1;
      }

      char *end = NULL;
      const char *value = (arg[1] == 's' && arg[2] != '\0') ? arg + 2 : argv[i];
      out->score = strtod(value, &end);
      if (end == value || *end != '\0') {
        snprintf(errbuf, errbuf_size, "invalid score: %s", value);
        return -1;
      }
      continue;
    }

    break;
  }

  out->paths = &argv[i];
  out->paths_len = 0;
  while (argv[i] != NULL) {
    i++;
    out->paths_len++;
  }

  return 0;
}
