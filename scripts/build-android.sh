#!/usr/bin/env bash
# Builds the Android APK (arm64) and signs it. Used by both GitHub workflows.
# Needs: Rust (+ aarch64-linux-android target), Node, pnpm, JDK 17, Android SDK + NDK
# (ANDROID_HOME, NDK_HOME). Output: out/Spotless-arm64-v8a.apk
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$PWD"
AAR_URL="https://github.com/jarnedemeulemeester/libmpv-android/releases/download/v1.1.0/libmpv-release.aar"
TAURI="$ROOT/ui/node_modules/.bin/tauri"

echo "::group::UI dependencies"
(cd ui && pnpm install --frozen-lockfile)
echo "::endgroup::"

echo "::group::libmpv for Android"
mkdir -p .libmpv && cd .libmpv
[ -f libmpv-release.aar ] || curl -fsSL -o libmpv-release.aar "$AAR_URL"
rm -rf x && mkdir x && unzip -q libmpv-release.aar -d x
rm -rf "$ROOT/libmpv-link" && mkdir -p "$ROOT/libmpv-link"
cp x/jni/arm64-v8a/*.so "$ROOT/libmpv-link/"
cd "$ROOT"
echo "libmpv-link contents:"; ls -la libmpv-link
[ -f libmpv-link/libmpv.so ] || { echo "ERROR: libmpv.so missing from the AAR (jni/arm64-v8a)"; find .libmpv/x -name '*.so' | head -30; exit 1; }
echo "::endgroup::"

# The linker needs libmpv.so (and the FFmpeg libs it depends on) to resolve symbols.
export CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS="-L native=$ROOT/libmpv-link -C link-arg=-Wl,-rpath-link,$ROOT/libmpv-link -C link-arg=-Wl,--allow-shlib-undefined"
echo "::group::tauri android init"
rm -rf src-tauri/gen/android
"$TAURI" android init --ci
"$TAURI" icon src-tauri/icons/icon.png >/dev/null
echo "::endgroup::"

echo "::group::apply overlay"
GEN="$ROOT/src-tauri/gen/android/app"
mkdir -p "$GEN/src/main/mpvLibs/arm64-v8a" "$GEN/libs"
cp libmpv-link/*.so "$GEN/src/main/mpvLibs/arm64-v8a/"
cp .libmpv/x/classes.jar "$GEN/libs/mpv-classes.jar"
python3 android-overlay/apply.py
echo "::endgroup::"

echo "::group::tauri android build"
if ! "$TAURI" android build --apk --target aarch64 --ci 2>&1 | tee /tmp/tauri-build.log; then
  echo "::endgroup::"
  echo "::group::ROOT CAUSE (short)"
  grep -nE 'error(\[E[0-9]+\])?:|undefined (symbol|reference)|unable to find|cannot find|ld\.lld|What went wrong|Execution failed' /tmp/tauri-build.log | grep -v 'Io(Env' | cut -c1-250 | head -40
  echo "::endgroup::"
  exit 1
fi
echo "::endgroup::"

echo "::group::sign"
UNSIGNED="$(find src-tauri/gen/android/app/build/outputs/apk -name '*unsigned*.apk' | head -n1)"
[ -n "$UNSIGNED" ] || { echo "no APK produced"; find src-tauri/gen/android/app/build/outputs -type f; exit 1; }
BT="$(ls -d "$ANDROID_HOME"/build-tools/* | sort -V | tail -n1)"
mkdir -p out
"$BT/zipalign" -f -p 4 "$UNSIGNED" out/aligned.apk
"$BT/apksigner" sign --ks android-signing/spotless.jks --ks-pass pass:spotless --key-pass pass:spotless \
  --ks-key-alias spotless --out out/Spotless-arm64-v8a.apk out/aligned.apk
rm -f out/aligned.apk out/*.idsig
ls -lh out
echo "::endgroup::"
