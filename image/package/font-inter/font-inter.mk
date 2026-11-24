# Inter: the text font of the kiosk UI (see board/mestro/common/
# fonts.conf). Only the variable TTF ships — that is the one file
# the image's fontconfig config lists.

FONT_INTER_VERSION = 4.1
FONT_INTER_SOURCE = Inter-$(FONT_INTER_VERSION).zip
FONT_INTER_SITE = https://github.com/rsms/inter/releases/download/v$(FONT_INTER_VERSION)
FONT_INTER_LICENSE = OFL-1.1
FONT_INTER_LICENSE_FILES = LICENSE.txt

# the release zip has no top-level directory
define FONT_INTER_EXTRACT_CMDS
	$(UNZIP) -d $(@D) $(FONT_INTER_DL_DIR)/$(FONT_INTER_SOURCE)
endef

define FONT_INTER_INSTALL_TARGET_CMDS
	mkdir -p $(TARGET_DIR)/usr/share/fonts/mestro
	$(INSTALL) -m 0644 $(@D)/InterVariable.ttf \
		$(TARGET_DIR)/usr/share/fonts/mestro/InterVariable.ttf
endef

$(eval $(generic-package))
