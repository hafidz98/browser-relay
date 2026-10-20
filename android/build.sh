#!/bin/bash
# Build the browser-relay native Android client without Gradle.
#   ./build.sh [versionCode] [versionName]
# Recipe mirrors ~/workspace/kunci-lokal/build.sh:
# javac -> d8 -> aapt2 compile/link -> zip -> zipalign -> apksigner.
set -e
P="$(cd "$(dirname "$0")" && pwd)"
SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/workspace/android-sdk}}"
BT="$SDK/build-tools/34.0.0"
PLAT="$SDK/platforms/android-34-ext12/android.jar"
[ -f "$PLAT" ] || PLAT="$SDK/platforms/android-34/android.jar"
export JAVA_HOME="${JAVA_HOME:-$SDK/jdk17}"
export PATH="$JAVA_HOME/bin:$BT:$PATH"
VCODE="${1:-1}"
VNAME="${2:-1.0}"
OUT="$P/browserrelay-$VNAME.apk"

mkdir -p "$P/obj" "$P/dex" "$P/build" "$P/gen"

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
