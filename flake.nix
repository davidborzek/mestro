{
  description = "mestro — now playing + control panel for the Music Assistant player";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs =
    {
      nixpkgs,
      ...
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAll =
        f:
        builtins.listToAttrs (
          map (system: {
            name = system;
            value = f (
              import nixpkgs {
                inherit system;
              }
            );
          }) systems
        );

      # the linuxkms + winit backends dlopen these at runtime; embed their
      # store paths via rpath so no LD_LIBRARY_PATH is needed anywhere
      rpathFlags =
        pkgs:
        nixpkgs.lib.concatStringsSep " " (
          map (p: "-C link-arg=-Wl,-rpath,${(p.lib or p)}/lib") (
            with pkgs;
            [
              wayland
              libxkbcommon
              libglvnd # provides libEGL.so.1 (mesa ships only the vendor driver)
              libgbm
              mesa
              fontconfig
              udev
              seatd
              libinput
            ]
          )
        );
    in
    {
      # nix is dev setup only — no packages output. The player
      # image is built with Buildroot (image/README.md), the
      # aarch64 binary with plain cargo in the .#pi shell.
      devShells = forAll (pkgs: {
        # buildroot host tooling for the kiosk image (see image/README.md)
        image = pkgs.mkShell {
          packages = with pkgs; [
            gnumake
            gcc
            which
            perl
            rsync
            unzip
            file
            bc
            bison
            flex
            (writeShellScriptBin "python3" "exec ${python3}/bin/python3 \"\$@\"")
            cpio
            zstd
            wget
          ];
          # NixOS has no /usr/include: cmake's find_path() (host-cmake
          # bootstrap checks for iconv.h) only searches system prefixes
          # plus this variable — point it at the glibc headers the gcc
          # wrapper already compiles against
          env.CMAKE_INCLUDE_PATH = "${pkgs.glibc.dev}/include";
        };
        # fast kiosk iteration: plain `cargo build` for aarch64,
        # linked against the Buildroot toolchain + sysroot — the
        # exact libs/glibc the image ships, no pkgsCross closure.
        # Needs a built Buildroot tree at $MESTRO_BUILDROOT
        # (default ~/.cache/pi/buildroot/output). See image/README.md
        pi =
          let
            crossRust = pkgs.pkgsCross.aarch64-multiplatform.buildPackages;
          in
          pkgs.mkShell {
            packages = [
              crossRust.rustc
              crossRust.cargo
            ];
            shellHook = ''
              BR="''${MESTRO_BUILDROOT:-$HOME/.cache/pi/buildroot/output}"
              if [ ! -x "$BR/host/bin/aarch64-linux-gcc" ]; then
                echo "pi shell: no Buildroot toolchain at $BR — build the image once or set MESTRO_BUILDROOT" >&2
              fi
              export CARGO_BUILD_TARGET=aarch64-unknown-linux-gnu
              export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="$BR/host/bin/aarch64-linux-gcc"
              export CC_aarch64_unknown_linux_gnu="$BR/host/bin/aarch64-linux-gcc"
              export AR_aarch64_unknown_linux_gnu="$BR/host/bin/aarch64-linux-ar"
              # Buildroot's pkg-config wrapper bakes in the sysroot
              export PKG_CONFIG="$BR/host/bin/pkg-config"
              export PKG_CONFIG_ALLOW_CROSS=1
              # separate target dir: no rebuild ping-pong with the host build
              export CARGO_TARGET_DIR="$PWD/target/pi"
            '';
          };
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
            rust-analyzer
            slint-viewer
            bun
          ];
          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = with pkgs; [
            fontconfig
            udev
            seatd
            libxkbcommon
            libinput
            libglvnd
            libgbm
            mesa
            wayland
          ];
          env.RUSTFLAGS = rpathFlags pkgs;
          env.FONTCONFIG_FILE = pkgs.writeText "mestro-fonts.conf" ''
            <?xml version="1.0"?>
            <!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">
            <fontconfig>
              <dir>${pkgs.noto-fonts}</dir>
              <dir>${pkgs.noto-fonts-cjk-sans}</dir>
              <dir>${pkgs.dejavu_fonts}</dir>
              <dir>${pkgs.material-symbols}</dir>
              <cachedir>/tmp/fontconfig-cache</cachedir>
            </fontconfig>
          '';
        };
      });
    };
}
