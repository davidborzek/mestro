#!/usr/bin/env bash
# Stage the mestro payload the Buildroot package consumes
# (image/package/mestro): bin/mestro + share/fonts/, unpacked,
# behind the image/bin symlink.
#
# The binary is a plain `cargo build` for aarch64 in the flake's
# .#pi shell, linked against the Buildroot toolchain + sysroot —
# the exact glibc and libs the image ships. On a fresh Buildroot
# tree the script bootstraps toolchain + the binary's runtime
# libs with `make mestro-depends` first ($MESTRO_BUILDROOT,
# default ~/.cache/pi/buildroot/output).
#
# usage: image/scripts/build-mestro-aarch64.sh [stage-dir]
set -euo pipefail

cd "$(dirname "$0")/../.."
STAGE="${1:-$HOME/.cache/pi/mestro-image-bin}"

nix develop .#pi -c cargo build --release \
	--no-default-features --features linuxkms

# fonts: Inter (text) + Material Symbols (icons), both OFL — the
# files the image fonts.conf expects
FONTS="$(nix build --no-link --print-out-paths --impure --expr '
  let pkgs = (builtins.getFlake (toString ./.)).inputs.nixpkgs.legacyPackages.${builtins.currentSystem};
  in pkgs.symlinkJoin { name = "mestro-image-fonts"; paths = [ pkgs.inter pkgs.material-symbols ]; }')"

rm -rf "${STAGE}"
mkdir -p "${STAGE}/bin" "${STAGE}/share/fonts"
install -m 0755 target/pi/aarch64-unknown-linux-gnu/release/mestro "${STAGE}/bin/mestro"
nix shell nixpkgs#pkgsCross.aarch64-multiplatform.buildPackages.binutils \
	-c aarch64-unknown-linux-gnu-strip "${STAGE}/bin/mestro"
install -m 0644 "${FONTS}/share/fonts/truetype/InterVariable.ttf" \
	"${STAGE}/share/fonts/InterVariable.ttf"
install -m 0644 "${FONTS}/share/fonts/truetype/MaterialSymbolsOutlined[FILL,GRAD,opsz,wght].ttf" \
	"${STAGE}/share/fonts/MaterialSymbolsOutlined.ttf"
install -m 0644 LICENSE "${STAGE}/LICENSE"

ln -sfn "${STAGE}" image/bin
echo "staged ${STAGE} → image/bin"
echo "next: cd <buildroot> && make mestro-dirclean && make"
