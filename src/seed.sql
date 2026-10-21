INSERT INTO "labels" ("name", "printer_type", "ftp_hostname", "ftp_username", "ftp_password", "print_command", "file_extension", "template") VALUES (
'Datamatrix on Brady/CAB printer', 'ftp', 'CHANGEME', 'print', 'ftpprint', '/usr/bin/lpr -o raw $file 2>&1', 'jscript',
'm m
J
; 6.35 mm label height, 19.05 mm width
S l1;0,0,6.35,6.35,19.05
; 18x18 matrix can hold 25 characters, and at 0.3 mm dot width should be 5.4x5.4 mm
B 0.4,0.6,0,DATAMATRIX+ROWS18+COLS18,0.3;$mac
; text MAC address
T 6,2,0,3,1.5;$mac
; text CMIO board (jumper setting)
T 14,6,0,3,1.5;$provisionboard
A 1');

INSERT INTO "scripts" ("name", "script_type", "priority", "bg", "script") VALUES (
'Resize ext4 partition', 'postinstall', 50, 0,
'#!/bin/sh
set -e

parted -s $STORAGE "resizepart 2 -1" "quit"
resize2fs -f $PART2

mkdir -p /mnt/boot /mnt/root
mount -t ext4 $PART2 /mnt/root
umount /mnt/root
mount -t vfat $PART1 /mnt/boot
sed -i ''s| init=/usr/lib/raspi-config/init_resize\.sh||'' /mnt/boot/cmdline.txt
umount /mnt/boot');

INSERT INTO "scripts" ("name", "script_type", "priority", "bg", "script") VALUES (
'Add dtoverlay=dwc2 to config.txt', 'postinstall', 100, 0,
'#!/bin/sh
set -e

mkdir -p /mnt/boot
mount -t vfat $PART1 /mnt/boot
echo "dtoverlay=dwc2,dr_mode=host" >> /mnt/boot/config.txt
umount /mnt/boot');

INSERT INTO "scripts" ("name", "script_type", "priority", "bg", "script") VALUES (
'Format eMMC as pSLC (one time settable only)', 'preinstall', 100, 0,
'#!/bin/sh
set +e

MAXSIZEKB=`mmc extcsd read /dev/mmcblk0 | grep MAX_ENH_SIZE_MULT -A 1 | grep -o ''[0-9]\+ ''`
mmc enh_area set -y 0 $MAXSIZEKB /dev/mmcblk0
if [ $? -eq 0 ]; then
    reboot -f
fi');
