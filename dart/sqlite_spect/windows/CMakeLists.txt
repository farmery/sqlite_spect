# Windows CMakeLists for the sqlite_spect Flutter FFI plugin. Packages
# the DLL produced by scripts/build_windows.ps1 into the consumer bundle.

cmake_minimum_required(VERSION 3.14)

set(PROJECT_NAME "sqlite_spect")
project(${PROJECT_NAME} LANGUAGES CXX)

set(sqlite_spect_bundled_libraries
  "${CMAKE_CURRENT_SOURCE_DIR}/sqlite_spect_ffi.dll"
  PARENT_SCOPE
)
