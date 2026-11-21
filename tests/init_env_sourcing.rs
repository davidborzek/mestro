#![allow(clippy::unwrap_used, clippy::expect_used)]
// test harness helpers: a panic fails the test loudly, which is
// exactly what a test wants

//! The init scripts source the user-editable env files
//! (`/data/mestro.env`, `/data/squeezelite.env`). Those are edited on
//! Windows (CRLF, sometimes a UTF-8 BOM) and rewritten by mestro,
//! which keeps both. The part of each script between its
//! `env-source:begin` / `env-source:end` markers must still set (and
//! export) the variables — run here with the real script text, the
//! data paths pointed at a temp dir.

use std::path::Path;
use std::process::Command;

const BOM: &str = "\u{feff}";

const SCRIPTS: &str = "image/board/mestro/rpi4/rootfs-overlay/etc/init.d";

/// the marked block of `script`, `/data/` and `/tmp/` redirected into subdirectories of `dir`
fn env_source_block(script: &str, dir: &Path) -> String {
    let text = std::fs::read_to_string(Path::new(SCRIPTS).join(script)).unwrap();
    let mut block = String::new();
    let mut inside = false;
    for line in text.lines() {
        if line.contains("env-source:end") {
            inside = false;
        }
        if inside {
            block.push_str(line);
            block.push('\n');
        }
        if line.contains("env-source:begin") {
            inside = true;
        }
    }
    assert!(!block.is_empty(), "{script} has no env-source block");
    let dir = dir.display().to_string();
    // placeholders first: the temp dir itself lives under /tmp
    block
        .replace("/data/", "@DATA@/")
        .replace("/tmp/", "@TMP@/")
        .replace("@DATA@", &format!("{dir}/data"))
        .replace("@TMP@", &format!("{dir}/tmp"))
}

/// run the block, then print the variables
fn sourced(script: &str, file: &str, content: &str, vars: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("data")).unwrap();
    std::fs::create_dir_all(dir.path().join("tmp")).unwrap();
    std::fs::write(dir.path().join("data").join(file), content).unwrap();
    let program = format!(
        "{}\nfor v in {vars}; do eval \"printf '%s=%s\\n' $v \\\"\\${{$v:-}}\\\"\"; done\nenv | grep '^TZ=' | sed 's/^/exported /'",
        env_source_block(script, dir.path())
    );
    // `SHELL_UNDER_TEST` runs the same blocks under busybox sh
    let shell = std::env::var("SHELL_UNDER_TEST").unwrap_or_else(|_| "sh".into());
    let out = Command::new(shell)
        .args(["-c", &program])
        .env_remove("TZ")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn mestro_env_with_a_bom_and_crlf_still_sets_and_exports_tz() {
    let content = format!("{BOM}TZ=Europe/London\r\nRUST_LOG=debug\r\n");
    let out = sourced("S50mestro", "mestro.env", &content, "TZ RUST_LOG");
    assert!(out.contains("TZ=Europe/London\n"), "{out}");
    assert!(out.contains("RUST_LOG=debug\n"), "{out}");
    assert!(out.contains("exported TZ=Europe/London"), "{out}");
}

#[test]
fn mestro_env_without_a_bom_is_unchanged() {
    let out = sourced("S50mestro", "mestro.env", "TZ=UTC\n", "TZ");
    assert!(out.contains("exported TZ=UTC"), "{out}");
}

#[test]
fn squeezelite_env_with_a_bom_and_crlf_still_sets_output() {
    let content = format!("{BOM}OUTPUT=hw:CARD=S3\r\nNAME=mestro\r\n");
    let out = sourced("S60squeezelite", "squeezelite.env", &content, "OUTPUT NAME");
    assert!(out.contains("OUTPUT=hw:CARD=S3\n"), "{out}");
    assert!(out.contains("NAME=mestro\n"), "{out}");
}

#[test]
fn a_bom_only_goes_from_the_first_line() {
    let content = format!("A=1\nB=2{BOM}\n");
    let out = sourced("S60squeezelite", "squeezelite.env", &content, "A B");
    assert!(out.contains("A=1\n"), "{out}");
    // the stray mark mid-file is data, not stripped
    assert!(out.contains(&format!("B=2{BOM}\n")), "{out}");
}
