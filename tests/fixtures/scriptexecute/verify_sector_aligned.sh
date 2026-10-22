set -o pipefail

export SERIAL="10000000abcdef12"
export SERVER="172.20.0.1"
export STORAGE="/dev/mmcblk0"
export PART1="/dev/mmcblk0p1"
export PART2="/dev/mmcblk0p2"

# Make sure we have random entropy
echo "RANDOM64" >/dev/urandom

# Writing script 'Verifying written image' to /tmp/post-0.sh
cat >/tmp/post-0.sh << "CMPROVISIONINGEOF"
#!/bin/sh
set -e
sync; echo 3 > /proc/sys/vm/drop_caches
READ_SHA256=$(dd if=/dev/mmcblk0 count=2049 | sha256sum | awk '{print $1}')
echo Computed SHA256: "$READ_SHA256"
if [ "$READ_SHA256" = "aa00000000000000000000000000000000000000000000000000000000000000" ]; then echo Verification successful!; else echo Verification failed; exit 2; fi

CMPROVISIONINGEOF

echo Querying and registering EEPROM version
vcgencmd bootloader_version >/tmp/eeprom_version || true
if [ -f /tmp/eeprom_version ]; then
    curl --retry 10 -g -F 'eeprom_version=@/tmp/eeprom_version' "http://172.20.0.1/scriptexecute?serial=10000000abcdef12"
fi


echo Sending BLKDISCARD to $STORAGE
blkdiscard -v $STORAGE || true

echo Writing image from http://172.20.0.1/uploads/abcdefghijklmnopqrstuvwxyz0123456789ABCD.xz to $STORAGE
curl --retry 10 -g "http://172.20.0.1/uploads/abcdefghijklmnopqrstuvwxyz0123456789ABCD.xz" \
 | xz -dc  \
 | dd of=$STORAGE conv=fsync obs=1M >/tmp/dd.log 2>&1
RETCODE=$?
if [ $RETCODE -eq 0 ]; then
    echo Original image written successfully
else
    echo Writing image failed.
    curl --retry 10 -g -F 'log=@/tmp/dd.log' "http://172.20.0.1/scriptexecute?serial=10000000abcdef12&retcode=$RETCODE&phase=dd"
    exit 1
fi

partprobe $STORAGE
sleep 0.1

echo "Running post-install scripts"
echo "===" >> /tmp/post.log
echo "Running post-installation script 'Verifying written image'" >> /tmp/post.log
echo "===" >> /tmp/post.log
sh -v /tmp/post-0.sh >>/tmp/post.log 2>&1  
RETCODE=$?
if [ $RETCODE -ne 0 ]; then
    echo "Postinstallation script failed."
    curl --retry 10 -g -F 'log=@/tmp/post.log' "http://172.20.0.1/scriptexecute?serial=10000000abcdef12&retcode=$RETCODE&phase=postinstall"
    exit 1
fi
curl --retry 10 -g -F 'log=@/tmp/post.log' "http://172.20.0.1/scriptexecute?serial=10000000abcdef12&retcode=0&phase=postinstall"

TEMP=`vcgencmd measure_temp`
curl --retry 10 -g "http://172.20.0.1/scriptexecute?serial=10000000abcdef12&alldone=1&temp=${TEMP:5}&verify=1"

echo ""
echo "====="
echo "Provisioning completed successfully!"

sleep 0.1
if [ -f /sys/kernel/config/usb_gadget/g1/UDC ]; then
    echo "" > /sys/kernel/config/usb_gadget/g1/UDC
fi

if [ -e /sys/class/leds/led1 ]; then
    while true; do
        echo 255 > /sys/class/leds/led0/brightness
        echo 0 > /sys/class/leds/led1/brightness
        sleep 0.5
        echo 0 > /sys/class/leds/led0/brightness
        echo 255 > /sys/class/leds/led1/brightness
        sleep 0.5
    done
fi
if [ -e /sys/class/leds/led0 ]; then
    echo 255 > /sys/class/leds/led0/brightness
fi
