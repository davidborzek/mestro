//! The Raspberry Pi firmware's boot config: `config.txt`, and the
//! `tryboot.txt` a trial boot reads — one `kernel=` line names the
//! kernel file the firmware loads. Only the `rpi` backend reads and
//! writes these.

use std::io;
use std::path::Path;

/// a boot config file: text with a `kernel=` line naming the kernel
/// it boots (`config.txt` and `tryboot.txt` alike)
#[derive(Debug, Clone)]
pub struct Config {
    text: String,
}

impl Config {
    pub fn read(path: &Path) -> io::Result<Self> {
        Ok(Self {
            text: std::fs::read_to_string(path)?,
        })
    }

    /// the kernel the config boots: the last `kernel=` line, CR
    /// stripped (the firmware tolerates Windows line endings)
    pub fn kernel(&self) -> String {
        self.text
            .lines()
            .rev()
            .find_map(|line| line.strip_prefix("kernel="))
            .map(|kernel| kernel.replace('\r', ""))
            .unwrap_or_default()
    }

    /// the config with every `kernel=` line pointing at `kernel`.
    /// Whole lines are replaced, so a stray CR cannot survive into
    /// the new kernel line — every other line stands as it was
    pub fn with_kernel(&self, kernel: &str) -> String {
        if self.text.is_empty() {
            return String::new();
        }
        let mut out = String::with_capacity(self.text.len() + kernel.len() + 8);
        for line in self.text.split_inclusive('\n') {
            let body = line.strip_suffix('\n').unwrap_or(line);
            if body.starts_with("kernel=") {
                out.push_str("kernel=");
                out.push_str(kernel);
            } else {
                out.push_str(body);
            }
            // whole, newline-terminated lines only — even a last
            // line the input left open
            out.push('\n');
        }
        out
    }

    /// the config a trial boot reads: the new kernel, with the SoC
    /// watchdog armed before the kernel starts — a kernel that
    /// hangs resets the board, and the boot after that one is back
    /// on the committed slot
    pub fn trial(&self, kernel: &str) -> String {
        format!(
            "{}\n[all]\nkernel_watchdog_timeout=60\n",
            self.with_kernel(kernel)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(text: &str) -> Config {
        Config { text: text.into() }
    }

    #[test]
    fn the_kernel_is_the_last_kernel_line_with_crs_stripped() {
        assert_eq!(config("arm_64bit=1\nkernel=Image.a\n").kernel(), "Image.a");
        assert_eq!(
            config("kernel=Image.b\nother=1\r\nkernel=Image.a\r\n").kernel(),
            "Image.a"
        );
        assert_eq!(config("kernel=Image.a extra\n").kernel(), "Image.a extra");
        assert_eq!(config("kernel=\n").kernel(), "");
        assert_eq!(config("no kernel here\n").kernel(), "");
    }

    #[test]
    fn with_kernel_replaces_whole_lines_and_terminates_the_last() {
        assert_eq!(
            config("arm_64bit=1\nkernel=Image.a\n").with_kernel("Image.b"),
            "arm_64bit=1\nkernel=Image.b\n"
        );
        // the CR of a Windows-terminated kernel line cannot survive
        assert_eq!(
            config("kernel=Image.a\r\nenable_uart=1\r\n").with_kernel("Image.b"),
            "kernel=Image.b\nenable_uart=1\r\n"
        );
        // a config whose last line has no newline gets one
        assert_eq!(
            config("kernel=Image.a").with_kernel("Image.b"),
            "kernel=Image.b\n"
        );
        // no kernel line: the text stands as it is
        assert_eq!(
            config("arm_64bit=1\n").with_kernel("Image.b"),
            "arm_64bit=1\n"
        );
    }

    #[test]
    fn the_trial_config_points_at_the_new_kernel_and_arms_the_watchdog() {
        assert_eq!(
            config("arm_64bit=1\nkernel=Image.a\n").trial("Image.b"),
            "arm_64bit=1\nkernel=Image.b\n\n[all]\nkernel_watchdog_timeout=60\n"
        );
        // a config without a kernel line boots whatever the firmware
        // defaults to — the trial only arms the watchdog
        assert_eq!(
            config("arm_64bit=1\n").trial("Image.b"),
            "arm_64bit=1\n\n[all]\nkernel_watchdog_timeout=60\n"
        );
    }
}
