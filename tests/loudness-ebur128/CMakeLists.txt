# camera-box#1436: the vendored libebur128 that obs-loudness-dock meters with, checked against the
# EBU Tech 3341 reference signal.
cmake_minimum_required(VERSION 3.16)
project(loudness-ebur128-test C)

set(EBUR128 ${CMAKE_CURRENT_SOURCE_DIR}/../../vendor/obs-loudness-dock/deps/libebur128/ebur128)
add_executable(ebur128-reference ebur128_reference.c ${EBUR128}/ebur128.c)
target_include_directories(ebur128-reference PRIVATE ${EBUR128} ${EBUR128}/queue)
# -Werror on the test only: the vendored ebur128.c is upstream code, warnings included.
set_source_files_properties(ebur128_reference.c PROPERTIES COMPILE_OPTIONS "-Wall;-Wextra;-Werror")
target_link_libraries(ebur128-reference PRIVATE m)
