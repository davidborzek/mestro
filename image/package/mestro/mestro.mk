# mestro: prebuilt aarch64 binary + fonts.
#
# mestro itself builds with cargo (slint pins a git rev and
# builds skia — too heavy to compile inside Buildroot). The
# package consumes the unpacked tree the staging script
# (image/scripts/build-mestro-aarch64.sh) links to image/bin:
#
#   bin/mestro
#   share/fonts/…
#
# MESTRO_DEPENDENCIES are exactly the libs the binary links
# (DT_NEEDED): libseat, libxkbcommon, libudev, libinput,
# libgbm, libfontconfig. libgbm is mesa3d with gbm (Config.in
# guards it via BR2_PACKAGE_HAS_LIBGBM). `make mestro-depends`
# on a fresh tree builds the toolchain + these sysroot libs,
# which is what the aarch64 cargo build in the .#pi shell
# links against.

MESTRO_VERSION = 1
MESTRO_SITE = $(BR2_PACKAGE_MESTRO_BIN_DIR)
MESTRO_SITE_METHOD = local
MESTRO_LICENSE = GPL-3.0+
MESTRO_LICENSE_FILES = LICENSE
MESTRO_DEPENDENCIES = seatd libxkbcommon udev libinput mesa3d fontconfig

define MESTRO_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0755 $(@D)/bin/mestro \
		$(TARGET_DIR)/usr/bin/mestro
	mkdir -p $(TARGET_DIR)/usr/share/fonts/mestro
	$(foreach f,$(wildcard $(@D)/share/fonts/*),\
		$(INSTALL) -m 0644 $(f) $(TARGET_DIR)/usr/share/fonts/mestro/$(notdir $(f))$(sep))
	$(INSTALL) -D -m 0644 $(BR2_EXTERNAL_MESTROIMAGE_PATH)/board/mestro/common/fonts.conf \
		$(TARGET_DIR)/etc/fonts.conf
endef

$(eval $(generic-package))
