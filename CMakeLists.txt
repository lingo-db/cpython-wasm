cmake_minimum_required(VERSION 3.16)

find_program(CARGO_EXECUTABLE cargo)
if(NOT CARGO_EXECUTABLE)
    message(FATAL_ERROR "cargo not found. Please install Rust and Cargo to proceed.")
endif()

# Use built/install wasi-sdk subdirectory
set(WASI_SDK_INSTALL_DIR "${CMAKE_CURRENT_BINARY_DIR}/wasi-sdk/build/install")
set(WASI_SYSROOT "${WASI_SDK_INSTALL_DIR}/share/wasi-sysroot")

## Build WASI SDK toolchain
# we currently need to build our own wasi-sdk since we need the newest LLVM version with a patched wasm-ld, since the
# current latest release has a bug that makes it incompatible with the --wrap linker flag used when building CPython
# see: https://github.com/llvm/llvm-project/pull/169656
add_custom_command(
    OUTPUT "${WASI_SDK_INSTALL_DIR}/bin/clang"
    COMMAND cmake -G Ninja -B build/toolchain -S ${CMAKE_CURRENT_LIST_DIR}/wasi-sdk -DWASI_SDK_BUILD_TOOLCHAIN=ON -DCMAKE_INSTALL_PREFIX=build/install
    COMMAND cmake --build build/toolchain --target install
    WORKING_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}/wasi-sdk"
    COMMENT "Building WASI SDK toolchain"
)
add_custom_target(WasiSDKToolchain DEPENDS "${WASI_SDK_INSTALL_DIR}/bin/clang")

## Build WASI SDK sysroot
add_custom_command(
    OUTPUT "${WASI_SDK_INSTALL_DIR}/share/wasi-sysroot"
    COMMAND cmake -G Ninja -B build/sysroot -S ${CMAKE_CURRENT_LIST_DIR}/wasi-sdk -DCMAKE_INSTALL_PREFIX=build/install -DCMAKE_TOOLCHAIN_FILE=${CMAKE_CURRENT_BINARY_DIR}/wasi-sdk/build/install/share/cmake/wasi-sdk.cmake -DCMAKE_C_COMPILER_WORKS=ON -DCMAKE_CXX_COMPILER_WORKS=ON
    COMMAND cmake --build build/sysroot --target install
    WORKING_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}/wasi-sdk"
    DEPENDS WasiSDKToolchain
    COMMENT "Building WASI SDK sysroot"
)
# wget https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-29/libclang_rt-29.0.tar.gz
# mkdir -p ${WASI_SDK_INSTALL_DIR}/lib/clang/21/lib/
# tar -xvf libclang_rt-29.0.tar.gz --strip-components=1 -C ${WASI_SDK_INSTALL_DIR}/lib/clang/21/lib/
add_custom_command(
    OUTPUT "${WASI_SDK_INSTALL_DIR}/lib/clang/21/lib/wasm32-unknown-wasi/libclang_rt.builtins.a"
    COMMAND ${CMAKE_COMMAND} -E make_directory "${WASI_SDK_INSTALL_DIR}/lib/clang/21/lib/"
    COMMAND bash -c 'wget https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-29/libclang_rt-29.0.tar.gz'
    COMMAND bash -c 'tar -xvf libclang_rt-29.0.tar.gz --strip-components=1 -C "${WASI_SDK_INSTALL_DIR}/lib/clang/21/lib/"'
    COMMAND bash -c 'rm libclang_rt-29.0.tar.gz'
    COMMENT "Copying libclang_rt.builtins-wasm32.a to WASI SDK"
)

add_custom_target(WasiSDKSysroot DEPENDS "${WASI_SDK_INSTALL_DIR}/share/wasi-sysroot" "${WASI_SDK_INSTALL_DIR}/lib/clang/21/lib/wasm32-unknown-wasi/libclang_rt.builtins.a")

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
    DEPENDS CPythonNative WasiSDKToolchain WasiSDKSysroot
    COMMENT "Building WASM CPython"
)
add_custom_target(CPythonWasm DEPENDS "${CPYTHON_WASM_BUILD_DIR}/python.wasm")

## Setup WAMR
# https://github.com/bytecodealliance/wasm-micro-runtime/blob/main/doc/build_wamr.md#wamr-vmcore-cmake-building-configurations
if (APPLE)
    set (WAMR_BUILD_PLATFORM "darwin")
    set (WAMR_BUILD_TARGET "AARCH64")
else ()
    set (WAMR_BUILD_PLATFORM "linux")
    set (WAMR_BUILD_TARGET "X86_64")
endif ()
set (WAMR_BUILD_INTERP 1)
set (WAMR_BUILD_FAST_INTERP 0)
set (WAMR_BUILD_AOT 1)
set (WAMR_BUILD_LIBC_BUILTIN 1)
set (WAMR_BUILD_LIBC_WASI 1)
set (WAMR_BUILD_SIMD 1)
set (WAMR_BUILD_REF_TYPES 1)
set (WAMR_BUILD_WITH_CUSTOM_LLVM 1)
set (WAMR_BUILD_DUMP_CALL_STACK 1)
set (WAMR_BUILD_CUSTOM_NAME_SECTION 1)
set (WAMR_ROOT_DIR ${CMAKE_CURRENT_LIST_DIR}/wasm-micro-runtime)
include (${WAMR_ROOT_DIR}/build-scripts/runtime_lib.cmake)
add_library(vmlib ${WAMR_RUNTIME_LIB_SOURCE} ${UNCOMMON_SHARED_SOURCE})
include (${SHARED_DIR}/utils/uncommon/shared_uncommon.cmake)

## Compile python.wasm to python.aot using wamrc
# Define the build directory for wamr-compiler
set(WAMR_COMPILER_DIR "${WAMR_ROOT_DIR}/wamr-compiler")
set(WAMR_COMPILER_BUILD_DIR "${CMAKE_BINARY_DIR}/wamr-compiler")
set(WAMR_COMPILER_EXECUTABLE "${WAMR_COMPILER_BUILD_DIR}/wamrc")

# Add custom command to build wamr-compiler
add_custom_command(
        OUTPUT "${WAMR_COMPILER_EXECUTABLE}"
        COMMAND ${CMAKE_COMMAND}
                    -S "${WAMR_COMPILER_DIR}"
                    -B "${WAMR_COMPILER_BUILD_DIR}"
                    -D WAMR_BUILD_INTERP=${WAMR_BUILD_INTERP}
                    -D WAMR_BUILD_FAST_INTERP=${WAMR_BUILD_FAST_INTERP}
                    -D WAMR_BUILD_AOT=${WAMR_BUILD_AOT}
                    -D WAMR_BUILD_LIBC_BUILTIN=${WAMR_BUILD_LIBC_BUILTIN}
                    -D WAMR_BUILD_LIBC_WASI=${WAMR_BUILD_LIBC_WASI}
                    -D WAMR_BUILD_SIMD=${WAMR_BUILD_SIMD}
                    -D WAMR_BUILD_REF_TYPES=${WAMR_BUILD_REF_TYPES}
                    -D WAMR_BUILD_WITH_CUSTOM_LLVM=${WAMR_BUILD_WITH_CUSTOM_LLVM}
                    -D WAMR_BUILD_PLATFORM=${WAMR_BUILD_PLATFORM}
                    -D WAMR_BUILD_TARGET=${WAMR_BUILD_TARGET}
                    -D WAMR_BUILD_DUMP_CALL_STACK=${WAMR_BUILD_DUMP_CALL_STACK}
                    -D WAMR_BUILD_CUSTOM_NAME_SECTION=${WAMR_BUILD_CUSTOM_NAME_SECTION}
                    -D CMAKE_BUILD_TYPE=Release
        COMMAND make -C "${WAMR_COMPILER_BUILD_DIR}" -j4
        COMMENT "Building wamr-compiler using cmake and make"
)
## Add custom command to generate python.aot from python.wasm
#add_custom_command(
#        OUTPUT "${CPYTHON_WASM_BUILD_DIR}/python.aot"
#        COMMAND ${WAMR_COMPILER_EXECUTABLE} --enable-dump-call-stack --size-level=0 -o "${CPYTHON_WASM_BUILD_DIR}/python.aot" "${CPYTHON_WASM_BUILD_DIR}/python.wasm"
#        DEPENDS CPythonWasm "${WAMR_COMPILER_EXECUTABLE}"
#        COMMENT "Generating python.aot from python.wasm using wamrc"
#)
## Add output as dependency for library
#add_custom_target(
#        GeneratePythonAOT ALL
#        DEPENDS "${CPYTHON_WASM_BUILD_DIR}/python.aot"
#)

add_library(cpython-wasm INTERFACE ${UNCOMMON_SHARED_SOURCE})
set(UNCOMMON_SHARED_SOURCE ${UNCOMMON_SHARED_SOURCE} PARENT_SCOPE)

target_include_directories(cpython-wasm INTERFACE
    ${UNCOMMON_SHARED_DIR}
    ${SHARED_DIR}/utils/
    ${SHARED_DIR}/platform/linux/
    ${IWASM_DIR}/include/
    ${IWASM_DIR}/common/
)
target_compile_definitions(cpython-wasm INTERFACE
    CPYTHON_WASM_SOURCE_DIR="${CPYTHON_WASM_SOURCE_DIR}"
    CPYTHON_WASM_BUILD_DIR="${CPYTHON_WASM_BUILD_DIR}"
    CPYTHON_WASM_AOT_FILE="${CPYTHON_WASM_BUILD_DIR}/cpython-merged.aot"
)
target_link_libraries(cpython-wasm INTERFACE vmlib)
#add_dependencies(cpython-wasm GeneratePythonAOT)
