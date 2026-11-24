# mestro image build — the Home Assistant OS pattern: the pinned
# Buildroot submodule does the building, this wrapper owns the
# defconfig, the web app and the order of the steps. Run it inside
# the build container (image/scripts/enter.sh) — see image/README.md.
#
#   image/scripts/enter.sh make mestro_pi4

BUILDDIR := $(CURDIR)
BUILDROOT := $(BUILDDIR)/buildroot
BUILDROOT_EXTERNAL := $(BUILDDIR)/image
DEFCONFIG_DIR := $(BUILDROOT_EXTERNAL)/configs

# Buildroot output stays in the tree (gitignored); the downloads
# and ccache live on the /cache volume, from the defconfig
ifneq ("$(origin O)", "command line")
O := $(BUILDDIR)/output
else
override O := $(BUILDDIR)/$(O)
endif

JOBS ?= 6

TARGETS := $(notdir $(patsubst %_defconfig,%,$(wildcard $(DEFCONFIG_DIR)/*_defconfig)))

.NOTPARALLEL: $(TARGETS)
.PHONY: $(TARGETS) $(TARGETS:%=%-config) web help

# anything unknown falls through to Buildroot
# (clean, distclean, menuconfig, mestro-dirclean, show-info, ...)
.DEFAULT:
	$(MAKE) -C $(BUILDROOT) O=$(O) BR2_EXTERNAL=$(BUILDROOT_EXTERNAL) "$@"

default: help

help:
	@echo "make <board>         configure and build the image (defconfig + build)"
	@echo "make <board>-config  only (re)write output/.config from the defconfig"
	@echo "make web             rebuild web/dist (the mestro crate embeds it)"
	@echo "make <anything else> falls through to Buildroot (clean, menuconfig, ...)"
	@echo ""
	@echo "boards: $(TARGETS)"

$(TARGETS:%=%-config):
	$(MAKE) -C $(BUILDROOT) O=$(O) BR2_EXTERNAL=$(BUILDROOT_EXTERNAL) "$(subst -config,,$@)_defconfig"

$(TARGETS): %: %-config web
	# the mestro package rsyncs this tree once per stamp: rebuild it,
	# so the image always contains what is on disk right now
	$(MAKE) -C $(BUILDROOT) O=$(O) BR2_EXTERNAL=$(BUILDROOT_EXTERNAL) -j$(JOBS) mestro-rebuild
	$(MAKE) -C $(BUILDROOT) O=$(O) BR2_EXTERNAL=$(BUILDROOT_EXTERNAL) -j$(JOBS)

web:
	cd web && bun install --frozen-lockfile && bun run build
