# Raspberry Pi Compute Module Provisioning System #

Web application to mass program compute modules; designed to run on a Raspberry Pi 4. Compute Module 3, 3+ and 4 are supported out-of-the-box.
Pi 4 boards can also be provisioned, but that requires that network boot is enabled in EEPROM settings first.

![screenshot](https://user-images.githubusercontent.com/1123701/209007803-7cc38d14-7f07-4910-8108-87323d25ac20.png)

## Documentation ##

Full documentation is available on the Product Information Portal: [Using the Compute Module Provisioner.pdf](https://pip.raspberrypi.com/categories/685-whitepapers-app-notes/documents/RP-003468-WP/Using-the-Compute-Module-Provisioner.pdf)

## Provisioning Compute Module 4 Devices ##

This process requires a Raspberry Pi 4 which will be used solely to mass program compute modules. Make sure eth0 connects to an Ethernet switch that only has the compute modules you wish to program connected.

**WARNING:** Do NOT connect eth0 to your office/public network, since it may also provision other Raspberry Pi devices. Instead, use the wireless LAN interface to access the web interface, and your local network.

1. Start with a fresh 64-bit Raspberry Pi OS (Lite) installation. For simplicity, use [Raspberry Pi Imager](https://www.raspberrypi.com/software/) and the advanced settings menu (Ctrl-Shift-X) to set the password, hostname and wireless LAN configuration.  
    
    **NOTE:** If you intend to write images larger than 2 GB, you MUST install the **64-bit** edition of Raspberry Pi OS (Lite), which is available in Raspberry Pi Imager under the category "Raspberry Pi OS (other)" -> "Raspberry Pi OS Lite (64-bit)".

1. Configure eth0 to have a static IP of 172.20.0.1 inside a /16 subnet (netmask 255.255.0.0):  
    
    ```
    sudo nano /etc/dhcpcd.conf  
    ```
    
    Add to bottom of file:  

    ```
    interface eth0
    static ip_address=172.20.0.1/16  
    ```

    Do not set a default gateway. Reboot afterwards.

1. Run `sudo apt update` to make sure the package information on your Pi is up to date, and it is able to install dependencies.

1. Install the ready-made .deb package from https://github.com/bzhoek/cmprovision/releases/, `arm64` for 64-bit and `armhf` for 32-bit Raspberry Pi OS:  

    ```
    sudo apt install ./cmprovision4_*_arm64.deb  
    ```

1. Set a web application username and password with:  

    ```
    sudo -u www-data cmprovision create-user  
    ```

You can now access the web interface with a web browser on the wireless LAN IP address.

## Provisioning Compute Module 3 and 3+ Devices ##

This process requires a Raspberry Pi 4 which will be used solely to mass program compute modules.

**WARNING:** Do NOT connect eth0 to your office/public network, since it may also provision other Raspberry Pi devices. Instead, use the wireless LAN interface to access the web interface, and your local network.

When provisioning Compute Module 3 and 3+ modules, Ethernet is not used: instead the images are transferred over USB. Use a good quality USB A to micro USB cable between the Pi 4 acting as server and the `USB slave` port of each CMIO board containing the compute modules to be provisioned. If you wish to provision more than 4 compute modules at a time, a USB hub can be used. Connect power supplies to the `power in` micro USB port of the CMIO boards, and make sure the J4 `USB slave boot enable` jumper is set to `en`.


1. Start with a fresh 64-bit Raspberry Pi OS (Lite) installation.  For simplicity, use [Raspberry Pi Imager](https://www.raspberrypi.com/software/) and the advanced settings menu (Ctrl-Shift-X) to set the password, hostname and wireless LAN configuration.  
    
    **NOTE:** If you intend to write images larger than 2 GB, you MUST install the **64-bit** edition of Raspberry Pi OS (Lite), which is available in Raspberry Pi Imager under the category "Raspberry Pi OS (other)" -> "Raspberry Pi OS Lite (64-bit)".

1. Run `sudo apt update` to make sure the package information on your Pi is up to date, and it is able to install dependencies.

1. Install the ready-made .deb package from https://github.com/bzhoek/cmprovision/releases/, `arm64` for 64-bit and `armhf` for 32-bit Raspberry Pi OS:  

    ```
    sudo apt install ./cmprovision4_*_arm64.deb  
    ```

1. Set a web application username and password with:  

    ```
    sudo -u www-data cmprovision create-user  
    ```

You can now access the web interface with a web browser on the wireless LAN IP address.

### Provisioning from an alternate OS ###

During provisioning of CM3 and CM3+ devices, a small utility operating system, `scriptexecute`, is USB booted on each compute module. This configures the compute modules as  USB network adapters, and expects to be able to reach the provisioning server on predictable IPv6 link-local addresses that can be calculated based on the MAC address that each compute module chooses for its USB network interface. On Raspberry Pi OS, this is configured automatically by putting `slaac hwaddr` in `/etc/dhcpcd.conf`, however if your OS does not use `dhcpcd` as its network manager, then you will need to set this up manually. Examples of alternate network managers include `systemd-networkd` and `Network Manager`. How you set this up depends on the exact network manager used by your Linux distribution, so we cannot advise on how to do this.

## Provisioning Raspberry Pi 4 boards ##

It is also possible to use the system to provision regular Raspberry Pi 4 boards, instead of CM4 modules.
However that requires that network boot is enabled first in the EEPROM settings of each Pi 4.

You can create a SD card that enables that with [Imager](https://www.raspberrypi.com/software/), by going to: "Choose OS" -> "Misc utility images" -> "Bootloader" -> "Network boot"

## Upgrading from the PHP version ##

Installing version 2 over 1.x keeps the database, images, firmware and static IP assignments. Afterwards:

* cmprovision serves port 80 itself. The nginx site is removed, and nginx is stopped and disabled when it served nothing else.
* Users must enable two factor authentication again on their profile page.
* `artisan auth:create-user` and `artisan ethernetswitch:configure` are now `cmprovision create-user` and `cmprovision ethernetswitch-configure`.

PHP, php-fpm and nginx are no longer dependencies; remove them with `sudo apt autoremove` if nothing else uses them.

## Development ##

The server is a single Rust binary. Its toolchain comes from [devenv](https://devenv.sh):

* `devenv shell -- cargo test` runs the test suite.
* `devenv shell -- cargo serve` runs the server on http://127.0.0.1:7080 with its data in `target/dev`.
* `devenv shell -- css` rebuilds `assets/dist/app.css` after changing Tailwind classes in `templates/`.
* `make deb` cross-compiles and packages `target/debian/cmprovision4_<version>_{arm64,armhf}.deb`.

`tests/fixtures` holds responses captured from the PHP version by `tests/Golden/CaptureTest.php`; the Rust tests compare against them to keep the device and API contracts unchanged.

## Licence ##

The main code of the Raspberry Pi compute module provisioning system is made available under the terms of the BSD 3-clause ("new") license.
Run `cargo tree` to list the dependencies, whose licenses are in their crates.
