fn main() {
    // experimental gates ComponentContainer + component-factory,
    // which the runtime VU skins (ui/vu/page.slint) are built on
    std::env::set_var("SLINT_ENABLE_EXPERIMENTAL_FEATURES", "1");

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

    slint_build::compile("ui/mestro.slint").unwrap();

    // ── the embedded setup-wizard SPA ──
    // pattern: the build/release process runs the Vite build first
    // (`bun run build` in web/), this only embeds the single file.
    // $MESTRO_WEB_DIST points at a prebuilt index.html (a release
    // pipeline may pass its own). The dev loop uses `bun run dev` (Vite + ws proxy) and never rebuilds
    // this.
    println!("cargo:rerun-if-env-changed=MESTRO_WEB_DIST");
    println!("cargo:rerun-if-changed=web/dist/index.html");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"))
        .join("mestro-web.html");
    let source = std::env::var("MESTRO_WEB_DIST")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("web/dist/index.html"));
    match std::fs::copy(&source, &out) {
        Ok(_) => {}
        Err(e) => panic!(
            "cannot embed the setup wizard SPA from {}: {e}\n\
             build it first: `nix shell nixpkgs#bun -c bun install && bun run build` (in web/)\n\
             or set MESTRO_WEB_DIST to a prebuilt index.html",
            source.display()
        ),
    }
}
