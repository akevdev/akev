# CMake toolchain file for building BoringSSL (boring-sys2, the `impersonate` feature) natively
# on Windows Arm64 with MSVC.
#
# BoringSSL's Arm64 assembly is GNU-syntax `.S`, which MSVC can't assemble: the Visual Studio
# generator leaves those files out while the C code still calls them, and linking fails with
# unresolved aes_hw_*, gcm_*_v8, sha*_block_data_order and friends. OPENSSL_NO_ASM builds the
# portable C implementations instead (x86 and x64 Windows use NASM and keep their assembly).
#
# cmake-rs reads it from CMAKE_TOOLCHAIN_FILE_aarch64_pc_windows_msvc, which
# .github/workflows/release.yml sets for that target only. With the Visual Studio generator a
# toolchain file changes nothing else about the build.
set(OPENSSL_NO_ASM ON CACHE BOOL "Build BoringSSL without assembly" FORCE)
