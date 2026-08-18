cmake_minimum_required(VERSION 3.21)
project(zoxide-native
    VERSION 0.1.0
    DESCRIPTION "zoxide as native shell plugins (zsh + pwsh)"
    LANGUAGES C)

# Always generate compile_commands.json for tooling (LSP, clangd, etc.)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

# ── Global options ────────────────────────────────────────────────────────
option(BUILD_ZSH_MODULE   "Build the zsh loadable module (Linux/macOS)"     ON)
option(BUILD_PWSH_MODULE  "Build the PowerShell binary module"              ON)
option(BUILD_TESTS        "Build test executables"                          ON)

if(WIN32)
    message(STATUS "zsh loadable modules are not supported on Windows — disabling zsh build")
    set(BUILD_ZSH_MODULE OFF)
endif()

# zoxide source: "AUTO" (use repo symlink / clone from GitHub) or local path.
# Unlike the starship project, zoxide upstream is binary-only, so the Rust FFI
# crate includes the required source modules via #[path] through this symlink.
set(ZOXIDE_SOURCE "AUTO" CACHE STRING
    "zoxide source: 'AUTO' to use the zoxide symlink (or clone from GitHub), or a local path")
# zsh source: "AUTO" (use repo symlink / download zsh 5.9.2) or local path.
set(ZSH_SOURCE "AUTO" CACHE STRING
    "zsh source: 'AUTO' to use the zsh symlink (or download zsh 5.9.2), or path to configured source tree")
set(ZSH_VERSION "5.9.2" CACHE STRING "zsh version to download (when ZSH_SOURCE=AUTO)")

include(FetchContent)

# ── Platform detection ────────────────────────────────────────────────────
get_property(_is_multi_config GLOBAL PROPERTY GENERATOR_IS_MULTI_CONFIG)
if(_is_multi_config)
    message(STATUS "Multi-config generator detected — use --config <Release|Debug>")
endif()

# ── Find required tools ───────────────────────────────────────────────────
find_program(CARGO cargo REQUIRED)
find_program(DOTNET dotnet)

if(BUILD_PWSH_MODULE AND NOT DOTNET)
    message(WARNING "dotnet not found — PowerShell module will not be built")
    set(BUILD_PWSH_MODULE OFF)
endif()

# ══════════════════════════════════════════════════════════════════════════
# zoxide source
# ══════════════════════════════════════════════════════════════════════════
set(ZOXIDE_LINK "${CMAKE_SOURCE_DIR}/zoxide")
if(ZOXIDE_SOURCE STREQUAL "AUTO")
    if(EXISTS "${ZOXIDE_LINK}/Cargo.toml" AND EXISTS "${ZOXIDE_LINK}/src/config.rs")
        get_filename_component(ZOXIDE_SRC_DIR "${ZOXIDE_LINK}" REALPATH)
        message(STATUS "Using existing zoxide checkout: ${ZOXIDE_SRC_DIR}")
    else()
        message(STATUS "Fetching zoxide from GitHub...")
        FetchContent_Declare(zoxide-src
            GIT_REPOSITORY "https://github.com/ajeetdsouza/zoxide"
            GIT_TAG        v0.10.0
            GIT_SHALLOW    TRUE
        )
        FetchContent_MakeAvailable(zoxide-src)
        set(ZOXIDE_SRC_DIR "${zoxide-src_SOURCE_DIR}")
    endif()
else()
    get_filename_component(ZOXIDE_SRC_DIR "${ZOXIDE_SOURCE}" ABSOLUTE)
    if(NOT EXISTS "${ZOXIDE_SRC_DIR}/Cargo.toml")
        message(FATAL_ERROR "ZOXIDE_SOURCE=${ZOXIDE_SOURCE} does not contain Cargo.toml")
    endif()
endif()
if(NOT EXISTS "${ZOXIDE_SRC_DIR}/src/config.rs")
    message(FATAL_ERROR "zoxide source at ${ZOXIDE_SRC_DIR} is missing src/config.rs")
endif()

# Keep rust_src's relative #[path] include stable: repo-root/zoxide must point
# at the selected source tree.
get_filename_component(_zoxide_real "${ZOXIDE_LINK}" REALPATH)
if(NOT _zoxide_real STREQUAL "${ZOXIDE_SRC_DIR}")
    file(REMOVE "${ZOXIDE_LINK}")
    file(CREATE_LINK "${ZOXIDE_SRC_DIR}" "${ZOXIDE_LINK}" SYMBOLIC)
    message(STATUS "zoxide symlink: ${ZOXIDE_LINK} -> ${ZOXIDE_SRC_DIR}")
endif()
message(STATUS "zoxide source: ${ZOXIDE_SRC_DIR}")

# ══════════════════════════════════════════════════════════════════════════
# zsh source (for building the zsh module — headers only)
# ══════════════════════════════════════════════════════════════════════════
if(BUILD_ZSH_MODULE)
    set(ZSH_LINK "${CMAKE_SOURCE_DIR}/zsh-${ZSH_VERSION}")
    if(ZSH_SOURCE STREQUAL "AUTO")
        if(EXISTS "${ZSH_LINK}/Src/zsh.mdh")
            get_filename_component(ZSH_SRC_DIR "${ZSH_LINK}" REALPATH)
            message(STATUS "Using existing zsh source tree: ${ZSH_SRC_DIR}")
        else()
            message(STATUS "Fetching zsh ${ZSH_VERSION}...")
            FetchContent_Declare(zsh-src
                URL "https://sourceforge.net/projects/zsh/files/zsh/${ZSH_VERSION}/zsh-${ZSH_VERSION}.tar.xz/download"
                URL_HASH SHA256=36fa734374b44783582cec09bcd67822e2f992c779ec1624ab5596df078d2f81
            )
            FetchContent_MakeAvailable(zsh-src)
            set(ZSH_SRC_DIR "${zsh-src_SOURCE_DIR}")

            set(ZSH_CONFIGURE_STAMP "${zsh-src_BINARY_DIR}/.configured")
            if(NOT EXISTS "${ZSH_CONFIGURE_STAMP}")
                message(STATUS "Configuring zsh (generating headers)...")
                execute_process(
                    COMMAND ./configure --disable-gdbm --disable-pcre
                    WORKING_DIRECTORY "${ZSH_SRC_DIR}"
                    RESULT_VARIABLE _zsh_conf_rc
                    OUTPUT_QUIET
                )
                if(NOT _zsh_conf_rc EQUAL 0)
                    message(FATAL_ERROR "zsh configure failed (run manually in ${ZSH_SRC_DIR})")
                endif()
                execute_process(
                    COMMAND ${CMAKE_COMMAND} -E env "top_srcdir=${ZSH_SRC_DIR}"
                        bash "${ZSH_SRC_DIR}/Src/mkmakemod.sh" Src Makemod
                    WORKING_DIRECTORY "${ZSH_SRC_DIR}"
                    RESULT_VARIABLE _zsh_mkm_rc
                    OUTPUT_QUIET
                )
                if(NOT _zsh_mkm_rc EQUAL 0)
                    message(FATAL_ERROR "zsh mkmakemod.sh failed")
                endif()
                execute_process(
                    COMMAND make -f Makemod zsh.mdh
                    WORKING_DIRECTORY "${ZSH_SRC_DIR}/Src"
                    RESULT_VARIABLE _zsh_mdh_rc
                    OUTPUT_QUIET
                )
                if(NOT _zsh_mdh_rc EQUAL 0)
                    message(WARNING "make zsh.mdh had errors (headers may be incomplete)")
                endif()
                file(TOUCH "${ZSH_CONFIGURE_STAMP}")
                message(STATUS "zsh headers generated in ${ZSH_SRC_DIR}/Src")
            endif()
        endif()
    else()
        get_filename_component(ZSH_SRC_DIR "${ZSH_SOURCE}" ABSOLUTE)
    endif()

    if(NOT EXISTS "${ZSH_SRC_DIR}/Src/zsh.mdh")
        message(WARNING "zsh.mdh not found at ${ZSH_SRC_DIR}/Src/ — "
                        "set ZSH_SOURCE to a configured zsh source tree or use AUTO")
        set(BUILD_ZSH_MODULE OFF)
    else()
        set(ZSH_SRC "${ZSH_SRC_DIR}/Src")
        message(STATUS "zsh headers: ${ZSH_SRC}")
    endif()
endif()

# ── Rust FFI crate ─────────────────────────────────────────────────────────
set(RUST_SRC_DIR "${CMAKE_SOURCE_DIR}/rust_src")
set(CARGO_RELEASE_FLAG "$<IF:$<CONFIG:Release>,--release,>")
set(CARGO_PROFILE_DIR "$<IF:$<CONFIG:Release>,release,debug>")
set(CARGO_HOME_DIR "${CMAKE_BINARY_DIR}/cargo-home")
file(MAKE_DIRECTORY "${CARGO_HOME_DIR}")

set(FFI_NAME "${CMAKE_SHARED_LIBRARY_PREFIX}zoxide_ffi${CMAKE_SHARED_LIBRARY_SUFFIX}")
set(FFI_SHARED "${RUST_SRC_DIR}/target/${CARGO_PROFILE_DIR}/${FFI_NAME}")

# Build zoxide-ffi via a real OUTPUT rule so Ninja only re-runs cargo when the
# library is missing or one of the inputs changed.
file(GLOB_RECURSE ZOXIDE_FFI_RUST_SOURCES CONFIGURE_DEPENDS
    "${RUST_SRC_DIR}/src/*.rs"
)
file(GLOB_RECURSE ZOXIDE_UPSTREAM_SOURCES CONFIGURE_DEPENDS
    "${ZOXIDE_SRC_DIR}/src/config.rs"
    "${ZOXIDE_SRC_DIR}/src/db/*.rs"
    "${ZOXIDE_SRC_DIR}/src/error.rs"
    "${ZOXIDE_SRC_DIR}/src/util.rs"
)

set(ZOXIDE_FFI_DEPENDS
    ${ZOXIDE_FFI_RUST_SOURCES}
    ${ZOXIDE_UPSTREAM_SOURCES}
    "${RUST_SRC_DIR}/Cargo.toml"
    "${RUST_SRC_DIR}/Cargo.lock"
)

# On macOS, rustc's default LC_ID_DYLIB is the ABSOLUTE cargo target path,
# which gets baked into any consumer (zoxide_native.so) as its LC_LOAD_DYLIB,
# leaking the build machine's path into the artifact. Rewrite it to a
# relocatable @rpath reference right after `cargo build` so the zsh module
# resolves the ffi library through its own LC_RPATH (set to @loader_path at
# install time) instead of an absolute path.
if(APPLE)
    set(ZOXIDE_FFI_FIX
        COMMAND install_name_tool -id
            "@rpath/${FFI_NAME}" "${FFI_SHARED}")
else()
    set(ZOXIDE_FFI_FIX "")
endif()

add_custom_command(
    OUTPUT "${FFI_SHARED}"
    COMMAND ${CMAKE_COMMAND} -E env "CARGO_HOME=${CARGO_HOME_DIR}"
        ${CARGO} build ${CARGO_RELEASE_FLAG}
    ${ZOXIDE_FFI_FIX}
    WORKING_DIRECTORY "${RUST_SRC_DIR}"
    DEPENDS ${ZOXIDE_FFI_DEPENDS}
    COMMENT "Building zoxide-ffi (Rust crate, $<CONFIG>)..."
    USES_TERMINAL
)

add_custom_target(zoxide_ffi ALL DEPENDS "${FFI_SHARED}")

# ── zsh module ─────────────────────────────────────────────────────────────
if(BUILD_ZSH_MODULE)
    set(ZSH_MODULE_NAME "zoxide_native")

    add_library(${ZSH_MODULE_NAME} SHARED
        "${CMAKE_SOURCE_DIR}/zsh_src/module.c"
        "${CMAKE_SOURCE_DIR}/zsh_src/argparse.c"
    )

    target_include_directories(${ZSH_MODULE_NAME} PRIVATE
        "${CMAKE_SOURCE_DIR}/zsh_src"
        "${ZSH_SRC}"
        "${ZSH_SRC}/Zle"
        "${ZSH_SRC}/Modules"
        "${ZSH_SRC_DIR}"  # for config.h
    )

    target_compile_definitions(${ZSH_MODULE_NAME} PRIVATE
        IMPORTING_MODULE_zshQsmain=1
    )

    target_compile_options(${ZSH_MODULE_NAME} PRIVATE
        -fPIC -O3 -flto -Wall -Wextra
    )

    add_dependencies(${ZSH_MODULE_NAME} zoxide_ffi)
    target_link_libraries(${ZSH_MODULE_NAME} PRIVATE
        "${FFI_SHARED}"
        pthread m
    )
    if(NOT APPLE)
        target_link_libraries(${ZSH_MODULE_NAME} PRIVATE dl)
    endif()

    if(APPLE)
        target_link_options(${ZSH_MODULE_NAME} PRIVATE
            -Wl,-undefined,dynamic_lookup
        )
    else()
        target_link_options(${ZSH_MODULE_NAME} PRIVATE
            -Wl,--allow-shlib-undefined -Wl,-z,lazy
        )
    endif()

    # zsh hardcodes its loadable-module suffix to ".so" on EVERY platform
    # (DL_EXT=so in zsh's configure.ac), including macOS. Even though this
    # target is a Mach-O dylib on macOS, it MUST be named zoxide_native.so
    # or `zmodload` will refuse to find it.
    set_target_properties(${ZSH_MODULE_NAME} PROPERTIES
        LIBRARY_OUTPUT_DIRECTORY "${CMAKE_SOURCE_DIR}/zsh_src/build"
        PREFIX ""
        SUFFIX ".so"
        BUILD_RPATH "${RUST_SRC_DIR}/target/$<IF:$<CONFIG:Release>,release,debug>"
    )
    if(APPLE)
        # macOS has no $ORIGIN. @loader_path makes the installed module resolve
        # libzoxide_ffi.dylib in its own directory (the ffi dylib is installed
        # alongside it under lib/zsh/zoxide-native).
        set_target_properties(${ZSH_MODULE_NAME} PROPERTIES
            INSTALL_RPATH "@loader_path"
        )
    else()
        set_target_properties(${ZSH_MODULE_NAME} PROPERTIES
            INSTALL_RPATH "$ORIGIN"
        )
    endif()

    if(APPLE)
        add_custom_command(TARGET ${ZSH_MODULE_NAME} POST_BUILD
            COMMAND ${CMAKE_STRIP} -u "$<TARGET_FILE:${ZSH_MODULE_NAME}>"
            COMMAND_EXPAND_LISTS
        )
    else()
        add_custom_command(TARGET ${ZSH_MODULE_NAME} POST_BUILD
            COMMAND ${CMAKE_STRIP} "$<TARGET_FILE:${ZSH_MODULE_NAME}>"
            COMMAND_EXPAND_LISTS
        )
    endif()

    if(UNIX)
        install(TARGETS ${ZSH_MODULE_NAME}
            LIBRARY DESTINATION "lib/zsh/zoxide-native"
        )
        install(FILES "${FFI_SHARED}"
            DESTINATION "lib/zsh/zoxide-native"
        )
        install(FILES "${CMAKE_SOURCE_DIR}/zsh_src/zoxide-native.plugin.zsh"
            DESTINATION "lib/zsh/zoxide-native"
        )
    endif()
endif()

# ── PowerShell module ──────────────────────────────────────────────────────
if(BUILD_PWSH_MODULE)
    set(PWSH_PROJECT "${CMAKE_SOURCE_DIR}/pwsh_src/ZoxideNative/ZoxideNative.csproj")
    set(PWSH_OUT_DIR "${CMAKE_SOURCE_DIR}/pwsh_src/ZoxideNative/bin/$<CONFIG>/net8.0")

    set(DOTNET_HOME_DIR "${CMAKE_BINARY_DIR}/dotnet-home")
    file(MAKE_DIRECTORY "${DOTNET_HOME_DIR}")
    file(MAKE_DIRECTORY "${CMAKE_BINARY_DIR}/nuget-packages")

    add_custom_target(zoxide_pwsh ALL
        COMMAND ${CMAKE_COMMAND} -E env
            "DOTNET_CLI_HOME=${DOTNET_HOME_DIR}"
            "DOTNET_SKIP_FIRST_TIME_EXPERIENCE=1"
            "DOTNET_NOLOGO=1"
            "NUGET_PACKAGES=${CMAKE_BINARY_DIR}/nuget-packages"
            ${DOTNET} build -c "$<CONFIG>" "${PWSH_PROJECT}"
        COMMAND ${CMAKE_COMMAND} -E copy_if_different
            "${FFI_SHARED}"
            "${PWSH_OUT_DIR}/"
        COMMENT "Building ZoxideNative (C# + P/Invoke, $<CONFIG>)..."
        DEPENDS zoxide_ffi
        USES_TERMINAL
    )

    install(DIRECTORY "${PWSH_OUT_DIR}/"
        DESTINATION "share/pwsh/modules/zoxide-native"
        FILES_MATCHING
        PATTERN "*.dll"
        PATTERN "*.psd1"
        PATTERN "*.psm1"
        PATTERN "*.so"
        PATTERN "*.dylib"
    )
endif()

# ── Tests ──────────────────────────────────────────────────────────────────
if(BUILD_TESTS)
    # Unit test for the pure-C argument parser used by the zsh shim.
    add_executable(argparse_unit
        "${CMAKE_SOURCE_DIR}/tests/test_argparse.c"
        "${CMAKE_SOURCE_DIR}/zsh_src/argparse.c"
    )
    target_include_directories(argparse_unit PRIVATE "${CMAKE_SOURCE_DIR}/zsh_src")
    target_link_libraries(argparse_unit PRIVATE m)
    if(NOT WIN32)
        target_compile_options(argparse_unit PRIVATE -Wall -Wextra)
    endif()

    # System test: dlopen the built cdylib and exercise the exported ABI.
    add_executable(ffi_smoke "${CMAKE_SOURCE_DIR}/tests/ffi_smoke.c")
    target_include_directories(ffi_smoke PRIVATE "${CMAKE_SOURCE_DIR}/zsh_src")
    target_link_libraries(ffi_smoke PRIVATE ${CMAKE_DL_LIBS})
    add_dependencies(ffi_smoke zoxide_ffi)
    if(NOT WIN32)
        target_compile_definitions(ffi_smoke PRIVATE _DEFAULT_SOURCE)
        target_compile_options(ffi_smoke PRIVATE -Wall -Wextra)
    endif()

    enable_testing()
    add_test(NAME argparse_unit COMMAND argparse_unit)
    add_test(NAME ffi_smoke COMMAND ffi_smoke "${FFI_SHARED}")

    add_custom_target(test-rust
        COMMAND ${CMAKE_COMMAND} -E env "CARGO_HOME=${CARGO_HOME_DIR}"
            ${CARGO} test ${CARGO_RELEASE_FLAG}
        WORKING_DIRECTORY "${RUST_SRC_DIR}"
        USES_TERMINAL
    )

    if(BUILD_ZSH_MODULE AND TARGET ${ZSH_MODULE_NAME})
        add_custom_target(test-zsh
            COMMAND ${CMAKE_COMMAND} -E env
                "MODULE_DIR=${CMAKE_SOURCE_DIR}/zsh_src/build"
                zsh "${CMAKE_SOURCE_DIR}/tests/test_zoxide_zsh.sh"
            DEPENDS ${ZSH_MODULE_NAME}
            USES_TERMINAL
        )
        add_custom_target(test-zsh-unload
            COMMAND ${CMAKE_COMMAND} -E env
                "MODULE_DIR=${CMAKE_SOURCE_DIR}/zsh_src/build"
                zsh "${CMAKE_SOURCE_DIR}/tests/test_unload_zoxide_zsh.sh"
            DEPENDS ${ZSH_MODULE_NAME}
            USES_TERMINAL
        )
    endif()

    if(BUILD_PWSH_MODULE)
        add_custom_target(test-pwsh
            COMMAND ${CMAKE_COMMAND} -E env
                "DLL_DIR=${PWSH_OUT_DIR}"
                "DOTNET_CLI_HOME=${DOTNET_HOME_DIR}"
                "DOTNET_SKIP_FIRST_TIME_EXPERIENCE=1"
                pwsh -NoProfile -File "${CMAKE_SOURCE_DIR}/tests/test_pwsh.ps1"
                    -DllDir "${PWSH_OUT_DIR}"
            DEPENDS zoxide_pwsh
            USES_TERMINAL
        )
    endif()

    add_custom_target(check
        COMMAND ${CMAKE_CTEST_COMMAND} --output-on-failure -C "$<CONFIG>"
        DEPENDS ffi_smoke
    )
endif()

# ── Print configuration summary ────────────────────────────────────────────
message(STATUS "")
message(STATUS "zoxide-native ${PROJECT_VERSION} — Build Configuration")
message(STATUS "──────────────────────────────────────────────────")
message(STATUS "Generator:         ${CMAKE_GENERATOR}")
message(STATUS "zoxide source:     ${ZOXIDE_SRC_DIR}")
message(STATUS "zsh source:        ${ZSH_SRC_DIR}")
message(STATUS "zsh module:        ${BUILD_ZSH_MODULE}")
message(STATUS "PowerShell module: ${BUILD_PWSH_MODULE}")
message(STATUS "Tests:             ${BUILD_TESTS}")
message(STATUS "Cargo:             ${CARGO}")
message(STATUS "dotnet:            ${DOTNET}")
message(STATUS "compile_commands:  ${CMAKE_EXPORT_COMPILE_COMMANDS}")
message(STATUS "")
message(STATUS "Build with: cmake --build <dir> --config <Release|Debug>")
message(STATUS "")
