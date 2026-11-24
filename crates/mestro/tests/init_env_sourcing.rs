#![allow(clippy::unwrap_used, clippy::expect_used)]
// test harness helpers: a panic fails the test loudly, which is
// exactly what a test wants

//! The init scripts source env files: mestro's own (written whole, clean
//! LF: `state/mestro/mestro.env` with `TZ`, `state/mestro/squeezelite.env`
//! for squeezelite) and the user's `/data/mestro.env`, edited on Windows
//! (CRLF, sometimes a UTF-8 BOM). The part of each script between its
//! `env-source:begin` / `env-source:end` markers must still set (and
//! export) the variables — run here with the real script text, the
//! data paths pointed at a temp dir. The user's mestro.env is sourced
//! after mestro's file and wins; squeezelite reads mestro's file only.

use std::path::Path;
use std::process::Command;

const BOM: &str = "\u{feff}";

const SCRIPTS: &str = "../../image/board/mestro/rpi4/rootfs-overlay/etc/init.d";

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

/// run the block over `files` (paths below `/data/`), then print the variables
fn sourced_files(script: &str, files: &[(&str, &str)], vars: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("data")).unwrap();
    std::fs::create_dir_all(dir.path().join("tmp")).unwrap();
    for (file, content) in files {
        let path = dir.path().join("data").join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
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

/// run the block over one file
fn sourced(script: &str, file: &str, content: &str, vars: &str) -> String {
    sourced_files(script, &[(file, content)], vars)
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

const MESTRO_OWN_ENV: &str = "state/mestro/mestro.env";

#[test]
fn mestros_own_env_sets_and_exports_tz() {
    let out = sourced(
        "S50mestro",
        MESTRO_OWN_ENV,
        "TZ=Europe/Berlin\n",
        "TZ RUST_LOG",
    );
    assert!(out.contains("TZ=Europe/Berlin\n"), "{out}");
    assert!(out.contains("exported TZ=Europe/Berlin"), "{out}");
}

#[test]
fn the_users_mestro_env_wins_over_mestros_own() {
    let out = sourced_files(
        "S50mestro",
        &[
            (MESTRO_OWN_ENV, "TZ=Europe/Berlin\n"),
            ("mestro.env", "TZ=Asia/Tokyo\r\nRUST_LOG=debug\r\n"),
        ],
        "TZ RUST_LOG",
    );
    assert!(out.contains("TZ=Asia/Tokyo\n"), "{out}");
    assert!(out.contains("RUST_LOG=debug\n"), "{out}");
    assert!(out.contains("exported TZ=Asia/Tokyo"), "{out}");
}

#[test]
fn the_users_mestro_env_without_tz_leaves_mestros_zone() {
    let out = sourced_files(
        "S50mestro",
        &[
            (MESTRO_OWN_ENV, "TZ=Europe/Berlin\n"),
            ("mestro.env", "RUST_LOG=trace\n"),
        ],
        "TZ RUST_LOG",
    );
    assert!(out.contains("exported TZ=Europe/Berlin"), "{out}");
    assert!(out.contains("RUST_LOG=trace\n"), "{out}");
}

const SQUEEZELITE_ENV: &str = "state/mestro/squeezelite.env";
const SQUEEZELITE_VARS: &str = "OUTPUT NAME SERVER EXTRA";

/// the variables the start line sees, from mestro's file
fn squeezelite_sees(files: &[(&str, &str)]) -> String {
    sourced_files("S60squeezelite", files, SQUEEZELITE_VARS)
}

#[test]
fn squeezelite_takes_what_mestro_wrote() {
    let file = "# written by mestro\nOUTPUT=hw:CARD=S3\nNAME='Wohnzimmer HiFi'\nSERVER=192.168.3.13\nEXTRA='-r -Z 100'\n";
    let out = squeezelite_sees(&[(SQUEEZELITE_ENV, file)]);
    assert!(out.contains("OUTPUT=hw:CARD=S3\n"), "{out}");
    assert!(out.contains("NAME=Wohnzimmer HiFi\n"), "{out}");
    assert!(out.contains("SERVER=192.168.3.13\n"), "{out}");
    assert!(out.contains("EXTRA=-r -Z 100\n"), "{out}");
}

#[test]
fn squeezelite_without_mestros_file_has_nothing_set() {
    let out = squeezelite_sees(&[]);
    for var in ["OUTPUT", "NAME", "SERVER", "EXTRA"] {
        assert!(out.contains(&format!("{var}=\n")), "{out}");
    }
}

#[test]
fn squeezelite_ignores_any_user_file() {
    let out = squeezelite_sees(&[
        (SQUEEZELITE_ENV, "OUTPUT=hw:CARD=Auto\n"),
        ("squeezelite.env", "OUTPUT=hw:CARD=Mine\nNAME=HiFi\n"),
    ]);
    assert!(out.contains("OUTPUT=hw:CARD=Auto\n"), "{out}");
    assert!(out.contains("NAME=\n"), "{out}");
}

#[test]
fn squeezelite_gets_no_empty_output_or_name_flag() {
    // an empty -o/-n argument would make squeezelite fail or name
    // the player ""; the start line must drop the flag instead
    let script = std::fs::read_to_string(Path::new(SCRIPTS).join("S60squeezelite")).unwrap();
    assert!(script.contains("${OUTPUT:+-o \"$OUTPUT\"}"));
    assert!(script.contains("${NAME:+-n \"$NAME\"}"));
    assert!(script.contains("${SERVER:+-s \"$SERVER\"}"));
}
