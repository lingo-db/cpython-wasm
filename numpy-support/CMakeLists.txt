add_custom_target(create_numpy_build_symlinks
        COMMAND ${CMAKE_COMMAND} -P "${CMAKE_CURRENT_LIST_DIR}/cross-compile-links/create_numpy_build_symlinks.cmake"
        COMMENT "Create pywasmcross wrapper symlinks in ${CMAKE_CURRENT_LIST_DIR}"
        VERBATIM
)
