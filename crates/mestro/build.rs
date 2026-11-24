// A build script panics on purpose: a failed embed or a missing
// OUT_DIR is a build error, not a runtime condition to propagate.
// (mestro denies unwrap/expect everywhere else — see [lints] in
// Cargo.toml.)
#![allow(clippy::unwrap_used, clippy::expect_used)]

/// the Gregorian year of a unix timestamp (days → civil date, the
/// well-known days-from-civil inverse) — no date crate in a build script
fn year_of(epoch_seconds: i64) -> i64 {
    let days = epoch_seconds.div_euclid(86_400) + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_shifted = (5 * day_of_year + 2) / 153;
    let year = year_of_era + era * 400;
    // January and February belong to the next shifted year
    if month_shifted >= 10 {
        year + 1
    } else {
        year
    }
}

fn main() {
    // the image's timezone and diagnostics build-time variables
    // moved with their modules into mestro-os and mestro-remote: their
    // build scripts rerun on them

    // component-container is experimental and gates ComponentContainer
    // + component-factory, which the runtime VU skins (ui/vu/page.slint)
    // are built on. slint 1.19 names the features instead of accepting "1"
    std::env::set_var("SLINT_ENABLE_EXPERIMENTAL_FEATURES", "component-container");

    // track every file under ui/ and skins/: the slint compiler
    // embeds images at build time and manifest.rs include_str!s
    // the built-in manifests, so an SVG or TOML change must
    // recompile even when no .slint file changed
    fn walk(dir: &std::path::Path) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            println!("cargo:rerun-if-changed={}", path.display());
            if path.is_dir() {
                walk(&path);
            }
        }
    }
    walk(std::path::Path::new("ui"));
    walk(std::path::Path::new("skins"));

    // the same edition as SLINT_EDITION in src/vu/mod.rs
    slint_build::compile_with_config(
        "ui/mestro.slint",
        slint_build::CompilerConfiguration::new().with_edition((1, 19)),
    )
    .unwrap();

    // the short git hash next to the crate version (empty without
    // git, e.g. in the nix sandbox) — the crate lives under crates/,
    // the repository root is two levels up
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    let git = std::process::Command::new("git")
        .args(["describe", "--always", "--dirty"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default();
    println!("cargo:rustc-env=MESTRO_GIT={git}");

    // the build year: the standby clock and the topbar show "--:--"
    // while the system year is older (no RTC, NTP not answered yet —
    // src/standby/time.rs). The wall clock, deliberately not
    // SOURCE_DATE_EPOCH: the dev shell pins that to 1980, which would
    // make every date after the epoch look trustworthy. A cached build
    // script only ever leaves an older year behind, the permissive side
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
        .unwrap_or(0);
    std::fs::write(
        std::path::Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("build_year.rs"),
        format!("pub const BUILD_YEAR: i32 = {};\n", year_of(epoch)),
    )
    .unwrap();

    // the embedded web app (PWA) moved with the web endpoint into
    // mestro-remote: its build.rs embeds $MESTRO_WEB_DIST / web/dist
}
