# Maya2C C++ SDK: header-only C++20 over the cargo-built sdk-ffi cdylib.
#
# Build sdk-ffi first (from the repo root):  cargo build -p maya-sdk-ffi --lib
# then point MAYA_SDK_FFI_DIR at the directory holding the library if it is
# not the default target/debug. sdk-ffi ships a cdylib, not a staticlib, so
# this links the shared library and the tests need it at run time.
cmake_minimum_required(VERSION 3.22)
project(maya2c_sdk_cpp VERSION 0.1.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

set(MAYA_SDK_FFI_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../../target/debug"
    CACHE PATH "Directory holding the cargo-built maya_sdk_ffi library")
option(MAYA2C_SDK_BUILD_TESTS "Build the GoogleTest suite" ${PROJECT_IS_TOP_LEVEL})
option(MAYA2C_SDK_BUILD_EXAMPLES "Build the example program" ${PROJECT_IS_TOP_LEVEL})

# On Windows the import library cargo writes is maya_sdk_ffi.dll.lib.
find_library(MAYA_SDK_FFI_LIBRARY
  NAMES maya_sdk_ffi maya_sdk_ffi.dll
  PATHS "${MAYA_SDK_FFI_DIR}"
  NO_DEFAULT_PATH)
if(NOT MAYA_SDK_FFI_LIBRARY)
  message(FATAL_ERROR
    "maya_sdk_ffi not found in ${MAYA_SDK_FFI_DIR}. Run `cargo build -p maya-sdk-ffi --lib` "
    "at the repo root, or pass -DMAYA_SDK_FFI_DIR=<dir>.")
endif()

add_library(maya2c_sdk_ffi SHARED IMPORTED)
if(WIN32)
  set_target_properties(maya2c_sdk_ffi PROPERTIES
    IMPORTED_IMPLIB "${MAYA_SDK_FFI_LIBRARY}"
    IMPORTED_LOCATION "${MAYA_SDK_FFI_DIR}/maya_sdk_ffi.dll")
else()
  # A Rust cdylib carries no SONAME; this makes the link record the file
  # name rather than the absolute build path.
  set_target_properties(maya2c_sdk_ffi PROPERTIES
    IMPORTED_LOCATION "${MAYA_SDK_FFI_LIBRARY}"
    IMPORTED_NO_SONAME TRUE)
endif()

add_library(maya2c_sdk INTERFACE)
add_library(maya2c::sdk ALIAS maya2c_sdk)
target_include_directories(maya2c_sdk INTERFACE "${CMAKE_CURRENT_SOURCE_DIR}/include")
target_compile_features(maya2c_sdk INTERFACE cxx_std_20)
target_link_libraries(maya2c_sdk INTERFACE maya2c_sdk_ffi)

# Where the shared library is found at run time by tests and the example.
# ENVIRONMENT_MODIFICATION, because a Windows PATH holds ';', which a plain
# ENVIRONMENT property would split into separate entries.
if(WIN32)
  set(MAYA2C_RUNTIME_ENV "PATH=path_list_prepend:${MAYA_SDK_FFI_DIR}")
elseif(APPLE)
  set(MAYA2C_RUNTIME_ENV "DYLD_LIBRARY_PATH=path_list_prepend:${MAYA_SDK_FFI_DIR}")
else()
  set(MAYA2C_RUNTIME_ENV "LD_LIBRARY_PATH=path_list_prepend:${MAYA_SDK_FFI_DIR}")
endif()

function(maya2c_warnings target)
  if(MSVC)
    target_compile_options(${target} PRIVATE /W4 /utf-8)
  else()
    target_compile_options(${target} PRIVATE -Wall -Wextra -Wpedantic -Wconversion)
  endif()
endfunction()

if(MAYA2C_SDK_BUILD_EXAMPLES)
  add_executable(sign_and_verify examples/sign_and_verify.cpp)
  target_link_libraries(sign_and_verify PRIVATE maya2c::sdk)
  maya2c_warnings(sign_and_verify)
endif()

if(MAYA2C_SDK_BUILD_TESTS)
  enable_testing()
  find_package(GTest QUIET)
  if(NOT GTest_FOUND)
    include(FetchContent)
    FetchContent_Declare(googletest
      GIT_REPOSITORY https://github.com/google/googletest.git
      GIT_TAG v1.15.2)
    set(gtest_force_shared_crt ON CACHE BOOL "" FORCE)
    FetchContent_MakeAvailable(googletest)
  endif()

  add_executable(maya2c_sdk_tests
    tests/amount_test.cpp
    tests/address_test.cpp
    tests/error_test.cpp
    tests/keys_test.cpp)
  target_link_libraries(maya2c_sdk_tests PRIVATE maya2c::sdk GTest::gtest_main)
  maya2c_warnings(maya2c_sdk_tests)

  add_test(NAME maya2c_sdk_tests COMMAND maya2c_sdk_tests)
  set_tests_properties(maya2c_sdk_tests PROPERTIES ENVIRONMENT_MODIFICATION "${MAYA2C_RUNTIME_ENV}")
  if(MAYA2C_SDK_BUILD_EXAMPLES)
    add_test(NAME sign_and_verify COMMAND sign_and_verify)
    set_tests_properties(sign_and_verify PROPERTIES ENVIRONMENT_MODIFICATION "${MAYA2C_RUNTIME_ENV}")
  endif()
endif()
