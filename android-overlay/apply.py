#!/usr/bin/env python3
"""Applied by scripts/build-android.sh after `tauri android init`: adds the Kotlin files, the
manifest entries and the libmpv libraries to the generated Gradle project."""
import json, os, re, shutil, sys

root = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
gen = os.path.join(root, "src-tauri", "gen", "android", "app")
conf = json.load(open(os.path.join(root, "src-tauri", "tauri.conf.json")))
ident = conf["identifier"]
pkg_dir = os.path.join(gen, "src", "main", "java", *ident.split("."))
os.makedirs(pkg_dir, exist_ok=True)

for name in ("MainActivity.kt", "PlaybackService.kt"):
    src = open(os.path.join(root, "android-overlay", name)).read()
    src = re.sub(r"^package .*$", "package " + ident, src, count=1, flags=re.M)
    open(os.path.join(pkg_dir, name), "w").write(src)

# --- manifest
mpath = os.path.join(gen, "src", "main", "AndroidManifest.xml")
m = open(mpath).read()
perms = [
    "android.permission.INTERNET",
    "android.permission.ACCESS_NETWORK_STATE",
    "android.permission.ACCESS_WIFI_STATE",
    "android.permission.WAKE_LOCK",
    "android.permission.FOREGROUND_SERVICE",
    "android.permission.FOREGROUND_SERVICE_MEDIA_PLAYBACK",
    "android.permission.POST_NOTIFICATIONS",
]
add = "".join(
    f'    <uses-permission android:name="{p}" />\n' for p in perms if p not in m
)
m = m.replace("<application", add + "    <application", 1)
if "PlaybackService" not in m:
    m = m.replace(
        "</application>",
        '        <service android:name=".PlaybackService" android:exported="false"\n'
        '            android:foregroundServiceType="mediaPlayback" />\n    </application>',
        1,
    )
# the page loads http://127.0.0.1 (loopback audio/video proxy), which Android blocks by default
m = m.replace('${usesCleartextTraffic}', "true")
m = re.sub(r'android:usesCleartextTraffic="[^"]*"', 'android:usesCleartextTraffic="true"', m)
if "usesCleartextTraffic" not in m:
    m = m.replace("<application", '<application android:usesCleartextTraffic="true"', 1)
open(mpath, "w").write(m)

# --- gradle
gpath = os.path.join(gen, "build.gradle.kts")
g = open(gpath).read()
g = g.replace('manifestPlaceholders["usesCleartextTraffic"] = "false"',
              'manifestPlaceholders["usesCleartextTraffic"] = "true"')
if "mpvLibs" not in g:
    g = re.sub(r"^android \{", 'android {\n    sourceSets { getByName("main") { jniLibs.srcDir("src/main/mpvLibs") } }',
               g, count=1, flags=re.M)
if "mpv-classes.jar" not in g:
    g = re.sub(r"^dependencies \{", 'dependencies {\n    implementation(files("libs/mpv-classes.jar"))',
               g, count=1, flags=re.M)
open(gpath, "w").write(g)
print("overlay applied to", gen)
