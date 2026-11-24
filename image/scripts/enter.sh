#!/bin/sh
# Run a command in the image build container (podman locally, docker
# in CI): the repository is mounted at /build, the cache volume at
# /cache (downloads, ccache, the cargo crate cache, bun's cache —
# everything that survives `make clean`).
#
#   image/scripts/enter.sh make mestro_pi4   # build the image
#   image/scripts/enter.sh                   # shell in the container
#
# The container runs as the calling user; the cache dir defaults to
# ~/.cache/pi/mestro-cache and can be moved with MESTRO_CACHE_DIR.
set -eu

REPO_DIR=$(cd "$(dirname "$0")/../.." && pwd)
IMAGE_NAME=mestro-build:latest
CACHE_DIR="${MESTRO_CACHE_DIR:-$HOME/.cache/pi/mestro-cache}"

if command -v podman >/dev/null 2>&1; then
	ENGINE=podman
elif command -v docker >/dev/null 2>&1; then
	ENGINE=docker
else
	echo "error: neither podman nor docker is installed" >&2
	exit 1
fi

if [ "$(id -u)" -eq 0 ]; then
	echo "error: run this as a regular user, not root" >&2
	exit 1
fi

cd "$REPO_DIR"

if [ ! -f buildroot/Makefile ]; then
	# the pinned Buildroot submodule (bumps are manual, see
	# image/README.md)
	git submodule update --init buildroot
fi

"$ENGINE" build -t "$IMAGE_NAME" image/

mkdir -p "$CACHE_DIR"

TTY=
if [ -t 0 ]; then
	TTY=-t
fi

if [ "$ENGINE" = podman ]; then
	# rootless podman: the container sees the calling uid as itself
	"$ENGINE" run --rm -i $TTY \
		--userns=keep-id \
		-v "$REPO_DIR":/build \
		-v "$CACHE_DIR":/cache \
		-w /build \
		-e HOME=/cache/home \
		-e BUN_INSTALL_CACHE_DIR=/cache/bun \
		"$IMAGE_NAME" "${@:-bash}"
else
	# docker: run as the calling uid/gid so the bind mounts keep
	# their ownership
	"$ENGINE" run --rm -i $TTY \
		--user "$(id -u):$(id -g)" \
		-v "$REPO_DIR":/build \
		-v "$CACHE_DIR":/cache \
		-w /build \
		-e HOME=/cache/home \
		-e BUN_INSTALL_CACHE_DIR=/cache/bun \
		"$IMAGE_NAME" "${@:-bash}"
fi
