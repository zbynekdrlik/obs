cmake_minimum_required(VERSION 3.16)

# obs#30: libobs's tick audit names a video tick over 20 ms (a source's or a tick callback's) on the
# graphics thread. Builds against the OBS SDK this repo's CI installs; runs under xvfb (libobs-opengl
# needs an X display).
project(tick-audit-test LANGUAGES C)

find_package(libobs REQUIRED)

add_executable(tick-audit-test tick_audit_test.c)
target_link_libraries(tick-audit-test PRIVATE OBS::libobs)
target_compile_options(tick-audit-test PRIVATE -Wall -Wextra -Werror)
# The audit names a tick callback by its symbol (dladdr): export the test's own symbols.
set_target_properties(tick-audit-test PROPERTIES ENABLE_EXPORTS ON)
