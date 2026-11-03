cmake_minimum_required(VERSION 3.16)

# obs#1 / camera-box#1485: the OBS `dante_input` source. The Dante device is dante-input-ffi
# (ffi/, Rust over dstack's dlink), built first and handed in as a static library.
project(dante-input VERSION 0.2.0 LANGUAGES C)

find_package(libobs REQUIRED)

set(DANTE_INPUT_FFI_LIB "" CACHE FILEPATH "ffi/target/release/libdante_input_ffi.a")
set(DANTE_INPUT_FFI_NATIVE_LIBS "" CACHE STRING "the native-static-libs rustc printed for it")
if(NOT EXISTS "${DANTE_INPUT_FFI_LIB}")
	message(FATAL_ERROR "DANTE_INPUT_FFI_LIB '${DANTE_INPUT_FFI_LIB}' does not exist; build ffi/ first")
endif()
if(DANTE_INPUT_FFI_NATIVE_LIBS STREQUAL "")
	message(FATAL_ERROR "DANTE_INPUT_FFI_NATIVE_LIBS is empty; pass rustc's native-static-libs")
endif()
separate_arguments(DANTE_INPUT_NATIVE UNIX_COMMAND "${DANTE_INPUT_FFI_NATIVE_LIBS}")

add_library(dante-input MODULE src/plugin.c)
set_target_properties(dante-input PROPERTIES PREFIX "")
target_include_directories(dante-input PRIVATE include)
target_link_libraries(dante-input PRIVATE OBS::libobs "${DANTE_INPUT_FFI_LIB}" ${DANTE_INPUT_NATIVE})
target_compile_options(dante-input PRIVATE -Wall -Wextra -Werror)
# -z defs: an unresolved symbol fails the link here, not at OBS load. --exclude-libs: the Rust
# archive's symbols stay private to this module, so they never meet another plugin's Rust copy
# (DistroAV carries the JPEG XS one).
target_link_options(dante-input PRIVATE -Wl,-z,defs -Wl,--exclude-libs,ALL -Wl,-z,noexecstack)

# obs#1: the module test (test/module_test.c), run by CI against the built module.
add_executable(dante-input-module-test test/module_test.c)
target_link_libraries(dante-input-module-test PRIVATE OBS::libobs)
target_compile_options(dante-input-module-test PRIVATE -Wall -Wextra -Werror)
