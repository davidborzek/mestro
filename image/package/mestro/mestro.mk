# mestro: the player binary, built inside Buildroot with the Rust
# toolchain it ships (host-rustc).
#
# The source is this repository itself (SITE_METHOD = local): the
# tree is rsynced into the build directory, then vendored there.
# Buildroot's own cargo vendoring runs in the download step only
# (support/download/cargo-post-process), which local sources never
# reach — so MESTRO_VENDOR_CRATES below does exactly what that
# script would have. The crate cache lives in BR_CARGO_HOME on the
# cache volume: only a build with a cold cache needs the network,
# every later one runs fully offline.
#
# The workspace is a virtual manifest, so the infra's stock
# `cargo install --path ./` cannot find a package — the binary is
# installed directly instead. Stripping is Buildroot's global
# target strip pass.

MESTRO_VERSION = 1
MESTRO_SITE = $(BR2_EXTERNAL_MESTROIMAGE_PATH)/..
MESTRO_SITE_METHOD = local
MESTRO_LICENSE = GPL-3.0+
MESTRO_LICENSE_FILES = LICENSE
MESTRO_DEPENDENCIES = \
	seatd libxkbcommon udev libinput mesa3d fontconfig \
	font-inter font-material-symbols

# the rsync must not carry the Buildroot submodule, build outputs
# or the node_modules symlink; web/dist is included on purpose
# (build.rs embeds it at compile time)
MESTRO_OVERRIDE_SRCDIR_RSYNC_EXCLUSIONS = \
	--exclude buildroot \
	--exclude target \
	--exclude output \
	--exclude web/node_modules \
	--exclude .direnv

# the rsync drops .git (RSYNC_VCS_EXCLUSIONS), so build.rs' `git
# describe` would bake an empty suffix into the version string.
# TARGET_MAKE_ENV pins GIT_DIR=. (Buildroot's isolation), which
# overrides it — point cargo at the real repository instead, so
# the image binary carries the same version suffix as an in-tree
# build.
#
# --absolute-git-dir: a worktree's .git is a gitfile itself
MESTRO_GITDIR = $(shell cd $(MESTRO_SITE) && git rev-parse --absolute-git-dir)

# pkg-cargo pins CARGO_PROFILE_RELEASE_* from BR2_OPTIMIZE_* /
# BR2_ENABLE_LTO (options meant for C code); restore the workspace
# release profile ([profile.release]: opt-level = "s", lto = true)
# so the binary matches the one the live-swap workflow builds
MESTRO_CARGO_ENV = \
	GIT_DIR=$(MESTRO_GITDIR) \
	GIT_WORK_TREE=$(MESTRO_SITE) \
	CARGO_PROFILE_RELEASE_OPT_LEVEL="s" \
	CARGO_PROFILE_RELEASE_LTO="true"

# the kiosk feature set (the feature flags live on the binary
# crate) — release profile, the vendored deps, Cargo.lock as-is
MESTRO_CARGO_BUILD_OPTS = \
	-p mestro \
	--no-default-features \
	--features linuxkms,ota,wifi-setup,audio-output,timezone

# same vendoring as support/download/cargo-post-process: crates
# land in BR_CARGO_HOME (DL_DIR, on the cache volume) and the
# build-dir VENDOR/ tree makes the stock `cargo build --offline`
# work
define MESTRO_VENDOR_CRATES
	@mkdir -p $(@D)/.cargo $(BR_CARGO_HOME)
	@cd $(@D) && flock $(BR_CARGO_HOME)/.br-lock env $(TARGET_MAKE_ENV) CARGO_HOME=$(BR_CARGO_HOME) \
		cargo vendor --locked VENDOR > .cargo/config.toml \
		|| { echo 'mestro: cargo vendor failed — with a cold crate cache it needs network access (crates.io, once); retry with the network up' >&2; false; }
endef
MESTRO_POST_RSYNC_HOOKS += MESTRO_VENDOR_CRATES

define MESTRO_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0755 \
		$(@D)/target/$(RUSTC_TARGET_NAME)/release/mestro \
		$(TARGET_DIR)/usr/bin/mestro
	$(INSTALL) -D -m 0644 \
		$(BR2_EXTERNAL_MESTROIMAGE_PATH)/board/mestro/common/fonts.conf \
		$(TARGET_DIR)/etc/fonts.conf
endef

$(eval $(cargo-package))
