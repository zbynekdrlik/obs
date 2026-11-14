cmake_minimum_required(VERSION 3.16)

# obs#30: libobs spreads sources' deferred updates over frames (a per-tick_sources budget), so an OBS
# start's first ticks do not stall the graphics thread. Builds against the OBS SDK this repo's CI
# installs; runs under xvfb (libobs-opengl needs an X display).
project(deferred-update-test LANGUAGES C)

find_package(libobs REQUIRED)

add_executable(deferred-update-test deferred_update_test.c)
target_link_libraries(deferred-update-test PRIVATE OBS::libobs)
target_compile_options(deferred-update-test PRIVATE -Wall -Wextra -Werror)
