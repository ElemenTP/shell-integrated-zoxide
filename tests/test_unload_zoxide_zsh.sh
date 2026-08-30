#!/usr/bin/env zsh
#
# test_unload_zoxide_zsh.sh — zsh unload/reload regression test.
#
# The module owns no threads or global runtime, so zmodload -u must be safe and
# the next zmodload must create a fresh session against the same database.

set -eu

: "${MODULE_DIR:?set MODULE_DIR to the directory containing zoxide_native.so}"

SCRIPT_DIR="${0:h}"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/zoxide-unload-test.XXXXXX")"
DATA_DIR="$TMP_DIR/data"
TARGET_DIR="$TMP_DIR/target"
mkdir -p "$DATA_DIR" "$TARGET_DIR"

cleanup() {
  cd /
  zmodload -u zoxide_native 2>/dev/null || true
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT

export _ZO_DATA_DIR="$DATA_DIR"
module_path=("$MODULE_DIR" $module_path)

# A failed plugin load must be retryable: the loaded marker is only set after
# zmodload succeeds. Run in a subshell so the plugin's hooks don't leak into
# the direct zmodload loop below.
(
  ZOXIDE_NATIVE_DIR="$TMP_DIR/does-not-exist"
  if source "$SCRIPT_DIR/../zsh_src/zoxide-native.plugin.zsh" >/dev/null 2>&1; then
    print -u2 "FAIL: plugin load with a bad module dir should fail"
    exit 1
  fi
  (( ${+_ZOXIDE_NATIVE_LOADED} )) && {
    print -u2 "FAIL: failed plugin load must not set _ZOXIDE_NATIVE_LOADED"
    exit 1
  }
  ZOXIDE_NATIVE_DIR="$MODULE_DIR"
  source "$SCRIPT_DIR/../zsh_src/zoxide-native.plugin.zsh" || {
    print -u2 "FAIL: retry plugin load with a valid module dir"
    exit 1
  }
  (( ${+_ZOXIDE_NATIVE_LOADED} )) || {
    print -u2 "FAIL: successful plugin load should set _ZOXIDE_NATIVE_LOADED"
    exit 1
  }
  zmodload -u zoxide_native
) || exit 1
print "PASS: failed plugin load can be retried"

for i in 1 2 3; do
  zmodload zoxide_native || {
    print -u2 "FAIL: load iteration $i"
    exit 1
  }

  ZOXIDE_ADD_PATH="$TARGET_DIR"
  zoxide_add

  unset ZOXIDE_QUERY_EXCLUDE ZOXIDE_QUERY_BASE_DIR
  typeset -g ZOXIDE_QUERY_ALL=0
  typeset -g ZOXIDE_QUERY_INTERACTIVE=0
  typeset -g ZOXIDE_QUERY_LIST=0
  typeset -g ZOXIDE_QUERY_SCORE=0
  typeset -ga ZOXIDE_QUERY_KEYWORDS
  ZOXIDE_QUERY_KEYWORDS=(target)
  zoxide_query
  [[ "${ZOXIDE_RESULT}" == "$TARGET_DIR" ]] || {
    print -u2 "FAIL: query iteration $i got ${ZOXIDE_RESULT-}"
    exit 1
  }

  zmodload -u zoxide_native || {
    print -u2 "FAIL: unload iteration $i"
    exit 1
  }
  print "PASS: unload/reload iteration $i"
done

print "zsh unload/reload test passed"
