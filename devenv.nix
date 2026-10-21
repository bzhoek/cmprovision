{ pkgs, ... }:

{
  # nix-phps builds PHP 8.1 from source without its binary cache.
  cachix.pull = [ "fossar" ];

  packages = [
    pkgs.sqlite
    pkgs.net-snmp
    pkgs.tailwindcss_4
    # Cross-compiling for Raspberry Pi OS without Docker.
    pkgs.zig
    pkgs.cargo-zigbuild
    pkgs.shellcheck
  ];

  # The built stylesheet is committed and embedded, so cargo builds never need Tailwind.
  scripts.css.exec = ''
    tailwindcss --input "$DEVENV_ROOT/assets/app.css" --output "$DEVENV_ROOT/assets/dist/app.css" --minify "$@"
  '';

  # Builds target/debian/cmprovision4_<version>_<arch>.deb for arm64 (default) or armhf.
  # cargo-deb is installed with cargo because nixpkgs marks it broken on macOS.
  scripts.deb.exec = ''
    set -e
    case "''${1:-arm64}" in
      arm64) target=aarch64-unknown-linux-gnu ;;
      armhf) target=armv7-unknown-linux-gnueabihf ;;
      *) echo "usage: deb [arm64|armhf]" >&2; exit 2 ;;
    esac
    tools="$DEVENV_STATE/cargo-deb"
    [ -x "$tools/bin/cargo-deb" ] || cargo install --locked --root "$tools" cargo-deb
    # glibc 2.31 is Debian Bullseye, the oldest Raspberry Pi OS this supports.
    cargo zigbuild --manifest-path "$DEVENV_ROOT/Cargo.toml" --release --target "$target.2.31"
    "$tools/bin/cargo-deb" --manifest-path "$DEVENV_ROOT/Cargo.toml" --target "$target" --no-build --no-strip
  '';

  # The PHP toolchain only exists to run the legacy app for capturing golden outputs.
  languages.php = {
    enable = true;
    version = "8.1";
    extensions = [ "sqlite3" "pdo_sqlite" "bcmath" "mbstring" "zip" "snmp" ];
  };

  languages.rust = {
    enable = true;
    channel = "stable";
    # 64-bit and 32-bit Raspberry Pi OS on a Pi 4.
    targets = [ "aarch64-unknown-linux-gnu" "armv7-unknown-linux-gnueabihf" ];
  };
}
