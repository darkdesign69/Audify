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
# Gradle later runs `node <cli> android android-studio-script`, and the Tauri CLI reports its own
# path as just "tauri" (resolved inside src-tauri/) when started from node_modules/.bin.
# Make that path a real file pointing at the CLI so Gradle can find it.
CLI_DIR="$(readlink -f "$ROOT/ui/node_modules/@tauri-apps/cli")"
CLI_JS="$CLI_DIR/tauri.js"
[ -f "$CLI_JS" ] || CLI_JS="$(readlink -f "$TAURI")"
echo "Tauri CLI entry: $CLI_JS"
ln -sf "$CLI_JS" "$ROOT/src-tauri/tauri"
ls -l "$ROOT/src-tauri/tauri"
if ! "$TAURI" android build --apk --target aarch64 --ci 2>&1 | tee /tmp/tauri-build.log; then
  echo "::endgroup::"
  echo "::group::ROOT CAUSE (short)"
  # cargo prints colour codes, which broke the earlier search: strip them first
  sed -E 's/\x1b\[[0-9;]*[A-Za-z]//g' /tmp/tauri-build.log | grep -v 'Io(Env' | cut -c1-220 > /tmp/clean.log
  echo "--- first rust/linker errors ---"
  grep -n -m4 -B2 -A18 -E '^ *(error(\[E[0-9]+\])?:|.*ld\.lld: error|.*undefined (symbol|reference)|.*unable to find|.*cannot find)' /tmp/clean.log | head -90
  echo "--- last 40 lines before the Gradle summary ---"
  grep -n -B40 -m1 'FAILURE: Build failed' /tmp/clean.log | head -45
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
