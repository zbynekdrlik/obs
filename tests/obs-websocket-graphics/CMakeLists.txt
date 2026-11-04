cmake_minimum_required(VERSION 3.16)

# camera-box#1387 (obs#1): obs-websocket's GL work (GetSourceScreenshot, CreateSceneItem) runs on the
# graphics thread, not on the plugin's Qt pool thread. Builds the plugin's libobs-only helper against
# the OBS SDK this repo's CI installs; runs under xvfb (libobs-opengl needs an X display).
project(obs-websocket-graphics-test LANGUAGES C CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

find_package(libobs REQUIRED)

set(helper_dir "${CMAKE_CURRENT_SOURCE_DIR}/../../vendor/obs-studio/plugins/obs-websocket/src/utils")

add_executable(ws-graphics-test ws_graphics_test.cpp "${helper_dir}/Obs_GraphicsTask.cpp")
target_include_directories(ws-graphics-test PRIVATE "${helper_dir}")
target_link_libraries(ws-graphics-test PRIVATE OBS::libobs)
target_compile_options(ws-graphics-test PRIVATE -Wall -Wextra -Werror)
