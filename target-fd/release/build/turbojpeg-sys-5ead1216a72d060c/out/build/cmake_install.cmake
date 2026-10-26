# Install script for directory: /Users/itamarwe/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/turbojpeg-sys-1.2.0/libjpeg-turbo

# Set the install prefix
if(NOT DEFINED CMAKE_INSTALL_PREFIX)
  set(CMAKE_INSTALL_PREFIX "/Users/itamarwe/Documents/code/ecw2tiff/target-fd/release/build/turbojpeg-sys-5ead1216a72d060c/out")
endif()
string(REGEX REPLACE "/$" "" CMAKE_INSTALL_PREFIX "${CMAKE_INSTALL_PREFIX}")

# Set the install configuration name.
if(NOT DEFINED CMAKE_INSTALL_CONFIG_NAME)
  if(BUILD_TYPE)
    string(REGEX REPLACE "^[^A-Za-z0-9_]+" ""
           CMAKE_INSTALL_CONFIG_NAME "${BUILD_TYPE}")
  else()
    set(CMAKE_INSTALL_CONFIG_NAME "Release")
  endif()
  message(STATUS "Install configuration: \"${CMAKE_INSTALL_CONFIG_NAME}\"")
endif()

# Set the component getting installed.
if(NOT CMAKE_INSTALL_COMPONENT)
  if(COMPONENT)
    message(STATUS "Install component: \"${COMPONENT}\"")
    set(CMAKE_INSTALL_COMPONENT "${COMPONENT}")
  else()
    set(CMAKE_INSTALL_COMPONENT)
  endif()
endif()

# Is this installation the result of a crosscompile?
if(NOT DEFINED CMAKE_CROSSCOMPILING)
  set(CMAKE_CROSSCOMPILING "FALSE")
endif()

# Set path to fallback-tool for dependency-resolution.
if(NOT DEFINED CMAKE_OBJDUMP)
  set(CMAKE_OBJDUMP "/usr/bin/objdump")
endif()

if(NOT CMAKE_INSTALL_LOCAL_ONLY)
  # Include the install script for the subdirectory.
  include("/Users/itamarwe/Documents/code/ecw2tiff/target-fd/release/build/turbojpeg-sys-5ead1216a72d060c/out/build/simd/cmake_install.cmake")
endif()

if(NOT CMAKE_INSTALL_LOCAL_ONLY)
  # Include the install script for the subdirectory.
  include("/Users/itamarwe/Documents/code/ecw2tiff/target-fd/release/build/turbojpeg-sys-5ead1216a72d060c/out/build/src/md5/cmake_install.cmake")
endif()

if(CMAKE_INSTALL_COMPONENT STREQUAL "lib" OR NOT CMAKE_INSTALL_COMPONENT)
  file(INSTALL DESTINATION "${CMAKE_INSTALL_PREFIX}/lib" TYPE STATIC_LIBRARY FILES "/Users/itamarwe/Documents/code/ecw2tiff/target-fd/release/build/turbojpeg-sys-5ead1216a72d060c/out/build/libturbojpeg.a")
  if(EXISTS "$ENV{DESTDIR}${CMAKE_INSTALL_PREFIX}/lib/libturbojpeg.a" AND
     NOT IS_SYMLINK "$ENV{DESTDIR}${CMAKE_INSTALL_PREFIX}/lib/libturbojpeg.a")
    execute_process(COMMAND "/usr/bin/ranlib" "$ENV{DESTDIR}${CMAKE_INSTALL_PREFIX}/lib/libturbojpeg.a")
  endif()
endif()

if(CMAKE_INSTALL_COMPONENT STREQUAL "bin" OR NOT CMAKE_INSTALL_COMPONENT)
  file(INSTALL DESTINATION "${CMAKE_INSTALL_PREFIX}/bin" TYPE PROGRAM RENAME "tjbench" FILES "/Users/itamarwe/Documents/code/ecw2tiff/target-fd/release/build/turbojpeg-sys-5ead1216a72d060c/out/build/tjbench-static")
endif()

if(CMAKE_INSTALL_COMPONENT STREQUAL "include" OR NOT CMAKE_INSTALL_COMPONENT)
  file(INSTALL DESTINATION "${CMAKE_INSTALL_PREFIX}/include" TYPE FILE FILES "/Users/itamarwe/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/turbojpeg-sys-1.2.0/libjpeg-turbo/src/turbojpeg.h")
endif()

if(CMAKE_INSTALL_COMPONENT STREQUAL "lib" OR NOT CMAKE_INSTALL_COMPONENT)
  file(INSTALL DESTINATION "${CMAKE_INSTALL_PREFIX}/lib" TYPE STATIC_LIBRARY FILES "/Users/itamarwe/Documents/code/ecw2tiff/target-fd/release/build/turbojpeg-sys-5ead1216a72d060c/out/build/libjpeg.a")
  if(EXISTS "$ENV{DESTDIR}${CMAKE_INSTALL_PREFIX}/lib/libjpeg.a" AND
     NOT IS_SYMLINK "$ENV{DESTDIR}${CMAKE_INSTALL_PREFIX}/lib/libjpeg.a")
    execute_process(COMMAND "/usr/bin/ranlib" "$ENV{DESTDIR}${CMAKE_INSTALL_PREFIX}/lib/libjpeg.a")
  endif()
endif()

if(CMAKE_INSTALL_COMPONENT STREQUAL "bin" OR NOT CMAKE_INSTALL_COMPONENT)
  file(INSTALL DESTINATION "${CMAKE_INSTALL_PREFIX}/bin" TYPE PROGRAM RENAME "cjpeg" FILES "/Users/itamarwe/Documents/code/ecw2tiff/target-fd/release/build/turbojpeg-sys-5ead1216a72d060c/out/build/cjpeg-static")
endif()

if(CMAKE_INSTALL_COMPONENT STREQUAL "bin" OR NOT CMAKE_INSTALL_COMPONENT)
  file(INSTALL DESTINATION "${CMAKE_INSTALL_PREFIX}/bin" TYPE PROGRAM RENAME "djpeg" FILES "/Users/itamarwe/Documents/code/ecw2tiff/target-fd/release/build/turbojpeg-sys-5ead1216a72d060c/out/build/djpeg-static")
endif()

if(CMAKE_INSTALL_COMPONENT STREQUAL "bin" OR NOT CMAKE_INSTALL_COMPONENT)
  file(INSTALL DESTINATION "${CMAKE_INSTALL_PREFIX}/bin" TYPE PROGRAM RENAME "jpegtran" FILES "/Users/itamarwe/Documents/code/ecw2tiff/target-fd/release/build/turbojpeg-sys-5ead1216a72d060c/out/build/jpegtran-static")
endif()

if(CMAKE_INSTALL_COMPONENT STREQUAL "bin" OR NOT CMAKE_INSTALL_COMPONENT)
  file(INSTALL DESTINATION "${CMAKE_INSTALL_PREFIX}/bin" TYPE EXECUTABLE FILES "/Users/itamarwe/Documents/code/ecw2tiff/target-fd/release/build/turbojpeg-sys-5ead1216a72d060c/out/build/rdjpgcom")
  if(EXISTS "$ENV{DESTDIR}${CMAKE_INSTALL_PREFIX}/bin/rdjpgcom" AND
     NOT IS_SYMLINK "$ENV{DESTDIR}${CMAKE_INSTALL_PREFIX}/bin/rdjpgcom")
    if(CMAKE_INSTALL_DO_STRIP)
      execute_process(COMMAND "/usr/bin/strip" -u -r "$ENV{DESTDIR}${CMAKE_INSTALL_PREFIX}/bin/rdjpgcom")
    endif()
  endif()
endif()

if(CMAKE_INSTALL_COMPONENT STREQUAL "bin" OR NOT CMAKE_INSTALL_COMPONENT)
  file(INSTALL DESTINATION "${CMAKE_INSTALL_PREFIX}/bin" TYPE EXECUTABLE FILES "/Users/itamarwe/Documents/code/ecw2tiff/target-fd/release/build/turbojpeg-sys-5ead1216a72d060c/out/build/wrjpgcom")
  if(EXISTS "$ENV{DESTDIR}${CMAKE_INSTALL_PREFIX}/bin/wrjpgcom" AND
     NOT IS_SYMLINK "$ENV{DESTDIR}${CMAKE_INSTALL_PREFIX}/bin/wrjpgcom")
    if(CMAKE_INSTALL_DO_STRIP)
      execute_process(COMMAND "/usr/bin/strip" -u -r "$ENV{DESTDIR}${CMAKE_INSTALL_PREFIX}/bin/wrjpgcom")
    endif()
  endif()
endif()

if(CMAKE_INSTALL_COMPONENT STREQUAL "doc" OR NOT CMAKE_INSTALL_COMPONENT)
  file(INSTALL DESTINATION "${CMAKE_INSTALL_PREFIX}/share/doc/libjpeg-turbo" TYPE FILE FILES
    "/Users/itamarwe/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/turbojpeg-sys-1.2.0/libjpeg-turbo/README.ijg"
    "/Users/itamarwe/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/turbojpeg-sys-1.2.0/libjpeg-turbo/README.md"
    "/Users/itamarwe/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/turbojpeg-sys-1.2.0/libjpeg-turbo/src/example.c"
    "/Users/itamarwe/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/turbojpeg-sys-1.2.0/libjpeg-turbo/src/tjcomp.c"
    "/Users/itamarwe/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/turbojpeg-sys-1.2.0/libjpeg-turbo/src/tjdecomp.c"
    "/Users/itamarwe/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/turbojpeg-sys-1.2.0/libjpeg-turbo/src/tjtran.c"
    "/Users/itamarwe/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/turbojpeg-sys-1.2.0/libjpeg-turbo/doc/libjpeg.txt"
    "/Users/itamarwe/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/turbojpeg-sys-1.2.0/libjpeg-turbo/doc/structure.txt"
    "/Users/itamarwe/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/turbojpeg-sys-1.2.0/libjpeg-turbo/doc/usage.txt"
    "/Users/itamarwe/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/turbojpeg-sys-1.2.0/libjpeg-turbo/doc/wizard.txt"
    "/Users/itamarwe/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/turbojpeg-sys-1.2.0/libjpeg-turbo/LICENSE.md"
    )
endif()

if(CMAKE_INSTALL_COMPONENT STREQUAL "man" OR NOT CMAKE_INSTALL_COMPONENT)
  file(INSTALL DESTINATION "${CMAKE_INSTALL_PREFIX}/share/man/man1" TYPE FILE FILES
    "/Users/itamarwe/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/turbojpeg-sys-1.2.0/libjpeg-turbo/doc/cjpeg.1"
    "/Users/itamarwe/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/turbojpeg-sys-1.2.0/libjpeg-turbo/doc/djpeg.1"
    "/Users/itamarwe/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/turbojpeg-sys-1.2.0/libjpeg-turbo/doc/jpegtran.1"
    "/Users/itamarwe/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/turbojpeg-sys-1.2.0/libjpeg-turbo/doc/rdjpgcom.1"
    "/Users/itamarwe/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/turbojpeg-sys-1.2.0/libjpeg-turbo/doc/wrjpgcom.1"
    )
endif()

if(CMAKE_INSTALL_COMPONENT STREQUAL "lib" OR NOT CMAKE_INSTALL_COMPONENT)
  file(INSTALL DESTINATION "${CMAKE_INSTALL_PREFIX}/lib/pkgconfig" TYPE FILE FILES "/Users/itamarwe/Documents/code/ecw2tiff/target-fd/release/build/turbojpeg-sys-5ead1216a72d060c/out/build/pkgscripts/libjpeg.pc")
endif()

if(CMAKE_INSTALL_COMPONENT STREQUAL "lib" OR NOT CMAKE_INSTALL_COMPONENT)
  file(INSTALL DESTINATION "${CMAKE_INSTALL_PREFIX}/lib/pkgconfig" TYPE FILE FILES "/Users/itamarwe/Documents/code/ecw2tiff/target-fd/release/build/turbojpeg-sys-5ead1216a72d060c/out/build/pkgscripts/libturbojpeg.pc")
endif()

if(CMAKE_INSTALL_COMPONENT STREQUAL "lib" OR NOT CMAKE_INSTALL_COMPONENT)
  file(INSTALL DESTINATION "${CMAKE_INSTALL_PREFIX}/lib/cmake/libjpeg-turbo" TYPE FILE FILES
    "/Users/itamarwe/Documents/code/ecw2tiff/target-fd/release/build/turbojpeg-sys-5ead1216a72d060c/out/build/pkgscripts/libjpeg-turboConfig.cmake"
    "/Users/itamarwe/Documents/code/ecw2tiff/target-fd/release/build/turbojpeg-sys-5ead1216a72d060c/out/build/pkgscripts/libjpeg-turboConfigVersion.cmake"
    )
endif()

if(CMAKE_INSTALL_COMPONENT STREQUAL "lib" OR NOT CMAKE_INSTALL_COMPONENT)
  if(EXISTS "$ENV{DESTDIR}${CMAKE_INSTALL_PREFIX}/lib/cmake/libjpeg-turbo/libjpeg-turboTargets.cmake")
    file(DIFFERENT _cmake_export_file_changed FILES
         "$ENV{DESTDIR}${CMAKE_INSTALL_PREFIX}/lib/cmake/libjpeg-turbo/libjpeg-turboTargets.cmake"
         "/Users/itamarwe/Documents/code/ecw2tiff/target-fd/release/build/turbojpeg-sys-5ead1216a72d060c/out/build/CMakeFiles/Export/f0d506f335508d6549928070f26fb787/libjpeg-turboTargets.cmake")
    if(_cmake_export_file_changed)
      file(GLOB _cmake_old_config_files "$ENV{DESTDIR}${CMAKE_INSTALL_PREFIX}/lib/cmake/libjpeg-turbo/libjpeg-turboTargets-*.cmake")
      if(_cmake_old_config_files)
        string(REPLACE ";" ", " _cmake_old_config_files_text "${_cmake_old_config_files}")
        message(STATUS "Old export file \"$ENV{DESTDIR}${CMAKE_INSTALL_PREFIX}/lib/cmake/libjpeg-turbo/libjpeg-turboTargets.cmake\" will be replaced.  Removing files [${_cmake_old_config_files_text}].")
        unset(_cmake_old_config_files_text)
        file(REMOVE ${_cmake_old_config_files})
      endif()
      unset(_cmake_old_config_files)
    endif()
    unset(_cmake_export_file_changed)
  endif()
  file(INSTALL DESTINATION "${CMAKE_INSTALL_PREFIX}/lib/cmake/libjpeg-turbo" TYPE FILE FILES "/Users/itamarwe/Documents/code/ecw2tiff/target-fd/release/build/turbojpeg-sys-5ead1216a72d060c/out/build/CMakeFiles/Export/f0d506f335508d6549928070f26fb787/libjpeg-turboTargets.cmake")
  if(CMAKE_INSTALL_CONFIG_NAME MATCHES "^([Rr][Ee][Ll][Ee][Aa][Ss][Ee])$")
    file(INSTALL DESTINATION "${CMAKE_INSTALL_PREFIX}/lib/cmake/libjpeg-turbo" TYPE FILE FILES "/Users/itamarwe/Documents/code/ecw2tiff/target-fd/release/build/turbojpeg-sys-5ead1216a72d060c/out/build/CMakeFiles/Export/f0d506f335508d6549928070f26fb787/libjpeg-turboTargets-release.cmake")
  endif()
endif()

if(CMAKE_INSTALL_COMPONENT STREQUAL "include" OR NOT CMAKE_INSTALL_COMPONENT)
  file(INSTALL DESTINATION "${CMAKE_INSTALL_PREFIX}/include" TYPE FILE FILES
    "/Users/itamarwe/Documents/code/ecw2tiff/target-fd/release/build/turbojpeg-sys-5ead1216a72d060c/out/build/jconfig.h"
    "/Users/itamarwe/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/turbojpeg-sys-1.2.0/libjpeg-turbo/src/jerror.h"
    "/Users/itamarwe/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/turbojpeg-sys-1.2.0/libjpeg-turbo/src/jmorecfg.h"
    "/Users/itamarwe/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/turbojpeg-sys-1.2.0/libjpeg-turbo/src/jpeglib.h"
    )
endif()

string(REPLACE ";" "\n" CMAKE_INSTALL_MANIFEST_CONTENT
       "${CMAKE_INSTALL_MANIFEST_FILES}")
if(CMAKE_INSTALL_LOCAL_ONLY)
  file(WRITE "/Users/itamarwe/Documents/code/ecw2tiff/target-fd/release/build/turbojpeg-sys-5ead1216a72d060c/out/build/install_local_manifest.txt"
     "${CMAKE_INSTALL_MANIFEST_CONTENT}")
endif()
if(CMAKE_INSTALL_COMPONENT)
  if(CMAKE_INSTALL_COMPONENT MATCHES "^[a-zA-Z0-9_.+-]+$")
    set(CMAKE_INSTALL_MANIFEST "install_manifest_${CMAKE_INSTALL_COMPONENT}.txt")
  else()
    string(MD5 CMAKE_INST_COMP_HASH "${CMAKE_INSTALL_COMPONENT}")
    set(CMAKE_INSTALL_MANIFEST "install_manifest_${CMAKE_INST_COMP_HASH}.txt")
    unset(CMAKE_INST_COMP_HASH)
  endif()
else()
  set(CMAKE_INSTALL_MANIFEST "install_manifest.txt")
endif()

if(NOT CMAKE_INSTALL_LOCAL_ONLY)
  file(WRITE "/Users/itamarwe/Documents/code/ecw2tiff/target-fd/release/build/turbojpeg-sys-5ead1216a72d060c/out/build/${CMAKE_INSTALL_MANIFEST}"
     "${CMAKE_INSTALL_MANIFEST_CONTENT}")
endif()
