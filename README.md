# Spotless for Android

Android-only build of Spotless (GPL-3.0, fork of Limusic). Rust core + Svelte UI in Tauri, libmpv playback.

Build: push this repo to GitHub, open Actions, run "Apply Patch and Build APK". The signed arm64 APK is the `spotless-apk` artifact.
Release: run "Make Release" with a tag.
Patches: push a zip to the repo root; it is applied over the code (a name containing "full" replaces the whole project).

Not in this version: YouTube sign-in, lock-screen controls, importing files picked through Android's file picker.
