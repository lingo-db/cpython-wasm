# Helper: download URL to DEST, verify SHA256, atomic rename.
# Used by the parent CMakeLists for python.webc (~45 MB) and similar.

if(NOT URL OR NOT DEST OR NOT SHA256)
    message(FATAL_ERROR "download_pinned.cmake needs URL, DEST, SHA256")
endif()

if(EXISTS "${DEST}")
    file(SHA256 "${DEST}" existing)
    if(existing STREQUAL SHA256)
        return()
    endif()
    message(STATUS "[download_pinned] checksum mismatch, redownloading ${DEST}")
    file(REMOVE "${DEST}")
endif()

file(DOWNLOAD "${URL}" "${DEST}.partial"
     EXPECTED_HASH SHA256=${SHA256}
     SHOW_PROGRESS
     STATUS dl_status)
list(GET dl_status 0 dl_code)
if(NOT dl_code EQUAL 0)
    list(GET dl_status 1 dl_msg)
    file(REMOVE "${DEST}.partial")
    message(FATAL_ERROR "download ${URL} failed: ${dl_msg}")
endif()
file(RENAME "${DEST}.partial" "${DEST}")
