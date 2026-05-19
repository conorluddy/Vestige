#!/usr/bin/env bash
# Build an unsigned Release Vestige.app and stage it at dist/Vestige.app.
# Run from anywhere: `./app/Vestige-Mac/scripts/build-app.sh`.

set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
app_root="$(cd -- "$script_dir/.." && pwd)"
project="$app_root/Vestige/Vestige.xcodeproj"
build_dir="$app_root/build"
dist_dir="$app_root/dist"

if [[ ! -d "$project" ]]; then
  echo "error: $project not found. Open Xcode and create the Vestige project first (see README.md)." >&2
  exit 1
fi

rm -rf "$dist_dir"
mkdir -p "$dist_dir"

echo "→ building Vestige.app (Release, unsigned)"
xcodebuild \
  -project "$project" \
  -scheme Vestige \
  -configuration Release \
  -derivedDataPath "$build_dir" \
  -destination 'platform=macOS' \
  CODE_SIGN_IDENTITY="-" \
  CODE_SIGNING_REQUIRED=NO \
  CODE_SIGNING_ALLOWED=NO \
  build \
  | sed -E '/^(CompileC|CompileSwift|Ld|CodeSign|ProcessInfoPlistFile|RegisterExecutionPolicyException|Touch|CreateBuildDirectory|MkDir|WriteAuxiliaryFile|GenerateDSYMFile|CopySwiftLibs) /d'

built_app="$build_dir/Build/Products/Release/Vestige.app"
if [[ ! -d "$built_app" ]]; then
  echo "error: build succeeded but $built_app is missing — scheme name may be wrong." >&2
  exit 1
fi

cp -R "$built_app" "$dist_dir/Vestige.app"
echo "✓ $dist_dir/Vestige.app"
echo "  Drag it to /Applications to install. Unsigned: first launch needs"
echo "  right-click → Open to bypass Gatekeeper."
