cmake_minimum_required(VERSION 3.16)

# Use built/install wasi-sdk subdirectory
set(WASI_SDK_INSTALL_DIR "${CMAKE_CURRENT_BINARY_DIR}/wasi-sdk/")
set(WASI_SYSROOT "${WASI_SDK_INSTALL_DIR}/share/wasi-sysroot")

## Add WASI SDK toolchain
add_custom_command(
    OUTPUT "${WASI_SDK_INSTALL_DIR}/bin/clang" "${WASI_SDK_INSTALL_DIR}/share/wasi-sysroot" "${WASI_SDK_INSTALL_DIR}/lib/clang/21/lib/wasm32-unknown-wasi/libclang_rt.builtins.a"
    COMMAND bash -c 'wget https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-29/wasi-sdk-29.0-x86_64-linux.tar.gz'
    COMMAND bash -c 'tar -xvf wasi-sdk-29.0-x86_64-linux.tar.gz --strip-components=1 -C "${WASI_SDK_INSTALL_DIR}/"'
    COMMAND bash -c 'rm wasi-sdk-29.0-x86_64-linux.tar.gz'
    COMMENT "Downloading WASI SDK toolchain"
)
add_custom_target(WasiSDKToolchain DEPENDS "${WASI_SDK_INSTALL_DIR}/bin/clang" "${WASI_SDK_INSTALL_DIR}/share/wasi-sysroot" "${WASI_SDK_INSTALL_DIR}/lib/clang/21/lib/wasm32-unknown-wasi/libclang_rt.builtins.a")

## Setup CPython WASM
set(CPYTHON_WASM_SOURCE_DIR "${CMAKE_CURRENT_LIST_DIR}/cpython" CACHE PATH "Path to the Python source directory")
set(CPYTHON_WASM_BUILD_DIR "${CMAKE_CURRENT_BINARY_DIR}/cpython/wasm32-wasip1" CACHE PATH "Path to the Python build directory")
set(CPYTHON_NATIVE_BUILD_DIR "${CMAKE_CURRENT_BINARY_DIR}/cpython/native")

message(STATUS "Using CPython source from ${CPYTHON_WASM_SOURCE_DIR}")
message(STATUS "Using CPython build from ${CPYTHON_WASM_BUILD_DIR}")

# Build native Python (bootstrap)
add_custom_command(
    OUTPUT "${CPYTHON_NATIVE_BUILD_DIR}/python"
    COMMAND ${CMAKE_COMMAND} -E make_directory "${CPYTHON_NATIVE_BUILD_DIR}"
    WORKING_DIRECTORY "${CPYTHON_NATIVE_BUILD_DIR}"
    COMMAND bash -c "SOURCE_DATE_EPOCH=1760114616 ${CPYTHON_WASM_SOURCE_DIR}/configure"
    COMMAND make -j$$(nproc) all
    DEPENDS "${CPYTHON_WASM_SOURCE_DIR}/configure"
    COMMENT "Building native CPython (bootstrap)"
)
add_custom_target(CPythonNative DEPENDS "${CPYTHON_NATIVE_BUILD_DIR}/python")

# Build WASM Python
add_custom_command(
    OUTPUT "${CPYTHON_WASM_BUILD_DIR}/python.wasm"
    COMMAND ${CMAKE_COMMAND} -E make_directory "${CPYTHON_WASM_BUILD_DIR}"
    WORKING_DIRECTORY ${CPYTHON_WASM_BUILD_DIR}
    COMMAND ${CMAKE_COMMAND} -E env
        CFLAGS='--sysroot=${WASI_SYSROOT}'
        LDFLAGS='--sysroot=${WASI_SYSROOT} -Wl,--export-all'
        AR=${WASI_SDK_INSTALL_DIR}/bin/llvm-ar
        CC='${WASI_SDK_INSTALL_DIR}/bin/clang'
        CONFIG_SITE=${CPYTHON_WASM_SOURCE_DIR}/Tools/wasm/wasi/config.site-wasm32-wasi
        CPP='${WASI_SDK_INSTALL_DIR}/bin/clang-cpp'
        CXX='${WASI_SDK_INSTALL_DIR}/bin/clang++'
        HOSTRUNNER='wasmtime run --wasm max-wasm-stack=16777216 --dir ${CPYTHON_WASM_SOURCE_DIR}::/ --env PYTHONPATH=/cross-build/wasm32-wasip1/build/lib.wasi-wasm32-3.14'
        PATH=${WASI_SDK_INSTALL_DIR}/bin:$$PATH
        PKG_CONFIG_LIBDIR=${WASI_SDK_INSTALL_DIR}/share/wasi-sysroot/lib/pkgconfig:${WASI_SDK_INSTALL_DIR}/share/wasi-sysroot/share/pkgconfig
        PKG_CONFIG_SYSROOT_DIR=${WASI_SYSROOT}
        PKG_CONFIG_PATH=
        RANLIB=${WASI_SDK_INSTALL_DIR}/bin/ranlib
        SOURCE_DATE_EPOCH=1760114616
        WASI_SDK_PATH=${WASI_SDK_INSTALL_DIR}
        WASI_SYSROOT=${WASI_SYSROOT}
        ${CPYTHON_WASM_SOURCE_DIR}/configure --host=wasm32-wasip1 --build=x86_64-pc-linux-gnu --with-build-python=${CPYTHON_NATIVE_BUILD_DIR}/python
    COMMAND make -j$$(nproc) all
    DEPENDS CPythonNative WasiSDKToolchain
    COMMENT "Building WASM CPython"
)
add_custom_target(CPythonWasm DEPENDS "${CPYTHON_WASM_BUILD_DIR}/python.wasm")

## Wasmer C API SDK
# Bundled at configure time so the build is hermetic — we get a known runtime
# version + matching CLI for AOT precompilation. Pin v7.1.0 explicitly.
set(WASMER_VERSION "7.1.0")
set(WASMER_INSTALL_DIR "${CMAKE_CURRENT_BINARY_DIR}/wasmer-sdk")
if (APPLE)
    set(WASMER_PLATFORM "darwin-arm64")
else ()
    set(WASMER_PLATFORM "linux-amd64")
endif ()
set(WASMER_TARBALL "wasmer-${WASMER_PLATFORM}.tar.gz")
set(WASMER_DOWNLOAD_URL "https://github.com/wasmerio/wasmer/releases/download/v${WASMER_VERSION}/${WASMER_TARBALL}")

add_custom_command(
    OUTPUT "${WASMER_INSTALL_DIR}/lib/libwasmer.so" "${WASMER_INSTALL_DIR}/include/wasmer.h" "${WASMER_INSTALL_DIR}/bin/wasmer"
    COMMAND ${CMAKE_COMMAND} -E make_directory "${WASMER_INSTALL_DIR}"
    COMMAND bash -c "wget -q '${WASMER_DOWNLOAD_URL}' -O '${WASMER_INSTALL_DIR}/${WASMER_TARBALL}'"
    COMMAND bash -c "tar -xzf '${WASMER_INSTALL_DIR}/${WASMER_TARBALL}' -C '${WASMER_INSTALL_DIR}'"
    COMMAND bash -c "rm '${WASMER_INSTALL_DIR}/${WASMER_TARBALL}'"
    COMMENT "Downloading Wasmer C API SDK v${WASMER_VERSION} (${WASMER_PLATFORM})"
)
add_custom_target(WasmerSDK DEPENDS "${WASMER_INSTALL_DIR}/lib/libwasmer.so" "${WASMER_INSTALL_DIR}/include/wasmer.h" "${WASMER_INSTALL_DIR}/bin/wasmer")

## Pre-compile python.wasm to a Wasmer-serialized artifact (replaces wamrc -> .aot).
# Trusts wasmer's default target/CPU detection — unlike wamrc which we had to
# manually pin to skylake to avoid AVX-512 SIGILL on Zen-mismatched runners.
add_custom_command(
    OUTPUT "${CPYTHON_WASM_BUILD_DIR}/python.wasmu"
    COMMAND "${WASMER_INSTALL_DIR}/bin/wasmer" compile
                -o "${CPYTHON_WASM_BUILD_DIR}/python.wasmu"
                "${CPYTHON_WASM_BUILD_DIR}/python.wasm"
    DEPENDS CPythonWasm WasmerSDK
    COMMENT "Pre-compiling python.wasm -> python.wasmu via wasmer"
)
add_custom_target(GeneratePythonWasmu ALL DEPENDS "${CPYTHON_WASM_BUILD_DIR}/python.wasmu")

# INTERFACE target consumed by src/runtime — exposes the SDK + the artifact path.
add_library(cpython-wasm INTERFACE)
target_include_directories(cpython-wasm INTERFACE "${WASMER_INSTALL_DIR}/include")
target_link_libraries(cpython-wasm INTERFACE "${WASMER_INSTALL_DIR}/lib/libwasmer.so")
target_compile_definitions(cpython-wasm INTERFACE
    CPYTHON_WASM_SOURCE_DIR="${CPYTHON_WASM_SOURCE_DIR}"
    CPYTHON_WASM_BUILD_DIR="${CPYTHON_WASM_BUILD_DIR}"
    CPYTHON_WASM_FILE="${CPYTHON_WASM_BUILD_DIR}/python.wasmu"
)
add_dependencies(cpython-wasm GeneratePythonWasmu)
