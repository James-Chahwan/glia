cmake_minimum_required(VERSION 3.16)
project(app CXX)
add_library(core src/box.cpp)
target_include_directories(core PUBLIC
  $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/inc>
  $<INSTALL_INTERFACE:include>)
add_executable(app src/main.cpp)
target_include_directories(app PRIVATE "$<$<CONFIG:Debug>:${CMAKE_CURRENT_SOURCE_DIR}/debug>")
target_link_libraries(app core)
