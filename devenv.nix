{ pkgs, ... }:

{
  # nix-phps builds PHP 8.1 from source without its binary cache.
  cachix.pull = [ "fossar" ];

  packages = [
    pkgs.sqlite
    pkgs.net-snmp
    pkgs.tailwindcss_4
  ];

  # The built stylesheet is committed and embedded, so cargo builds never need Tailwind.
  scripts.css.exec = ''
    tailwindcss --input "$DEVENV_ROOT/assets/app.css" --output "$DEVENV_ROOT/assets/dist/app.css" --minify "$@"
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
  };
}
