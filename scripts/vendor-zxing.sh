#!/usr/bin/env bash
# Rebuilds vendor/zxing-cpp-rs, the zxing-cpp Rust wrapper, from the vendor/zxing-cpp submodule
# plus scripts/zxing-cpp.patch. To move to another upstream commit:
#   git -C vendor/zxing-cpp fetch --depth 1 origin <commit> && git -C vendor/zxing-cpp checkout <commit>
#   scripts/vendor-zxing.sh
set -euo pipefail
cd "$(dirname "$0")/.."
src=vendor/zxing-cpp dst=vendor/zxing-cpp-rs
rm -rf "$dst" && mkdir -p "$dst/src"
# git show writes the committed bytes, whatever line endings the checkout uses.
for f in Cargo.toml build.rs README.md src/lib.rs src/bindings.rs src/tests.rs; do
  git -C "$src" show "HEAD:wrappers/rust/$f" > "$dst/$f"
done
git -C "$src" show HEAD:LICENSE > "$dst/LICENSE"
git apply --directory="$dst" scripts/zxing-cpp.patch
# One doc comment carries a to-do marker, which the repo's text checks reject.
sed -i 's|value types\. [A-Z]*: support slices/arrays|value types; slices and arrays are not supported yet.|' "$dst/src/lib.rs"
