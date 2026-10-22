# Installing on a Raspberry Pi 4

Use a Pi 4 dedicated to provisioning, with eth0 connected to a switch that has only the
modules you want to program. Any other Raspberry Pi on that network set to network boot
would be provisioned too.

## 1. Build the package

On a development machine, in this repository:

```
make deb-arm64        # 64-bit Raspberry Pi OS
make deb-armhf        # 32-bit Raspberry Pi OS
```

The package is written to `target/debian/cmprovision4_<version>_<arch>.deb`. To see which
one a Pi needs, run `dpkg --print-architecture` on it.

## 2. Upgrading from the PHP version: stop it and back it up

Skip this step on a fresh Pi.

The upgrade changes the database: it clears two-factor secrets and drops tables the PHP
version needs. Reinstalling 1.x afterwards therefore only works from a backup.

The PHP version consists of five services:

| Service | Role |
|---|---|
| `nginx` | web server for the web interface |
| `php*-fpm` (e.g. `php8.2-fpm`) | runs the PHP code |
| `cmprovision-queue` | computes image hashes in the background |
| `cmprovision-dnsmasq` | DHCP and TFTP for booting modules |
| `cmprovision-rpiboot` | USB boot for CM3 |

Stop them all, so nothing writes to the database or uploads during the backup. The quotes
keep the shell from expanding `*`, so systemd matches the installed PHP version.

```
sudo systemctl stop nginx 'php*-fpm.service' cmprovision-queue cmprovision-dnsmasq cmprovision-rpiboot
systemctl is-active nginx 'php*-fpm.service' cmprovision-queue cmprovision-dnsmasq cmprovision-rpiboot
```

Every line should read `inactive`. Then back up the data directory:

```
sudo tar czf ~/cmprovision-backup.tgz -C /var/lib cmprovision
```

To back out before installing, start the same services again with `sudo systemctl start`.

## 3. Install

```
scp target/debian/cmprovision4_2.0.0_arm64.deb pi@<pi-address>:
ssh pi@<pi-address>
sudo apt update
sudo apt install ./cmprovision4_2.0.0_arm64.deb
```

apt pulls in `rpiboot` from the Raspberry Pi repository and `dnsmasq-base`. The package
then does the following:

- **Services:** starts `cmprovision`, `cmprovision-dnsmasq` and `cmprovision-rpiboot`, and
  enables them at boot.
- **Upgrading from PHP:**
  - removes the nginx site, and stops and disables nginx if it served nothing else;
  - retires the queue worker;
  - keeps the database, images, firmware and static IP assignments.
- **Fresh install:** creates the database.

PHP and php-fpm stay installed but stopped. Remove them with `sudo apt autoremove` if nothing
else uses them.

## 4. Log in

On a fresh install, create an account:

```
sudo -u www-data cmprovision create-user
```

After an upgrade, existing accounts and passwords keep working. Users who had two-factor
authentication enabled must enable it again on their profile page.

Open `http://<wlan-ip>/` in a browser.

## 5. Network

eth0 needs the static address 172.20.0.1/16, without a gateway.

**Raspberry Pi OS Bookworm and later** use NetworkManager:

```
nmcli con show
sudo nmcli con mod "Wired connection 1" ipv4.method manual ipv4.addresses 172.20.0.1/16 ipv4.gateway "" ipv6.method link-local
sudo nmcli con up "Wired connection 1"
```

**Older releases using dhcpcd:** add this to `/etc/dhcpcd.conf` and reboot:

```
interface eth0
static ip_address=172.20.0.1/16
```

Provisioning CM3 modules over USB also needs predictable IPv6 link-local addresses on the USB
network interfaces. The package sets that up for dhcpcd only (`slaac hwaddr`). With
NetworkManager it has to be configured by hand; this is not yet verified, so test CM3
separately.

## 6. Check it runs

```
systemctl status cmprovision cmprovision-dnsmasq cmprovision-rpiboot
journalctl -u cmprovision -f            # web interface and provisioning requests
journalctl -u cmprovision-dnsmasq -f    # DHCP/TFTP: does the module ask for an address?
```

The Settings page in the web interface shows the same service status and logs.
[flashing-process.md](flashing-process.md) explains each step a module goes through, and
what the log messages mean.

## Rolling back to the PHP version

Restore the backup from step 2, then reinstall 1.6.3. Its package is on the
[upstream releases page](https://github.com/raspberrypi/cmprovision/releases).

```
sudo apt purge cmprovision4
sudo tar xzf ~/cmprovision-backup.tgz -C /var/lib
sudo apt install ./cmprovision4_1.6.3_all.deb
sudo systemctl enable --now nginx
```
