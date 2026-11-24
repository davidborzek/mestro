# Material Symbols: the icon font of the kiosk UI (see
# board/mestro/common/fonts.conf). The version is the
# material-design-icons commit the font is fetched from — Google
# publishes the variable TTFs per commit, not as releases. Only the
# outlined variable TTF ships: that is the one file the image's
# fontconfig config lists.

FONT_MATERIAL_SYMBOLS_VERSION = 5a4e1b7fd26f11ce3d2abf7d7fcd13274f874e6c
# the upstream file name is MaterialSymbolsOutlined[FILL,GRAD,opsz,wght].ttf
# — the brackets arrive percent-encoded in the URL
FONT_MATERIAL_SYMBOLS_SOURCE = MaterialSymbolsOutlined%5BFILL,GRAD,opsz,wght%5D.ttf
FONT_MATERIAL_SYMBOLS_SITE = https://raw.githubusercontent.com/google/material-design-icons/$(FONT_MATERIAL_SYMBOLS_VERSION)/variablefont
FONT_MATERIAL_SYMBOLS_LICENSE = Apache-2.0

# a raw file, not an archive: copy it into the build dir
define FONT_MATERIAL_SYMBOLS_EXTRACT_CMDS
	$(INSTALL) -m 0644 $(FONT_MATERIAL_SYMBOLS_DL_DIR)/$(FONT_MATERIAL_SYMBOLS_SOURCE) $(@D)/
endef

define FONT_MATERIAL_SYMBOLS_INSTALL_TARGET_CMDS
	mkdir -p $(TARGET_DIR)/usr/share/fonts/mestro
	$(INSTALL) -m 0644 $(@D)/$(FONT_MATERIAL_SYMBOLS_SOURCE) \
		$(TARGET_DIR)/usr/share/fonts/mestro/MaterialSymbolsOutlined.ttf
endef

$(eval $(generic-package))
