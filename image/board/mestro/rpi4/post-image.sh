#!/bin/sh
# post-image: assemble mestro.img (boot FAT + data FAT) around the
# initramfs kernel. Adapted from buildroot's board/raspberrypi/post-image.sh
# (new layout: rpi-firmware installs into $(BINARIES_DIR)/rpi-firmware/).

set -e

BOARD_DIR="$(dirname "$0")"
GENIMAGE_CFG="${BINARIES_DIR}/genimage.cfg"
GENIMAGE_TMP="${BUILD_DIR}/genimage.tmp"

# the kernel boots from slot A (config.txt kernel=Image.a); the
# update artifact stays the plain Image
cp "${BINARIES_DIR}/Image" "${BINARIES_DIR}/Image.a"

# generate genimage config: the boot partition carries the whole
# firmware tree, the kernel image and the kernel's bcm2711 dtbs
{
	cat <<-'EOF'
		image boot.vfat {
			vfat {
				files = {
	EOF
	for i in "${BINARIES_DIR}"/rpi-firmware/* \
		"${BINARIES_DIR}"/Image.a \
		"${BINARIES_DIR}"/bcm2711-*.dtb; do
		printf '\t\t\t"%s",\n' "${i#${BINARIES_DIR}/}"
	done
	cat <<-'EOF'
		}
				}
				size = 256M
				}

				image data.vfat {
				vfat {
		label = "MESTRODATA"
		files = {
			"mestro-data/squeezelite.env",
			"mestro-data/wpa_supplicant.conf"
		}
				}
				empty = false
				size = 32M
				}

				image mestro.img {
				hdimage {
		partition-table-type = "dos"
				}

				# the whole system lives in the initramfs inside Image —
				# no root partition, the SD card is only read at boot
				partition 0 {
		partition-type = 0xC
		bootable = "true"
		image = "boot.vfat"
				}
				partition 1 {
		partition-type = 0xC
		image = "data.vfat"
				}
				}
	EOF
} >"${GENIMAGE_CFG}"

# pre-seed the data partition with the editable config templates so
# a headless setup works: flash, mount the card, fill in wifi creds
# (and squeezelite.env), boot — no first-boot PC-less bootstrap needed
mkdir -p "${BINARIES_DIR}/mestro-data"
cp "${TARGET_DIR}/etc/mestro-defaults/squeezelite.env" \
	"${BINARIES_DIR}/mestro-data/"
cp "${TARGET_DIR}/etc/mestro-defaults/wpa_supplicant.conf" \
	"${BINARIES_DIR}/mestro-data/"

rm -rf "${GENIMAGE_TMP}" "${BUILD_DIR}/genimage.root"
mkdir -p "${BUILD_DIR}/genimage.root"

# We don't rely on genimage to build a rootfs image (the whole
# system lives in the initramfs inside Image), so the rootpath is
# an empty directory.
genimage \
	--rootpath "${BUILD_DIR}/genimage.root" \
	--tmppath "${GENIMAGE_TMP}" \
	--inputpath "${BINARIES_DIR}" \
	--outputpath "${BINARIES_DIR}" \
	--config "${GENIMAGE_CFG}"

echo "mestro.img ready in ${BINARIES_DIR}"
