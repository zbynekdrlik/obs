cmake_minimum_required(VERSION 3.16)

# camera-box issue 1495: the A/V-sync dock's measurement source reference
# (vendor/av-sync-dock/src/camera-box-measure-hold.hpp) through real libobs, built against the OBS
# SDK this repo's CI installs. No graphics: the test sources have no video.
project(av-sync-measure-hold-test LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

find_package(libobs REQUIRED)

add_executable(measure-hold-test measure_hold_test.cpp)
target_include_directories(measure-hold-test PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/../../vendor/av-sync-dock/src)
target_link_libraries(measure-hold-test PRIVATE OBS::libobs)
target_compile_options(measure-hold-test PRIVATE -Wall -Wextra -Werror)
