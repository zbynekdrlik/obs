# camera-box#1387: the JPEG XS program output's start retry (vendor/distroav/src/jxs-output-decide.h),
# the pure decisions the output's supervisor thread runs.
cmake_minimum_required(VERSION 3.16)
project(jxs-output-retry-test C)

add_executable(jxs-output-retry jxs_output_retry.c)
target_include_directories(jxs-output-retry PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/../../vendor/distroav/src)
target_compile_options(jxs-output-retry PRIVATE -Wall -Wextra -Werror)
