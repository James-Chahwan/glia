cmake_minimum_required(VERSION 3.16)
project(shop CXX)
add_library(shop src/cart.cpp)
target_include_directories(shop PUBLIC include)
