#!/bin/bash
# Build the browser-relay native Android client without Gradle.
#   ./build.sh [versionCode] [versionName]
# Recipe mirrors ~/workspace/kunci-lokal/build.sh:
# javac -> d8 -> aapt2 compile/link -> zip -> zipalign -> apksigner.
#
# The version comes from ONE place: the crate version in
# shell/Cargo.toml (the same number the release workflow uses for the
# .exe). The versionCode is derived from it, so it only ever grows:
# installing this APK over an older one must never fail with
# INSTALL_FAILED_VERSION_DOWNGRADE. An explicit versionCode argument
# is honoured when it is higher than the derived one; a lower one is
# refused, because it could not be installed over the current APK.
#   ./build.sh                 -> 0.9.61 / 9061 (from shell/Cargo.toml)
#   ./build.sh 25 2.5          -> 2.5    / 9061 (the code comes from Cargo)
set -e
P="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$P/.." && pwd)"
SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/workspace/android-sdk}}"

# ---- build tools: prefer 34.0.0, else the newest one that is there ----
BT_VER="${BT_VER:-}"
if [ -z "$BT_VER" ]; then
  if [ -d "$SDK/build-tools/34.0.0" ]; then
    BT_VER="34.0.0"
  else
    BT_VER="$(ls -1 "$SDK/build-tools" 2>/dev/null | sort -V | tail -1)"
  fi
fi
[ -n "$BT_VER" ] || { echo "no build-tools in $SDK/build-tools" >&2; exit 1; }
BT="$SDK/build-tools/$BT_VER"

# ---- platform: prefer android-34, else the newest one with an android.jar ----
PLAT="${PLAT:-}"
if [ -z "$PLAT" ]; then
  for cand in "$SDK/platforms/android-34/android.jar" \
              "$SDK/platforms/android-34-ext12/android.jar"; do
    [ -f "$cand" ] && { PLAT="$cand"; break; }
  done
fi
if [ -z "$PLAT" ]; then
  for d in $(ls -1 "$SDK/platforms" 2>/dev/null | sort -Vr); do
    if [ -f "$SDK/platforms/$d/android.jar" ]; then
      PLAT="$SDK/platforms/$d/android.jar"
      break
    fi
  done
fi
[ -n "$PLAT" ] || { echo "no platform in $SDK/platforms" >&2; exit 1; }

# ---- java ----
if [ -z "${JAVA_HOME:-}" ] || [ ! -x "$JAVA_HOME/bin/javac" ]; then
  if [ -x "$SDK/jdk17/bin/javac" ]; then
    JAVA_HOME="$SDK/jdk17"
  elif [ -x "/c/Program Files/Android/Android Studio/jbr/bin/javac" ]; then
    JAVA_HOME="/c/Program Files/Android/Android Studio/jbr"
  fi
fi
export JAVA_HOME="${JAVA_HOME:-}"
export PATH="$JAVA_HOME/bin:$BT:$PATH"

# ---- version: one source of truth (shell/Cargo.toml) ----
CARGO_TOML="$ROOT/shell/Cargo.toml"
[ -f "$CARGO_TOML" ] || { echo "no $CARGO_TOML" >&2; exit 1; }
CRATE_VERSION="$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' \
  "$CARGO_TOML" | head -1)"
[ -n "$CRATE_VERSION" ] || { echo "no version in $CARGO_TOML" >&2; exit 1; }
# major.minor.patch (a pre-release suffix is ignored for the code)
VER_NUM="${CRATE_VERSION%%-*}"
MAJOR="$(echo "$VER_NUM" | cut -d. -f1)"
MINOR="$(echo "$VER_NUM" | cut -d. -f2)"
PATCH="$(echo "$VER_NUM" | cut -d. -f3)"
MAJOR="${MAJOR:-0}"; MINOR="${MINOR:-0}"; PATCH="${PATCH:-0}"
# Monotonic and with headroom: 0.9.61 -> 9061, 0.10.0 -> 10000.
VCODE_FROM_CARGO=$(( MAJOR * 1000000 + MINOR * 1000 + PATCH ))
VCODE="$VCODE_FROM_CARGO"
VNAME="$CRATE_VERSION"
if [ -n "${1:-}" ]; then
  # An explicit code is honoured, but never a lower one: an APK with a
  # lower code cannot be installed over the one we publish
  # (INSTALL_FAILED_VERSION_DOWNGRADE).
  case "$1" in
    ''|*[!0-9]*) echo "note: '$1' is not a versionCode, using $VCODE_FROM_CARGO" >&2 ;;
    *)
      if [ "$1" -ge "$VCODE_FROM_CARGO" ]; then
        VCODE="$1"
      else
        echo "note: versionCode $1 is older than the crate version $CRATE_VERSION" \
             "(code $VCODE_FROM_CARGO) - using the latter" >&2
      fi
      ;;
  esac
fi
if [ -n "${2:-}" ]; then
  VNAME="$2"
fi
OUT="$P/browserrelay-$VNAME.apk"

# ---- fresh build: stale classes would end up in the APK ----
rm -rf "$P/obj" "$P/dex" "$P/gen"
mkdir -p "$P/obj" "$P/dex" "$P/build" "$P/gen"
rm -f "$P/build/res.zip" "$P/build/unaligned.apk" "$P/build/unsigned.apk" \
      "$P/build/aligned.apk"

echo "== aapt2 compile =="
"$BT/aapt2" compile --dir "$P/res" -o "$P/build/res.zip"

echo "== aapt2 link (vCode=$VCODE vName=$VNAME) =="
"$BT/aapt2" link -o "$P/build/unaligned.apk" \
  -I "$PLAT" \
  --manifest "$P/AndroidManifest.xml" \
  --java "$P/gen" \
  --version-code "$VCODE" --version-name "$VNAME" \
  "$P/build/res.zip"

echo "== javac =="
find "$P/src" "$P/gen" -name '*.java' > "$P/build/sources.txt"
javac -encoding UTF-8 -source 17 -target 17 -nowarn \
  -cp "$PLAT" \
  -d "$P/obj" @"$P/build/sources.txt"

echo "== d8 =="
d8 --min-api 26 --lib "$PLAT" --output "$P/dex" \
  $(find "$P/obj" -name '*.class')

echo "== add dex / align / sign =="
cp "$P/build/unaligned.apk" "$P/build/unsigned.apk"
(cd "$P/dex" && zip -q "$P/build/unsigned.apk" classes.dex)
"$BT/zipalign" -f 4 "$P/build/unsigned.apk" "$P/build/aligned.apk"
if [ ! -f "$P/build/debug.keystore" ]; then
  keytool -genkeypair -keystore "$P/build/debug.keystore" -alias androiddebugkey \
    -keyalg RSA -keysize 2048 -validity 10950 -storepass android -keypass android \
    -dname "CN=Android Debug,O=Android,C=US" 2>/dev/null
fi
"$BT/apksigner" sign --ks "$P/build/debug.keystore" --ks-pass pass:android \
  --out "$OUT" "$P/build/aligned.apk"

echo "== verify =="
"$BT/apksigner" verify "$OUT" && echo "APK_OK: $OUT"
"$BT/aapt" dump badging "$OUT" 2>/dev/null | grep -E "^package|launchable-activity" | head -4 || \
  "$BT/aapt2" dump badging "$OUT" 2>/dev/null | grep -E "^package|launchable-activity" | head -4
ls -la "$OUT"
