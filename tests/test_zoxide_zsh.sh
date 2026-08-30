#!/usr/bin/env zsh
#
# test_zoxide_zsh.sh — zsh integration test for the zoxide_native module and
# the zoxide-native.plugin.zsh loader.
#
# The builtins exchange all data through zsh variables, so this test also
# verifies that contract.
#
# Usage:
#   MODULE_DIR=/path/to/zsh_src/build zsh tests/test_zoxide_zsh.sh

set -eu

: "${MODULE_DIR:?set MODULE_DIR to the directory containing zoxide_native.so}"

# zsh hardcodes its module suffix to .so on every platform, including macOS.
MODULE_FILE="$MODULE_DIR/zoxide_native.so"
if [[ ! -f "$MODULE_FILE" ]]; then
  print -u2 "zoxide_native module not found in $MODULE_DIR"
  exit 1
fi

SCRIPT_DIR="${0:h}"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/zoxide-zsh-test.XXXXXX")"
DATA_DIR="$TMP_DIR/data"
ALPHA_DIR="$TMP_DIR/projects/alpha-🚀"
BETA_DIR="$TMP_DIR/projects/beta"
GAMMA_DIR="$TMP_DIR/projects/gamma"
DELTA_DIR="$TMP_DIR/projects/delta"
EMOJI_BASE_DIR="$TMP_DIR/🚀-projects"
EMOJI_KEEP_DIR="$EMOJI_BASE_DIR/keep-🚀"
EMOJI_CURRENT_DIR="$EMOJI_BASE_DIR/current-🚀"
CROSS_DIR="$TMP_DIR/projects/cross-process"
START_DIR="$TMP_DIR/start"
mkdir -p "$DATA_DIR" "$ALPHA_DIR" "$BETA_DIR" "$GAMMA_DIR" "$DELTA_DIR"   "$EMOJI_KEEP_DIR" "$EMOJI_CURRENT_DIR" "$CROSS_DIR" "$START_DIR"

cleanup() {
  cd /
  zmodload -u zoxide_native 2>/dev/null || true
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT

export _ZO_DATA_DIR="$DATA_DIR"
_ZO_DOCTOR=0

module_path=("$MODULE_DIR" $module_path)
zmodload zoxide_native || {
  print -u2 "FAIL: could not zmodload zoxide_native from $MODULE_DIR"
  exit 1
}
print "PASS: module loaded from $MODULE_FILE"

# ---- Builtin smoke tests ----------------------------------------------------
zoxide_version
[[ "${ZOXIDE_VERSION}" == "0.1.0" ]] && print "PASS: zoxide version ($ZOXIDE_VERSION)"

zoxide_stats
[[ "${ZOXIDE_STATS_ENTRIES}" == "0" ]] && print "PASS: initial stats"

ZOXIDE_ADD_PATH="$ALPHA_DIR"
ZOXIDE_ADD_SCORE=1
zoxide_add
ZOXIDE_ADD_PATH="$BETA_DIR"
zoxide_add
zoxide_stats
[[ "${ZOXIDE_STATS_ADDS}" == "2" ]] && print "PASS: zoxide add"
[[ "${ZOXIDE_STATS_ENTRIES}" == "2" ]] || {
  print -u2 "FAIL: expected 2 database entries, got ${ZOXIDE_STATS_ENTRIES}"
  exit 1
}

# Reset query options, then run a single-result query.
unset ZOXIDE_QUERY_EXCLUDE ZOXIDE_QUERY_BASE_DIR
typeset -g ZOXIDE_QUERY_ALL=0
typeset -g ZOXIDE_QUERY_INTERACTIVE=0
typeset -g ZOXIDE_QUERY_LIST=0
typeset -g ZOXIDE_QUERY_SCORE=0
typeset -ga ZOXIDE_QUERY_KEYWORDS
ZOXIDE_QUERY_KEYWORDS=(alpha-🚀)
zoxide_query
[[ "${ZOXIDE_RESULT}" == "$ALPHA_DIR" ]] && print "PASS: zoxide query (single)"

typeset -g ZOXIDE_QUERY_ALL=1
typeset -g ZOXIDE_QUERY_LIST=1
zoxide_query
[[ "${ZOXIDE_RESULT}" == "$ALPHA_DIR" ]] && print "PASS: zoxide query (list/all)"

typeset -g ZOXIDE_QUERY_ALL=0
typeset -g ZOXIDE_QUERY_LIST=0
ZOXIDE_QUERY_KEYWORDS=(definitely-no-such-entry)
QUERY_ERR_FILE="$TMP_DIR/query-no-match.err"
if zoxide_query 2>"$QUERY_ERR_FILE"; then
  print -u2 "FAIL: query for a missing entry should fail"
  exit 1
fi
[[ -z "${ZOXIDE_RESULT-}" ]] || {
  print -u2 "FAIL: failed query should clear ZOXIDE_RESULT"
  exit 1
}
[[ "$(<"$QUERY_ERR_FILE")" == "zoxide: no match found" ]] || {
  print -u2 "FAIL: no-match query should print the original binary error, got: $(<"$QUERY_ERR_FILE")"
  exit 1
}
print "PASS: failed query clears ZOXIDE_RESULT and reports no match"

typeset -ga ZOXIDE_REMOVE_PATHS
ZOXIDE_REMOVE_PATHS=("$BETA_DIR")
zoxide_remove
zoxide_stats
[[ "${ZOXIDE_STATS_REMOVES}" == "1" ]] && print "PASS: zoxide remove"
[[ "${ZOXIDE_STATS_ENTRIES}" == "1" ]] || {
  print -u2 "FAIL: expected 1 entry after remove, got ${ZOXIDE_STATS_ENTRIES}"
  exit 1
}

# Array input forms: ZOXIDE_ADD_PATH / ZOXIDE_REMOVE_PATHS may be a path
# scalar or an array of paths. An array must not be joined into one string.
typeset -ga ZOXIDE_ADD_PATH ZOXIDE_REMOVE_PATHS
ZOXIDE_ADD_PATH=("$GAMMA_DIR" "$DELTA_DIR")
zoxide_add
zoxide_stats
[[ "${ZOXIDE_STATS_ADDS}" == "4" ]] || {
  print -u2 "FAIL: array add should add both paths, adds=${ZOXIDE_STATS_ADDS}"
  exit 1
}
[[ "${ZOXIDE_STATS_ENTRIES}" == "3" ]] || {
  print -u2 "FAIL: expected 3 entries after array add, got ${ZOXIDE_STATS_ENTRIES}"
  exit 1
}
ZOXIDE_REMOVE_PATHS=("$GAMMA_DIR" "$DELTA_DIR")
zoxide_remove
zoxide_stats
[[ "${ZOXIDE_STATS_REMOVES}" == "3" ]] || {
  print -u2 "FAIL: array remove should remove both paths, removes=${ZOXIDE_STATS_REMOVES}"
  exit 1
}
[[ "${ZOXIDE_STATS_ENTRIES}" == "1" ]] || {
  print -u2 "FAIL: expected 1 entry after array remove, got ${ZOXIDE_STATS_ENTRIES}"
  exit 1
}
print "PASS: scalar and array path inputs for add/remove"

# Two non-ASCII scalars read back-to-back used to alias each other because
# getsparam_u() returns a shared static buffer. EXCLUDE and BASE_DIR must stay
# independent even when both contain emoji.
typeset -ga ZOXIDE_ADD_PATH ZOXIDE_REMOVE_PATHS ZOXIDE_QUERY_KEYWORDS
ZOXIDE_ADD_PATH=("$EMOJI_KEEP_DIR" "$EMOJI_CURRENT_DIR")
zoxide_add
unset ZOXIDE_QUERY_EXCLUDE ZOXIDE_QUERY_BASE_DIR
typeset -g ZOXIDE_QUERY_ALL=0
typeset -g ZOXIDE_QUERY_INTERACTIVE=0
typeset -g ZOXIDE_QUERY_LIST=1
typeset -g ZOXIDE_QUERY_SCORE=0
ZOXIDE_QUERY_KEYWORDS=()
ZOXIDE_QUERY_EXCLUDE="$EMOJI_CURRENT_DIR"
ZOXIDE_QUERY_BASE_DIR="$EMOJI_BASE_DIR"
zoxide_query
[[ "${ZOXIDE_RESULT}" == "$EMOJI_KEEP_DIR" ]] || {
  print -u2 "FAIL: non-ASCII EXCLUDE/BASE_DIR aliased, got: ${ZOXIDE_RESULT}"
  exit 1
}
print "PASS: non-ASCII EXCLUDE and BASE_DIR are read independently"
ZOXIDE_REMOVE_PATHS=("$EMOJI_KEEP_DIR" "$EMOJI_CURRENT_DIR")
zoxide_remove
zoxide_stats
[[ "${ZOXIDE_STATS_ENTRIES}" == "1" ]] || {
  print -u2 "FAIL: expected 1 entry after emoji base-dir test cleanup, got ${ZOXIDE_STATS_ENTRIES}"
  exit 1
}

# A separate zsh process writes the database while this session stays loaded.
# The next command in this session must reopen db.zo and see that update
# instead of overwriting it with stale in-memory state.
if ! MODULE_DIR="$MODULE_DIR" _ZO_DATA_DIR="$DATA_DIR" CROSS_DIR="$CROSS_DIR" zsh -f -c '
  module_path=("$MODULE_DIR" $module_path)
  zmodload zoxide_native || exit 1
  ZOXIDE_ADD_PATH="$CROSS_DIR"
  zoxide_add || exit 1
  zmodload -u zoxide_native
'; then
  print -u2 "FAIL: child zsh process could not add $CROSS_DIR"
  exit 1
fi
unset ZOXIDE_QUERY_EXCLUDE ZOXIDE_QUERY_BASE_DIR
typeset -g ZOXIDE_QUERY_ALL=0
typeset -g ZOXIDE_QUERY_INTERACTIVE=0
typeset -g ZOXIDE_QUERY_LIST=0
typeset -g ZOXIDE_QUERY_SCORE=0
typeset -ga ZOXIDE_QUERY_KEYWORDS
ZOXIDE_QUERY_KEYWORDS=(cross-process)
zoxide_query
[[ "${ZOXIDE_RESULT}" == "$CROSS_DIR" ]] || {
  print -u2 "FAIL: session did not see another process update, got: ${ZOXIDE_RESULT-}"
  exit 1
}
print "PASS: session sees database updates from another process"
typeset -ga ZOXIDE_REMOVE_PATHS
ZOXIDE_REMOVE_PATHS=("$CROSS_DIR")
zoxide_remove

# ---- Interactive query (fzf) ------------------------------------------------
if (( ! ${+commands[fzf]} )); then
  print "SKIP: fzf not installed"
else
  export _ZO_FZF_OPTS="--filter=alph"
  unset ZOXIDE_QUERY_EXCLUDE ZOXIDE_QUERY_BASE_DIR
  typeset -g ZOXIDE_QUERY_INTERACTIVE=1
  typeset -g ZOXIDE_QUERY_LIST=0
  ZOXIDE_QUERY_KEYWORDS=(alpha-🚀)
  zoxide_query
  [[ "${ZOXIDE_RESULT}" == "$ALPHA_DIR" ]] && print "PASS: zoxide query (interactive/fzf)"
  typeset -g ZOXIDE_QUERY_INTERACTIVE=0
  unset _ZO_FZF_OPTS
fi

# fzf exiting 130 is the Ctrl-C path in the original binary: the query must
# fail without printing anything.
FAKE_BIN_DIR="$TMP_DIR/fake-bin"
mkdir -p "$FAKE_BIN_DIR"
cat > "$FAKE_BIN_DIR/fzf" <<'SH'
#!/bin/sh
exit 130
SH
chmod +x "$FAKE_BIN_DIR/fzf"
SAVED_PATH="$PATH"
PATH="$FAKE_BIN_DIR:$PATH"
export PATH
typeset -g ZOXIDE_QUERY_INTERACTIVE=1
typeset -g ZOXIDE_QUERY_LIST=0
ZOXIDE_QUERY_KEYWORDS=(alpha-🚀)
CTRLC_ERR_FILE="$TMP_DIR/query-ctrlc.err"
if zoxide_query 2>"$CTRLC_ERR_FILE"; then
  print -u2 "FAIL: interactive query with fzf exit 130 should fail"
  exit 1
fi
[[ ! -s "$CTRLC_ERR_FILE" ]] || {
  print -u2 "FAIL: fzf Ctrl-C should be silent, got: $(<"$CTRLC_ERR_FILE")"
  exit 1
}
[[ -z "${ZOXIDE_RESULT-}" ]] || {
  print -u2 "FAIL: fzf Ctrl-C should clear ZOXIDE_RESULT"
  exit 1
}
print "PASS: zoxide query fzf Ctrl-C is silent"
PATH="$SAVED_PATH"
export PATH
typeset -g ZOXIDE_QUERY_INTERACTIVE=0

# ---- Plugin integration -----------------------------------------------------
ZOXIDE_NATIVE_DIR="$MODULE_DIR" source "$SCRIPT_DIR/../zsh_src/zoxide-native.plugin.zsh"
(( ${+functions[__zoxide_z]} )) && print "PASS: plugin defines __zoxide_z"
(( ${+functions[z]} )) && print "PASS: plugin defines z"

# `cd` triggers the plugin's chpwd hook and should add the start directory.
cd "$START_DIR"
zoxide_stats
[[ "${ZOXIDE_STATS_ENTRIES}" == "2" ]] && print "PASS: plugin chpwd hook adds cwd"

__zoxide_z alph
[[ "$PWD" == "$ALPHA_DIR" ]] && print "PASS: __zoxide_z jumped to $ALPHA_DIR"

# A failed jump must print the original binary error and stay in place.
cd "$START_DIR"
Z_ERR_FILE="$TMP_DIR/z-no-match.err"
if __zoxide_z definitely-no-such-entry 2>"$Z_ERR_FILE"; then
  print -u2 "FAIL: __zoxide_z with no match should fail"
  exit 1
fi
[[ "$PWD" == "$START_DIR" ]] || {
  print -u2 "FAIL: failed __zoxide_z changed directory"
  exit 1
}
[[ "$(<"$Z_ERR_FILE")" == "zoxide: no match found" ]] || {
  print -u2 "FAIL: __zoxide_z no match should print the original binary error, got: $(<"$Z_ERR_FILE")"
  exit 1
}
print "PASS: __zoxide_z no-match keeps directory and reports original error"

if (( ${+commands[fzf]} )); then
  cd "$START_DIR"
  export _ZO_FZF_OPTS="--filter=alph"
  __zoxide_zi alph
  [[ "$PWD" == "$ALPHA_DIR" ]] && print "PASS: __zoxide_zi jumped with fzf"
  unset _ZO_FZF_OPTS
fi

# __zoxide_zi with fzf Ctrl-C must be silent and stay in place.
cd "$START_DIR"
ZI_ERR_FILE="$TMP_DIR/zi-ctrlc.err"
PATH="$FAKE_BIN_DIR:$PATH"
export PATH
if __zoxide_zi alpha 2>"$ZI_ERR_FILE"; then
  print -u2 "FAIL: __zoxide_zi with fzf exit 130 should fail"
  exit 1
fi
PATH="$SAVED_PATH"
export PATH
[[ "$PWD" == "$START_DIR" ]] || {
  print -u2 "FAIL: __zoxide_zi Ctrl-C changed directory"
  exit 1
}
[[ ! -s "$ZI_ERR_FILE" ]] || {
  print -u2 "FAIL: __zoxide_zi Ctrl-C should be silent, got: $(<"$ZI_ERR_FILE")"
  exit 1
}
print "PASS: __zoxide_zi fzf Ctrl-C is silent and stays in place"

print "zsh integration test passed"
