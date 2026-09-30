# Linux CMakeLists for the sqlite_spect Flutter FFI plugin. Packages the
# .so produced by scripts/build_linux.sh into the consumer bundle.

cmake_minimum_required(VERSION 3.10)

set(PROJECT_NAME "sqlite_spect")
project(${PROJECT_NAME} LANGUAGES CXX)

# List of files that must be included in the plugin distribution.
set(sqlite_spect_bundled_libraries
  "${CMAKE_CURRENT_SOURCE_DIR}/libsqlite_spect_ffi.so"
  PARENT_SCOPE
)
