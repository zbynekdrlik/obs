cmake_minimum_required(VERSION 3.16)

# obs#94 (camera-box#1527): libobs counts the audio ticks whose render found a source's buffer short
# of the tick (it sat out that tick's mix). Builds against the OBS SDK this repo's CI installs.
project(audio-short-ticks-test LANGUAGES C)

find_package(libobs REQUIRED)

add_executable(audio-short-ticks-test audio_short_ticks_test.c)
target_link_libraries(audio-short-ticks-test PRIVATE OBS::libobs)
target_compile_options(audio-short-ticks-test PRIVATE -Wall -Wextra -Werror)
