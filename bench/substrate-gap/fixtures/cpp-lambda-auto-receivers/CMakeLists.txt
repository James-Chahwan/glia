cmake_minimum_required(VERSION 3.16)
project(app CXX)
add_executable(app src/main.cpp src/box.cpp)
target_include_directories(app PRIVATE inc)
