cmake_minimum_required(VERSION 3.16)

# obs#57 / obs#56: OBS's audio-thread cadence at the raw audio callback (dante-input's TX tap) with a
# directly stamped source smooth, pair-jittered and drifting (the fork's STAMP-basis ASRC follows the
# stamps). Builds against the OBS SDK this repo's CI installs; runs under xvfb.
project(audio-cadence-test LANGUAGES C)

find_package(libobs REQUIRED)

add_executable(audio-cadence-test audio_cadence_test.c)
target_link_libraries(audio-cadence-test PRIVATE OBS::libobs)
target_compile_options(audio-cadence-test PRIVATE -Wall -Wextra -Werror)
