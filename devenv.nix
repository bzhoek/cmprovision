{ pkgs, ... }:

{
  # nix-phps builds PHP 8.1 from source without its binary cache.
  cachix.pull = [ "fossar" ];

  packages = [
    pkgs.sqlite
    pkgs.net-snmp
  ];

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
