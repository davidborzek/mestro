# Build container for the mestro image — the same one for local
# builds and CI. Debian trixie carries every host tool Buildroot
# needs; bun (the web app build) is pinned by version + sha256 and
# kept in sync with the dev shell and ci.yml.
#
# The container runs as the calling user (image/scripts/enter.sh),
# not as root: all files it writes keep the caller's ownership.

FROM debian:trixie

ARG BUN_VERSION=1.3.13
ARG BUN_SHA256=79c0771fa8b92c33aae41e15a0e0d307ea99d0e2f00317c71c6c53237a78e25a

RUN apt-get update && apt-get install -y --no-install-recommends \
		bc \
		bison \
		build-essential \
		bzip2 \
		ca-certificates \
		cpio \
		curl \
		file \
		findutils \
		flex \
		gawk \
		git \
		graphviz \
		help2man \
		jq \
		libncurses-dev \
		locales \
		make \
		patch \
		perl \
		pigz \
		python3 \
		python-is-python3 \
		rsync \
		sudo \
		texinfo \
		unzip \
		wget \
		zip \
	&& rm -rf /var/lib/apt/lists/*

# C.UTF-8: Buildroot warns about locale-related trouble otherwise
ENV LC_ALL=C.UTF-8

# bun, pinned like the CI jobs (renovate bumps the dev shell's bun)
RUN curl -fsSL -o /tmp/bun.zip \
		"https://github.com/oven-sh/bun/releases/download/bun-v${BUN_VERSION}/bun-linux-x64.zip" \
	&& echo "${BUN_SHA256}  /tmp/bun.zip" | sha256sum -c - \
	&& unzip -q /tmp/bun.zip -d /opt/bun \
	&& rm /tmp/bun.zip \
	&& ln -s /opt/bun/bun-linux-x64/bun /usr/local/bin/bun

WORKDIR /build
