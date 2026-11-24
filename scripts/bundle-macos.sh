#!/usr/bin/env bash
# Builds the universal Juicetray.app (ad-hoc signed) and zips it for release.
set -euo pipefail

cd "$(dirname "$0")/.."
command -v cargo >/dev/null || PATH="$HOME/.cargo/bin:$PATH"

# The app inherits its version from [workspace.package].
version=$(sed -n '/^\[workspace.package\]/,/^\[/{s/^version *= *"\(.*\)"/\1/p;}' Cargo.toml)
[[ -n "$version" ]] || { echo "workspace version not found in Cargo.toml" >&2; exit 1; }

targets=(aarch64-apple-darwin x86_64-apple-darwin)
for target in "${targets[@]}"; do
  cargo build --release -p juicetray --target "$target"
done

app=target/Juicetray.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

# One binary holding both architectures.
lipo -create -output "$app/Contents/MacOS/juicetray" \
  target/aarch64-apple-darwin/release/juicetray \
  target/x86_64-apple-darwin/release/juicetray

sed "s/__VERSION__/$version/g" packaging/macos/Info.plist >"$app/Contents/Info.plist"
cp packaging/macos/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"

# Licence texts: this app (GPL), Material Symbols icons (Apache-2.0), Inter font (OFL).
mkdir -p "$app/Contents/Resources/licenses"
cp LICENSE "$app/Contents/Resources/licenses/LICENSE"
cp crates/strip-render/assets/icons/LICENSE "$app/Contents/Resources/licenses/Material-Symbols-LICENSE"
cp crates/strip-render/assets/OFL.txt "$app/Contents/Resources/licenses/Inter-OFL.txt"

# Ad-hoc signature; --deep is unnecessary because the bundle has no nested code.
codesign --force -s - "$app"

ditto -c -k --keepParent "$app" target/juicetray-macos-universal.app.zip
echo "built $app ($version) and target/juicetray-macos-universal.app.zip"
