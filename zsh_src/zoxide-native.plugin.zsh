# zoxide-native.plugin.zsh — in-process zoxide for zsh.
#
# A plugin-manager friendly loader, compatible with oh-my-zsh, zinit, antigen,
# zplug, zgen, sheldon, ... — just point your plugin manager at this repository
# and the *.plugin.zsh naming convention makes it pick this file up.
#
# Before this plugin can do anything you must have built & installed the
# compiled module (see README):
#
#     cmake -B build -S .
#     cmake --build build --config Release
#     cmake --install build --config Release --prefix ~/.local
#
# Compiled module discovery order:
#   1. $ZOXIDE_NATIVE_DIR                   — explicit override
#   2. this script's own directory          — `cmake --install` layout
#   3. common install prefixes              — ~/.local/lib/zsh/zoxide-native, ...
#
# The plugin mirrors the official `zoxide init zsh` script, replacing the
# `\command zoxide add|query` subprocess calls with the in-process `zoxide`
# builtin (loaded via zmodload). `zoxide query` writes its result to
# $ZOXIDE_RESULT and returns status 0/1, so no command substitution (and
# therefore no fork) is required for the hot paths.

# Prevent double-loading
if ((${+_ZOXIDE_NATIVE_LOADED})); then
  return 0
fi
typeset -g _ZOXIDE_NATIVE_LOADED=1

# Locate this script's directory (works under any plugin manager).
0="${${ZERO:-${0:#$ZSH_ARGZERO}}:-${(%):-%N}}"
0="${${(M)0:#/*}:-$PWD/$0}"
typeset -g _ZOXIDE_NATIVE_SCRIPT_DIR="${0:h}"

# ---- Compiled module discovery ---------------------------------------------
# zsh hardcodes the loadable-module suffix to ".so" via its DL_EXT macro on
# EVERY platform, including macOS (see zsh's configure.ac: DL_EXT="${DL_EXT=so}").
_zoxide_native_mods=(zoxide_native.so)

case "${OSTYPE:-}" in
darwin*)
  _zoxide_native_ffis=(libzoxide_ffi.dylib)
  ;;
*)
  _zoxide_native_ffis=(libzoxide_ffi.so)
  ;;
esac

typeset -ga _zoxide_native_dirs
if [[ -n "${ZOXIDE_NATIVE_DIR:-}" ]]; then
  _zoxide_native_dirs+=("$ZOXIDE_NATIVE_DIR")
fi
_zoxide_native_dirs+=(
  "$_ZOXIDE_NATIVE_SCRIPT_DIR"
  "${XDG_DATA_HOME:-$HOME/.local/share}/zsh/zoxide-native"
  "$HOME/.local/lib/zsh/zoxide-native"
  "/usr/local/lib/zsh/zoxide-native"
  "/usr/lib/zsh/zoxide-native"
  "/opt/zoxide-native/lib/zsh/zoxide-native"
)

typeset -g ZOXIDE_NATIVE_DIR=""
typeset -g ZOXIDE_NATIVE_FFI=""

typeset _dir _mod _ffi _found=0
for _dir in "${_zoxide_native_dirs[@]}"; do
  [[ -d "$_dir" ]] || continue
  for _mod in "${_zoxide_native_mods[@]}"; do
    if [[ -f "$_dir/$_mod" ]]; then
      ZOXIDE_NATIVE_DIR="$_dir"
      _found=1
      for _ffi in "${_zoxide_native_ffis[@]}"; do
        if [[ -f "$_dir/$_ffi" ]]; then
          ZOXIDE_NATIVE_FFI="$_dir/$_ffi"
          break
        fi
      done
      break
    fi
  done
  ((_found)) && break
done

if [[ -z "$ZOXIDE_NATIVE_DIR" ]]; then
  print -u2 "zoxide-native: compiled module (zoxide_native) not found."
  print -u2 "  Build it first:"
  print -u2 "    cmake -B build -S . && cmake --build build --config Release"
  print -u2 "    cmake --install build --config Release --prefix \$HOME/.local"
  print -u2 "  Or point ZOXIDE_NATIVE_DIR at the installed lib/zsh directory."
  unset _zoxide_native_dirs _zoxide_native_mods _zoxide_native_ffis
  return 1
fi

# ---- Load the native module ----
module_path=("$ZOXIDE_NATIVE_DIR" $module_path)
zmodload zoxide_native || {
  print -u2 "zoxide-native: failed to load zoxide_native from $ZOXIDE_NATIVE_DIR"
  return 1
}

# ---- The rest is adapted from zoxide init zsh -------------------------------

# pwd based on the value of _ZO_RESOLVE_SYMLINKS.
function __zoxide_pwd() {
  if [[ ${_ZO_RESOLVE_SYMLINKS:-0} == "1" ]]; then
    \builtin pwd -P
  else
    \builtin pwd -L
  fi
}

# cd + custom logic based on the value of _ZO_ECHO.
function __zoxide_cd() {
  # shellcheck disable=SC2164
  \builtin cd -- "$@"
  local _zo_status=$?
  if ((_zo_status == 0)) && [[ ${_ZO_ECHO:-0} == "1" ]]; then
    __zoxide_pwd
  fi
  return _zo_status
}

# Hook to add new entries to the database.
function __zoxide_hook() {
  local _zo_pwd
  _zo_pwd="$(__zoxide_pwd)" || return 0
  [[ -n "$_zo_pwd" ]] || return 0
  zoxide add -- "$_zo_pwd"
}

# Initialize hook.
\builtin typeset -ga precmd_functions
precmd_functions=("${(@)precmd_functions:#__zoxide_hook}")
precmd_functions+=(__zoxide_hook)

# Report common issues (mirrors `zoxide init zsh`).
function __zoxide_doctor() {
  [[ ${_ZO_DOCTOR:-1} -ne 0 ]] || return 0
  [[ $- == *i* ]] || return 0
  [[ ${precmd_functions[(Ie)__zoxide_hook]:-} -eq 0 ]] || return 0

  _ZO_DOCTOR=0
  \builtin printf '%s\n' \
    'zoxide: detected a possible configuration issue.' \
    'Please ensure that zoxide is initialized right at the end of your shell configuration file (usually ~/.zshrc).' \
    '' \
    'If the issue persists, consider filing an issue at:' \
    'https://github.com/ajeetdsouza/zoxide/issues' \
    '' \
    'Disable this message by setting _ZO_DOCTOR=0.' \
    '' >&2
}

# Jump to a directory using only keywords.
function __zoxide_z() {
  __zoxide_doctor
  if [[ "$" -eq 0 ]]; then
    __zoxide_cd ~
  elif [[ "$" -eq 1 ]] && [[ "$1" = '-' ]]; then
    __zoxide_cd "${OLDPWD}"
  elif [[ "$" -eq 1 ]] && { [[ "$1" =~ ^[-+][0-9]+$ ]] || (\builtin cd -q -- "$1") &>/dev/null; }; then
    __zoxide_cd "$1"
  elif [[ "$" -eq 2 ]] && [[ "$1" = "--" ]]; then
    __zoxide_cd "$2"
  else
    if zoxide query --exclude "$(__zoxide_pwd)" -- "$@"; then
      __zoxide_cd "${ZOXIDE_RESULT}"
    fi
  fi
}

# Jump to a directory using interactive search.
function __zoxide_zi() {
  __zoxide_doctor
  if zoxide query --interactive -- "$@"; then
    __zoxide_cd "${ZOXIDE_RESULT}"
  fi
}

# Commands for zoxide. Disable these with `typeset -g ZOXIDE_NO_CMD=1` before
# loading the plugin.
if [[ ${ZOXIDE_NO_CMD:-0} != "1" ]]; then
  function z() { __zoxide_z "$@"; }
  function zi() { __zoxide_zi "$@"; }
fi

# Completions.
if [[ -o zle ]]; then
  __zoxide_result=''

  function __zoxide_z_complete() {
    # Only show completions when the cursor is at the end of the line.
    [[ "${#words[@]}" -eq "${CURRENT}" ]] || return 0

    if [[ "${#words[@]}" -eq 2 ]]; then
      # Show completions for local directories.
      _cd -/

    elif [[ "${words[-1]}" == '' ]]; then
      # Show completions for Space-Tab. Call the builtin directly so the
      # database session stays in this shell (no fork).
      if zoxide query --exclude "$(__zoxide_pwd || \builtin true)" --interactive -- ${words[2,-1]} 2>/dev/null; then
        __zoxide_result="${ZOXIDE_RESULT}"
      else
        __zoxide_result=''
      fi

      # Set a result to ensure completion doesn't re-run.
      compadd -Q -S "" -- ""

      # Bind '\e[0n' to helper function.
      \builtin bindkey '\e[0n' '__zoxide_z_complete_helper'
      # Sends query device status code, which results in a '\e[0n' being sent to console input.
      \builtin printf '\e[5n'

      # Report that the completion was successful, so that we don't fall back
      # to another completion function.
      return 0
    fi
  }

  function __zoxide_z_complete_helper() {
    if [[ -n "${__zoxide_result}" ]]; then
      BUFFER="cd ${(q-)__zoxide_result}"
      __zoxide_result=''
      \builtin zle reset-prompt
      \builtin zle accept-line
    else
      \builtin zle reset-prompt
    fi
  }
  \builtin zle -N __zoxide_z_complete_helper

  [[ "${+functions[compdef]}" -ne 0 ]] && \compdef __zoxide_z_complete z
fi

# To initialize zoxide, add this to your shell configuration file (usually ~/.zshrc).
# The plugin manager loads this file, so no `zoxide init zsh` eval is required.
