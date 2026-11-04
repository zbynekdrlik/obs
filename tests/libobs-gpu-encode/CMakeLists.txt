cmake_minimum_required(VERSION 3.16)

# camera-box#1387 (obs#1): libobs texture encoding across recordings, built against the OBS SDK
# this repo's CI installs and run under xvfb (libobs-opengl needs an X display).
project(libobs-gpu-encode-test LANGUAGES C)

find_package(libobs REQUIRED)

add_executable(gpu-encode-test gpu_encode_test.c)
target_link_libraries(gpu-encode-test PRIVATE OBS::libobs)
target_compile_options(gpu-encode-test PRIVATE -Wall -Wextra -Werror)
