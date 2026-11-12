#!/usr/bin/env bash
# Кросс-компиляция узла p2p-node под Android и укладка в APK как libp2pnode.so.
#
# Нужно: Android NDK (ANDROID_NDK_HOME или $ANDROID_HOME/ndk/<версия>), CMake (+ ninja или make) и Rust-цели
#   rustup target add aarch64-linux-android x86_64-linux-android
# Использование: android/build_node.sh [release|debug] [abi…]   (по умолчанию release, arm64-v8a x86_64)
set -euo pipefail
cd "$(dirname "$0")/.."

PROFILE=${1:-release}; shift || true
ABIS=("$@"); [ ${#ABIS[@]} -eq 0 ] && ABIS=(arm64-v8a x86_64)
API=26

SDK=${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}
NDK=${ANDROID_NDK_HOME:-$(ls -d "$SDK"/ndk/* 2>/dev/null | sort -V | tail -1)}
[ -d "$NDK" ] || { echo "NDK не найден (ANDROID_NDK_HOME / \$ANDROID_HOME/ndk)"; exit 1; }
TC=$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin

# cargo/rustc с android-целями: rustup из ~/.cargo/bin, если есть (системный Rust обычно без них)
CARGO=cargo
[ -x "$HOME/.cargo/bin/cargo" ] && CARGO="$HOME/.cargo/bin/cargo" && export RUSTC="$HOME/.cargo/bin/rustc"

for abi in "${ABIS[@]}"; do
  case $abi in
    arm64-v8a) T=aarch64-linux-android ;;
    x86_64)    T=x86_64-linux-android ;;
    *) echo "неизвестный ABI $abi"; exit 1 ;;
  esac
  TU=$(echo "$T" | tr 'a-z-' 'A-Z_')
  export "CARGO_TARGET_${TU}_LINKER=$TC/${T}${API}-clang"
  export "CC_${T//-/_}=$TC/${T}${API}-clang"
  export "AR_${T//-/_}=$TC/llvm-ar"
  # libopus (звонки) собирается из исходников крейтом audiopus_sys через CMake: тулчейн NDK с нужным ABI
  # (сам тулчейн NDK по умолчанию берёт armeabi-v7a), статическая сборка, генератор Ninja
  tcf=target/android-$abi.cmake
  mkdir -p target
  printf 'set(ANDROID_ABI %s)\nset(ANDROID_PLATFORM %s)\ninclude(%s/build/cmake/android.toolchain.cmake)\n' "$abi" "$API" "$NDK" > "$tcf"
  export CMAKE_TOOLCHAIN_FILE="$PWD/$tcf" LIBOPUS_STATIC=1 CMAKE_POLICY_VERSION_MINIMUM=3.5
  command -v ninja >/dev/null && export CMAKE_GENERATOR=Ninja
  echo "== $abi ($T, $PROFILE)"
  flag=(); [ "$PROFILE" = release ] && flag=(--release)
  "$CARGO" build "${flag[@]}" --target "$T" --bin p2p-node
  out=android/app/src/main/jniLibs/$abi
  mkdir -p "$out"
  cp "target/$T/$PROFILE/p2p-node" "$out/libp2pnode.so"
  "$TC/llvm-strip" "$out/libp2pnode.so"
  ls -lh "$out/libp2pnode.so"
done
