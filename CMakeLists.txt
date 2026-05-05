cmake_minimum_required(VERSION 3.16)

# Phase-2 build: drive a persistent WASIX-CPython interpreter via the Rust
# shim crate `wasix-python-bridge`. The shim links wasmer + wasmer-wasix
# directly, so the host runtime no longer needs the standalone Wasmer C SDK,
# wasi-sdk, wasixcc, or any local CPython cross-compile.

# ── Inputs we still pull from the network at configure/build time ─────────
# 1. The python.webc package — the wasix-org cp313 build, served from the
#    Wasmer registry CDN. Pinned by SHA so builds are reproducible and
#    offline-capable after first fetch.
# 2. The numpy wasix wheel — for the optional numpy UDF tests. Same idea.
# 3. The Rust shim itself, which `cargo build --release` produces.

# ── Output paths consumed by the host runtime ─────────────────────────────
set(LINGODB_WASIX_BRIDGE_DIR "${CMAKE_CURRENT_LIST_DIR}/wasix-python-bridge"
    CACHE PATH "Source dir of the Rust shim crate")
set(LINGODB_WASIX_BUILD_DIR "${CMAKE_CURRENT_BINARY_DIR}")
set(LINGODB_WASIX_WEBC_FILE "${LINGODB_WASIX_BUILD_DIR}/python.webc")
set(LINGODB_WASIX_SITE_PACKAGES_DIR "${LINGODB_WASIX_BUILD_DIR}/site-packages"
    CACHE PATH "Host dir mounted at /site-packages inside the wasix sandbox")

# Pinned by sha256: this is the python/python@3.13.5 build from the Wasmer
# registry. URL is content-addressed so a SHA bump = a real upgrade.
set(PYTHON_WEBC_URL
    "https://cdn.wasmer.io/webcimages/c03ebe0946e66edf598fd7a1f192101f60e4e9c0095aecd04e049989692bdcab.webc")
set(PYTHON_WEBC_SHA256
    "c03ebe0946e66edf598fd7a1f192101f60e4e9c0095aecd04e049989692bdcab")

set(NUMPY_WHEEL_URL
    "https://pythonindex.wasix.org/packages/numpy-2.3.2-cp313-cp313-wasix_wasm32.whl")
set(NUMPY_WHEEL_FILENAME "numpy-2.3.2-cp313-cp313-wasix_wasm32.whl")

message(STATUS "[wasix] Rust shim source : ${LINGODB_WASIX_BRIDGE_DIR}")
message(STATUS "[wasix] webc download    : ${LINGODB_WASIX_WEBC_FILE}")
message(STATUS "[wasix] site-packages    : ${LINGODB_WASIX_SITE_PACKAGES_DIR}")

# ── Download python.webc once, EXPECTED_HASH guards corruption ────────────
add_custom_command(
    OUTPUT "${LINGODB_WASIX_WEBC_FILE}"
    COMMAND ${CMAKE_COMMAND}
        -DURL=${PYTHON_WEBC_URL}
        -DDEST=${LINGODB_WASIX_WEBC_FILE}
        -DSHA256=${PYTHON_WEBC_SHA256}
        -P ${CMAKE_CURRENT_LIST_DIR}/cmake/download_pinned.cmake
    COMMENT "Fetching python.webc (pinned ${PYTHON_WEBC_SHA256})"
)
add_custom_target(LingodbWasixWebc DEPENDS "${LINGODB_WASIX_WEBC_FILE}")

# ── Download + unpack the numpy wheel (and any others later) ──────────────
# Wheels are zip files whose top-level layout *is* a site-packages tree, so
# multiple wheels can be unpacked into the same dir.
add_custom_command(
    OUTPUT "${LINGODB_WASIX_SITE_PACKAGES_DIR}/.lingodb-numpy-installed"
    COMMAND ${CMAKE_COMMAND} -E make_directory "${LINGODB_WASIX_SITE_PACKAGES_DIR}"
    COMMAND wget -q "${NUMPY_WHEEL_URL}" -O "${LINGODB_WASIX_BUILD_DIR}/${NUMPY_WHEEL_FILENAME}"
    COMMAND ${CMAKE_COMMAND} -E chdir "${LINGODB_WASIX_SITE_PACKAGES_DIR}"
        unzip -oq "${LINGODB_WASIX_BUILD_DIR}/${NUMPY_WHEEL_FILENAME}"
    COMMAND ${CMAKE_COMMAND} -E rm "${LINGODB_WASIX_BUILD_DIR}/${NUMPY_WHEEL_FILENAME}"
    COMMAND ${CMAKE_COMMAND} -E touch
        "${LINGODB_WASIX_SITE_PACKAGES_DIR}/.lingodb-numpy-installed"
    COMMENT "Fetching + unpacking numpy WASIX wheel"
)
add_custom_target(LingodbWasixWheels ALL
    DEPENDS "${LINGODB_WASIX_SITE_PACKAGES_DIR}/.lingodb-numpy-installed")

# ── Build the Rust shim ──────────────────────────────────────────────────
# Cargo manages its own incremental builds; we just declare the artifact and
# stamp it with the input lib.rs/Cargo.toml as DEPENDS so reconfiguration
# triggers a rebuild only when the crate itself changed.
set(LINGODB_WASIX_BRIDGE_LIB
    "${LINGODB_WASIX_BRIDGE_DIR}/target/release/libwasix_python_bridge.so")
set(LINGODB_WASIX_BRIDGE_HEADER
    "${LINGODB_WASIX_BRIDGE_DIR}/include/wasix_python_bridge.h")
add_custom_command(
    OUTPUT "${LINGODB_WASIX_BRIDGE_LIB}"
    WORKING_DIRECTORY "${LINGODB_WASIX_BRIDGE_DIR}"
    COMMAND ${CMAKE_COMMAND} -E env
        cargo build --release
    DEPENDS
        "${LINGODB_WASIX_BRIDGE_DIR}/Cargo.toml"
        "${LINGODB_WASIX_BRIDGE_DIR}/src/lib.rs"
    COMMENT "cargo build --release (wasix-python-bridge)"
)
add_custom_target(LingodbWasixBridge DEPENDS "${LINGODB_WASIX_BRIDGE_LIB}")

# ── INTERFACE target consumed by src/runtime ──────────────────────────────
add_library(cpython-wasm INTERFACE)
target_include_directories(cpython-wasm INTERFACE
    "${LINGODB_WASIX_BRIDGE_DIR}/include")
target_link_libraries(cpython-wasm INTERFACE "${LINGODB_WASIX_BRIDGE_LIB}")
target_compile_definitions(cpython-wasm INTERFACE
    LINGODB_WASIX_WEBC_FILE="${LINGODB_WASIX_WEBC_FILE}"
    LINGODB_WASIX_SITE_PACKAGES_DIR="${LINGODB_WASIX_SITE_PACKAGES_DIR}"
    LINGODB_WASIX_MODULE_CACHE_DIR="${LINGODB_WASIX_BUILD_DIR}/module-cache"
)
add_dependencies(cpython-wasm LingodbWasixBridge LingodbWasixWebc LingodbWasixWheels)
