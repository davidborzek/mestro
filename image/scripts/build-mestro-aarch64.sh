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
# usage: image/scripts/build-mestro-aarch64.sh [--dev] [stage-dir]
#
# --dev builds [profile.pi] (no fat LTO, parallel, incremental):
# about a minute after a small change instead of ~5 — for test
# images. Release images keep [profile.release].
set -euo pipefail

cd "$(dirname "$0")/../.."
PROFILE=release
if [ "${1:-}" = "--dev" ]; then
	PROFILE=pi
	shift
fi
STAGE="${1:-$HOME/.cache/pi/mestro-image-bin}"

# the embedded web app first — build.rs embeds web/dist at
# compile time, so cargo would otherwise embed a stale one
nix develop -c bash -c \
	'cd web && { [ -d node_modules ] || bun install --frozen-lockfile; } && bun run build'

BR="${MESTRO_BUILDROOT:-$HOME/.cache/pi/buildroot/output}"
BR_SRC="${MESTRO_BUILDROOT_SRC:-$(cd "$BR/.." && pwd)}"
SYSROOT="$BR/host/aarch64-buildroot-linux-gnu/sysroot"
if [ ! -x "$BR/host/bin/aarch64-linux-gcc" ] ||
	[ ! -e "$SYSROOT/usr/lib/libseat.so.1" ]; then
	# with the default O=<src>/output buildroot keeps .config in the
	# source dir, otherwise inside the output dir
	if [ ! -f "$BR/.config" ] && [ ! -f "$BR_SRC/.config" ]; then
		echo "no configured Buildroot build at $BR — run first:" >&2
		echo "  make BR2_EXTERNAL=$PWD/image mestro_pi4_defconfig" >&2
		exit 1
	fi
	# toolchain + the libs mestro links (MESTRO_DEPENDENCIES in
	# image/package/mestro/mestro.mk)
	nix develop .#image -c \
		make -C "$BR_SRC" O="$BR" BR2_EXTERNAL="$PWD/image" mestro-depends
fi

nix develop .#pi -c cargo build --profile "${PROFILE}" \
	--no-default-features --features linuxkms,ota

# fonts: Inter (text) + Material Symbols (icons), both OFL — the
# files the image fonts.conf expects, from the nixpkgs revision
# flake.lock pins. Addressed by revision, not via getFlake ./. —
# that copies the whole working tree into the store on every run
NIXPKGS="github:NixOS/nixpkgs/$(sed -n '/"nixpkgs": {/,/"rev"/s/.*"rev": "\([0-9a-f]*\)".*/\1/p' flake.lock | head -n1)"
INTER="$(nix build --no-link --print-out-paths "${NIXPKGS}#inter")"
SYMBOLS="$(nix build --no-link --print-out-paths "${NIXPKGS}#material-symbols")"

rm -rf "${STAGE}"
mkdir -p "${STAGE}/bin" "${STAGE}/share/fonts"
install -m 0755 "target/pi/aarch64-unknown-linux-gnu/${PROFILE}/mestro" "${STAGE}/bin/mestro"
nix shell nixpkgs#pkgsCross.aarch64-multiplatform.buildPackages.binutils \
	-c aarch64-unknown-linux-gnu-strip "${STAGE}/bin/mestro"
install -m 0644 "${INTER}/share/fonts/truetype/InterVariable.ttf" \
	"${STAGE}/share/fonts/InterVariable.ttf"
install -m 0644 "${SYMBOLS}/share/fonts/truetype/MaterialSymbolsOutlined[FILL,GRAD,opsz,wght].ttf" \
	"${STAGE}/share/fonts/MaterialSymbolsOutlined.ttf"
install -m 0644 LICENSE "${STAGE}/LICENSE"

ln -sfn "${STAGE}" image/bin
echo "staged ${STAGE} → image/bin"
echo "next: cd <buildroot> && make mestro-dirclean && make"
