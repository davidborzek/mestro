// rpi-tryboot — reboot with the Raspberry Pi firmware's one-shot
// tryboot flag: the next boot reads tryboot.txt instead of
// config.txt, any boot after that is back on config.txt. busybox
// reboot cannot pass the "0 tryboot" argument to the kernel.

#include <linux/reboot.h>
#include <stdio.h>
#include <sys/syscall.h>
#include <unistd.h>

int main(void)
{
	sync();
	syscall(SYS_reboot, LINUX_REBOOT_MAGIC1, LINUX_REBOOT_MAGIC2,
		LINUX_REBOOT_CMD_RESTART2, "0 tryboot");
	perror("rpi-tryboot: reboot");
	return 1;
}
