#!/bin/sh
# mestro-upgrade — A/B system upgrade for the initramfs kiosk. The
# whole system is one kernel file (the rootfs is the initramfs
# inside it), so the two slots are two files on the boot partition:
# Image.a and Image.b, config.txt `kernel=` names the active one.
#
#   mestro-upgrade [--reboot] <new-Image | -> [sha256]
#       write the inactive slot (from a file or stdin, streamed —
#       never buffered in RAM) and prepare a trial boot of it;
#       --reboot starts the trial right away
#   mestro-upgrade --commit
#       make the trial slot permanent (S99ota calls this once the
#       trial boot came up healthy)
#   mestro-upgrade --abandon
#       drop a pending trial that never committed (S99ota, on a
#       normal boot)
#   mestro-upgrade --status
#
# Trial boot: tryboot.txt is config.txt pointing at the new slot,
# and rpi-tryboot reboots with the firmware's one-shot tryboot flag
# — that one boot reads tryboot.txt. Without a commit (a crash, a
# hang and a power cycle, a failed health check) the next boot is
# back on config.txt: the old slot.
set -e

BOOT=/boot
REBOOT=
MOUNTED_HERE=

mount_boot() {
	grep -q " $BOOT " /proc/mounts && return 0
	mkdir -p "$BOOT"
	for d in /dev/mmcblk0p1 /dev/nvme0n1p1 /dev/sda1; do
		mount -t vfat "$d" "$BOOT" 2>/dev/null && {
			MOUNTED_HERE=1
			return 0
		}
	done
	echo "cannot mount the boot partition at $BOOT" >&2
	exit 1
}

release_boot() {
	sync
	[ -n "$MOUNTED_HERE" ] && umount "$BOOT" 2>/dev/null
	MOUNTED_HERE=
}

kernel_of() {
	sed -n 's/^kernel=//p' "$1" | tail -n1 | tr -d '\r'
}

# the firmware's one-shot flag for this boot (device tree, 32-bit BE)
tryboot_active() {
	[ "$(hexdump -v -e '4/1 "%02x"' /proc/device-tree/chosen/bootloader/tryboot 2>/dev/null)" = "00000001" ]
}

status() {
	mount_boot
	echo "active:  $(kernel_of "$BOOT/config.txt")"
	if [ -f "$BOOT/tryboot.txt" ]; then
		echo "pending: $(kernel_of "$BOOT/tryboot.txt")"
	fi
	tryboot_active && echo "this boot: trial" || echo "this boot: normal"
	release_boot
}

commit() {
	mount_boot
	if ! tryboot_active || [ ! -f "$BOOT/tryboot.txt" ]; then
		echo "no trial boot to commit"
		release_boot
		return 0
	fi
	NEW=$(kernel_of "$BOOT/tryboot.txt")
	OLD=$(kernel_of "$BOOT/config.txt")
	sed "s/^kernel=.*/kernel=$NEW/" "$BOOT/config.txt" >"$BOOT/config.txt.new"
	sync
	# one FAT rename: config.txt is either the old or the new one
	mv "$BOOT/config.txt.new" "$BOOT/config.txt"
	rm -f "$BOOT/tryboot.txt"
	# a pre-A/B card booted plain `Image`: it becomes slot A, the
	# fallback of this commit
	if [ "$OLD" = "Image" ] && [ ! -e "$BOOT/Image.a" ]; then
		mv "$BOOT/Image" "$BOOT/Image.a"
	fi
	release_boot
	echo "committed: $NEW"
}

abandon() {
	mount_boot
	if [ -f "$BOOT/tryboot.txt" ]; then
		echo "trial of $(kernel_of "$BOOT/tryboot.txt") never committed, dropped"
		rm -f "$BOOT/tryboot.txt"
	fi
	release_boot
}

install() {
	SRC="$1"
	WANT_SHA="$2"
	[ "$SRC" = "-" ] || [ -f "$SRC" ] || {
		echo "no such file: $SRC" >&2
		exit 2
	}
	mount_boot

	CUR=$(kernel_of "$BOOT/config.txt")
	case "$CUR" in
	Image.a) SLOT=Image.b ;;
	*) SLOT=Image.a ;;
	esac
	# a pre-A/B card: plain Image is running, Image.old was its
	# manual fallback — make room for the slot, Image stays the
	# fallback until the commit renames it
	if [ "$CUR" = "Image" ]; then
		SLOT=Image.b
		rm -f "$BOOT/Image.old"
	fi
	[ "$SLOT" != "$CUR" ] || {
		echo "refusing to overwrite the running slot $CUR" >&2
		exit 1
	}

	rm -f "$BOOT/tryboot.txt" "$BOOT/$SLOT"
	echo "writing $SLOT (running: $CUR) …"
	if [ "$SRC" = "-" ]; then
		GOT_SHA=$(tee "$BOOT/$SLOT" | sha256sum | cut -d' ' -f1)
	else
		GOT_SHA=$(tee "$BOOT/$SLOT" <"$SRC" | sha256sum | cut -d' ' -f1)
	fi
	sync
	if [ -n "$WANT_SHA" ] && [ "$GOT_SHA" != "$WANT_SHA" ]; then
		rm -f "$BOOT/$SLOT"
		echo "sha256 mismatch: got $GOT_SHA, want $WANT_SHA" >&2
		exit 1
	fi
	# read back what landed on the card, not the page cache
	echo 3 >/proc/sys/vm/drop_caches
	ON_CARD=$(sha256sum "$BOOT/$SLOT" | cut -d' ' -f1)
	if [ "$ON_CARD" != "$GOT_SHA" ]; then
		rm -f "$BOOT/$SLOT"
		echo "corrupted write on the boot partition" >&2
		exit 1
	fi
	[ -n "$WANT_SHA" ] || echo "no sha256 given; written: $GOT_SHA"

	# the trial arms the SoC watchdog before the kernel starts: a
	# kernel that hangs resets the board, and that boot reads
	# config.txt again (S99ota feeds it, then turns it off)
	{
		sed "s/^kernel=.*/kernel=$SLOT/" "$BOOT/config.txt"
		printf '\n[all]\nkernel_watchdog_timeout=60\n'
	} >"$BOOT/tryboot.txt"
	release_boot
	echo "ready: $SLOT ($GOT_SHA)"

	if [ -n "$REBOOT" ]; then
		echo "trial boot …"
		exec mestro-reboot --tryboot
	fi
	echo "trial boot with: mestro-reboot --tryboot"
}

case "$1" in
--commit)
	commit
	exit 0
	;;
--status)
	status
	exit 0
	;;
--abandon)
	abandon
	exit 0
	;;
--reboot)
	REBOOT=1
	shift
	;;
esac
[ -n "$1" ] || {
	echo "usage: mestro-upgrade [--reboot] <new-Image | -> [sha256] | --commit | --abandon | --status" >&2
	exit 2
}
install "$1" "$2"
