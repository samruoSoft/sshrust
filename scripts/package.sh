#!/bin/bash
# 打包 SSHRust：release 构建 → .app bundle → DMG 安装镜像。
# 用法：scripts/package.sh [版本号，默认读 app/Cargo.toml]
set -euo pipefail
cd "$(dirname "$0")/.."

APP_NAME="SSHRust"
BIN="sshrust-app"
VERSION="${1:-$(sed -n 's/^version = "\(.*\)"/\1/p' app/Cargo.toml | head -1)}"
DIST="target/dist"
APP="$DIST/$APP_NAME.app"
STAGE_ICONS="target/iconset"

echo "==> 版本 $VERSION"

echo "==> release 构建"
cargo build --release -p "$BIN"

echo "==> 生成应用图标（scripts/make_icon.py → iconset → icns）"
mkdir -p "$STAGE_ICONS"
python3 scripts/make_icon.py "$STAGE_ICONS/master.png"
rm -rf "$STAGE_ICONS/$APP_NAME.iconset"
mkdir "$STAGE_ICONS/$APP_NAME.iconset"
for sz in 16 32 64 128 256 512; do
  sips -z $sz $sz "$STAGE_ICONS/master.png" --out "$STAGE_ICONS/$APP_NAME.iconset/icon_${sz}x${sz}.png" >/dev/null
  sips -z $((sz * 2)) $((sz * 2)) "$STAGE_ICONS/master.png" --out "$STAGE_ICONS/$APP_NAME.iconset/icon_${sz}x${sz}@2x.png" >/dev/null
done
sips -z 1024 1024 "$STAGE_ICONS/master.png" --out "$STAGE_ICONS/$APP_NAME.iconset/icon_512x512@2x.png" >/dev/null
iconutil -c icns "$STAGE_ICONS/$APP_NAME.iconset" -o "$STAGE_ICONS/AppIcon.icns"

echo "==> 组装 $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "target/release/$BIN" "$APP/Contents/MacOS/$BIN"
cp "$STAGE_ICONS/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns"
strip "$APP/Contents/MacOS/$BIN" || true

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>              <string>$APP_NAME</string>
    <key>CFBundleDisplayName</key>       <string>$APP_NAME</string>
    <key>CFBundleExecutable</key>        <string>$BIN</string>
    <key>CFBundleIdentifier</key>        <string>com.samruosoft.sshrust</string>
    <key>CFBundleVersion</key>           <string>$VERSION</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundlePackageType</key>       <string>APPL</string>
    <key>CFBundleIconFile</key>          <string>AppIcon</string>
    <key>LSMinimumSystemVersion</key>    <string>13.0</string>
    <key>NSHighResolutionCapable</key>   <true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key> <true/>
    <key>LSApplicationCategoryType</key> <string>public.app-category.developer-tools</string>
    <key>NSPrincipalClass</key>          <string>NSApplication</string>
</dict>
</plist>
PLIST

# ad-hoc 签名：strip 作废链接器签名后重签（无开发者证书的本地分发标准做法；
# 必须在 Info.plist 写完之后，否则后写文件会破坏签名封条）
codesign --force --sign - "$APP" >/dev/null

echo "==> 生成 DMG"
rm -rf "$DIST/dmg"
mkdir -p "$DIST/dmg"
cp -R "$APP" "$DIST/dmg/"
ln -s /Applications "$DIST/dmg/Applications"
rm -f "$DIST/$APP_NAME-$VERSION.dmg"
hdiutil create -volname "$APP_NAME" -srcfolder "$DIST/dmg" -ov -format UDZO \
    "$DIST/$APP_NAME-$VERSION.dmg" >/dev/null

echo "==> 完成："
ls -lh "$DIST/$APP_NAME-$VERSION.dmg" | awk '{print "    " $NF "  " $5}'
