#![allow(clippy::unwrap_used, clippy::expect_used)]
// test harness helpers: a panic fails the test loudly, which is
// exactly what a test wants

//! `unsafe_code` is denied crate-wide; the only legal escape is the
//! local `#[allow(unsafe_code)]` of the audited site — the raw
//! `reboot(2)` of the Raspberry Pi firmware backend (`reboot`,
//! src/rpi.rs, behind the `rpi` feature). This test scans every
//! source file under src/ for the `unsafe` keyword and for
//! `allow(unsafe_code)` (comments aside) and pins that one function
//! with its exact number of `unsafe` mentions — the allow attribute
//! and the block count together (2 each) — so a second site, or a
//! second block in the pinned function, fails CI until it is
//! audited and pinned here.
//!
//! Without the `rpi` feature the backend files are not part of the
//! build; this build must simply have no unsafe site at all.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// the audited site of the rpi backend: file (from the crate root),
/// function, and how often `unsafe` is mentioned in it
#[cfg(feature = "rpi")]
const PINNED: &[(&str, &str, usize)] = &[("src/rpi/reboot.rs", "reboot", 2)];

/// without the backend feature nothing unsafe is compiled in
#[cfg(not(feature = "rpi"))]
const PINNED: &[(&str, &str, usize)] = &[];

/// every .rs file under `dir`, recursively
fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).expect("read the src dir");
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// the name in a `fn name(…)` line
fn fn_name(line: &str) -> &str {
    let rest = line.split_once("fn ").expect("a fn line").1;
    rest.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .find(|name| !name.is_empty())
        .unwrap_or_default()
}

/// every (file, enclosing fn) that mentions `unsafe` or
/// `allow(unsafe_code)` outside a comment, with the number of
/// mentions, sorted. The enclosing fn is the nearest fn line at or
/// above the mention: an `unsafe fn name` line names itself
fn unsafe_sites(root: &Path) -> Vec<(String, String, usize)> {
    let mut files = Vec::new();
    sources(&root.join("src"), &mut files);
    let mut counts: BTreeMap<(String, String), usize> = BTreeMap::new();
    for file in files {
        let text = std::fs::read_to_string(&file).expect("read the source file");
        let lines: Vec<&str> = text.lines().collect();
        let relative = file
            .strip_prefix(root)
            .expect("the file is under the crate root")
            .display()
            .to_string();
        for (index, line) in lines.iter().enumerate() {
            let mentions = line.matches("unsafe").count();
            if is_comment(line) || mentions == 0 {
                continue;
            }
            let enclosing = lines[..=index]
                .iter()
                .rev()
                .find(|line| !is_comment(line) && line.contains("fn "))
                .expect("unsafe outside any function");
            *counts
                .entry((relative.clone(), fn_name(enclosing).to_string()))
                .or_default() += mentions;
        }
    }
    counts
        .into_iter()
        .map(|((file, function), count)| (file, function, count))
        .collect()
}

/// the backend files, which are behind the `rpi` feature: without
/// it they are not part of the build and not audited here
#[cfg(not(feature = "rpi"))]
fn part_of_this_build(file: &str) -> bool {
    !file.starts_with("src/rpi/")
}

#[cfg(feature = "rpi")]
fn part_of_this_build(_file: &str) -> bool {
    true
}

#[test]
fn exactly_the_audited_unsafe_sites_exist() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let sites: Vec<(String, String, usize)> = unsafe_sites(root)
        .into_iter()
        .filter(|(file, _, _)| part_of_this_build(file))
        .collect();
    let pinned: Vec<(String, String, usize)> = PINNED
        .iter()
        .map(|(file, function, count)| (file.to_string(), function.to_string(), *count))
        .collect();
    assert_eq!(
        sites, pinned,
        "the audited unsafe sites changed: audit the new one and \
         pin it here (or remove it)"
    );
}

/// a fake crate whose src/lib.rs holds `lib_rs`
fn fake_crate(lib_rs: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("lib.rs"), lib_rs).unwrap();
    dir
}

/// the audited shape: an allow attribute and one unsafe block
const REBOOT_FN: &str = "pub fn reboot() {\n    #[allow(unsafe_code)]\n    let _ = unsafe { 1 };\n";

#[test]
fn the_scan_sees_an_unsafe_block_in_a_fresh_file() {
    // the scanner itself must not be blind: a fake crate with an
    // unaudited site is reported
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("extra.rs"),
        "// unsafe in a comment is fine\nfn sneaky() {\n    let _ = unsafe { 1 };\n}\n",
    )
    .unwrap();
    assert_eq!(
        unsafe_sites(dir.path()),
        [("src/extra.rs".to_string(), "sneaky".to_string(), 1)]
    );
}

#[test]
fn an_unsafe_fn_after_a_pinned_fn_is_its_own_site() {
    // the `unsafe fn` line names itself; only its attribute line
    // above (which has no fn of its own) lands on the fn before
    let dir = fake_crate(&format!(
        "{REBOOT_FN}}}\n\n#[allow(unsafe_code)]\nunsafe fn second_site() {{}}\n"
    ));
    assert_eq!(
        unsafe_sites(dir.path()),
        [
            ("src/lib.rs".to_string(), "reboot".to_string(), 3),
            ("src/lib.rs".to_string(), "second_site".to_string(), 1),
        ]
    );
}

#[test]
fn a_second_unsafe_block_in_a_pinned_fn_changes_its_count() {
    let dir = fake_crate(&format!(
        "{REBOOT_FN}    #[allow(unsafe_code)]\n    let _extra = unsafe {{ 2 }};\n}}\n"
    ));
    assert_eq!(
        unsafe_sites(dir.path()),
        [("src/lib.rs".to_string(), "reboot".to_string(), 4)]
    );
}
