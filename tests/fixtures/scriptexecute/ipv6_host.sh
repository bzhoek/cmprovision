set -o pipefail

export SERIAL="10000000abcdef12"
export SERVER="[fe80::1%usb0]"
export STORAGE="/dev/mmcblk0"
export PART1="/dev/mmcblk0p1"
export PART2="/dev/mmcblk0p2"

# Make sure we have random entropy
echo "RANDOM64" >/dev/urandom


echo Querying and registering EEPROM version
vcgencmd bootloader_version >/tmp/eeprom_version || true
if [ -f /tmp/eeprom_version ]; then
    curl --retry 10 -g -F 'eeprom_version=@/tmp/eeprom_version' "http://[fe80::1%usb0]/scriptexecute?serial=10000000abcdef12"
fi


echo Sending BLKDISCARD to $STORAGE
blkdiscard -v $STORAGE || true

echo Writing image from http://[fe80::1%usb0]/uploads/abcdefghijklmnopqrstuvwxyz0123456789ABCD.gz to $STORAGE
curl --retry 10 -g "http://[fe80::1%usb0]/uploads/abcdefghijklmnopqrstuvwxyz0123456789ABCD.gz" \
 | gzip -dc  \
 | dd of=$STORAGE conv=fsync obs=1M >/tmp/dd.log 2>&1
RETCODE=$?
if [ $RETCODE -eq 0 ]; then
    echo Original image written successfully
else
    echo Writing image failed.
    curl --retry 10 -g -F 'log=@/tmp/dd.log' "http://[fe80::1%usb0]/scriptexecute?serial=10000000abcdef12&retcode=$RETCODE&phase=dd"
    exit 1
fi

partprobe $STORAGE
sleep 0.1


TEMP=`vcgencmd measure_temp`
curl --retry 10 -g "http://[fe80::1%usb0]/scriptexecute?serial=10000000abcdef12&alldone=1&temp=${TEMP:5}&verify="

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
