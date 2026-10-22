# Rust migration plan

Rewrite of cmprovision from Laravel 8 / Livewire to a single Rust binary, as a hard fork at
[bzhoek/cmprovision](https://github.com/bzhoek/cmprovision). Branch `rust-rewrite`, PR
[#1](https://github.com/bzhoek/cmprovision/pull/1). `upstream` is raspberrypi/cmprovision; PRs
go to the fork only (`gh pr create --repo bzhoek/cmprovision --base main`).

## Decisions

- Hard fork; no upstreaming.
- Users re-enroll two-factor authentication; Laravel-encrypted secrets are cleared on migration.
- No password reset, browser session management or account self-delete.
- Server-rendered UI (askama templates, plain forms, minimal vanilla JS), no SPA.
- Existing installations upgrade in place: same package name `cmprovision4`, same data
  directory `/var/lib/cmprovision`, Laravel's SQLite schema adopted unchanged.
- Device (`/scriptexecute`) and REST API contracts stay byte-compatible, verified against
  responses captured from the PHP app.

## Stack

axum, tokio, sqlx (SQLite), askama, tower-sessions with a SQLite store, bcrypt, totp-rs,
snmp2, suppaftp, reqwest with rustls/ring, pure-Rust gzip/bzip2/xz decoders. Tailwind v4
built by devenv and embedded. Cross-compiled with cargo-zigbuild (glibc 2.31), packaged
with cargo-deb.

## Phases

| # | Phase | Status | Commit |
|---|-------|--------|--------|
| 1 | Capture golden outputs of the PHP app (`tests/Golden`, `tests/fixtures`) | done | `70342ec` |
| 2 | Rust skeleton, Laravel-compatible schema, `serve`/`migrate` | done | `d558385` |
| 3 | Domain logic: EEPROM, labels, SNMP switch, dnsmasq, firmware | done | `803a921` |
| 4 | `/scriptexecute` with golden parity tests | done | `63f2155` |
| 5 | Image upload, background hashing, `/uploads` | done | `ddfb508` |
| 6 | Authentication, two-factor, API tokens | done | `0bb8a18` |
| 7 | Admin pages | done | `1d24ad2` |
| 8 | REST API with golden parity tests | done | `479fdc8` |
| 9 | CLI: `create-user`, `ethernetswitch-configure` | done | `e67efb6` |
| 10 | Debian packages for arm64/armhf, systemd unit, in-place upgrade | done | `55243ea` |
| 10a | Fixes from Docker install test | done | `8ebd9fa` |
| 11 | Cutover test on a real Pi 4 | open | — |
| 12 | Remove PHP sources | open, after 11 | — |

### 10a: fixes from the Docker install test

Tested in Debian Bookworm arm64 containers with systemd as PID 1: upgrade from the upstream
1.6.3 release with seeded data, fresh install, remove, purge.

- `debian/preinst` preserves `storage/app/firmware`, which 1.6.x shipped as package files and
  dpkg would delete on upgrade; `postinst` restores it.
- Purge removes all of `/var/lib/cmprovision`, including dnsmasq's lease file.
- `/scriptexecute` explains a missing staged EEPROM image instead of answering a bare 404.

Docker images kept for re-testing: `cmprovision-php163` (PHP install with data) and
`cmprovision-test` (clean Bookworm with systemd and an `rpiboot` stub).

## Phase 11: cutover test on a real Pi 4

Install and back-up steps are in [installing-on-a-pi.md](installing-on-a-pi.md).

1. Upgrade an existing PHP installation: `sudo apt install ./cmprovision4_2.0.0_arm64.deb`.
   Check projects, images, CMs, static IPs and firmware survive; log in with an existing password.
2. Provision a CM4: eMMC, verify enabled, EEPROM firmware update, label printing.
3. Provision a CM3 over USB (IPv6 link-local path).
4. Not yet tested anywhere: SNMP against a real switch, FTP label printing, firmware download
   from GitHub on the Pi, `create-user` in a real terminal, large uploads on armhf.

## Behaviour changes from PHP

- Single service on port 80; nginx, php-fpm and the queue worker are retired by `postinst`.
- Removing the package keeps the database; only purge deletes data.
- Stricter input: serial, Host header, storage path, script heredoc terminator; admin text is
  shell-escaped in generated scripts.
- `cmlogs.ip` is stored.
- Firmware updater handles the `firmware-2711` layout; legacy `stable/…` references keep working.
- Deleting a static IP rewrites `dnsmasq.conf`; `dhcp-host` lines include the hostname.
- Empty EEPROM settings keep the firmware defaults instead of wiping `bootconf.txt`.
- API `PATCH` is validated and re-stages the active project's EEPROM image.
- TOTP codes cannot be replayed; CLI passwords need at least 8 characters.

## Development

- `devenv shell -- cargo test`, `devenv shell -- cargo serve` (http://127.0.0.1:7080, data in
  `target/dev`; port 8080 is an SSH tunnel to a live system), `devenv shell -- css`, `make deb`.
- Golden fixtures are regenerated with
  `devenv shell -- php vendor/bin/phpunit --testsuite Golden` (needs `composer install`).
