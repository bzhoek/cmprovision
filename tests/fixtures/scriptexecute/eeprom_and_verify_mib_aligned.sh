set -o pipefail

export SERIAL="10000000abcdef12"
export SERVER="172.20.0.1"
export STORAGE="/dev/mmcblk0"
export PART1="/dev/mmcblk0p1"
export PART2="/dev/mmcblk0p2"

# Make sure we have random entropy
echo "RANDOM64" >/dev/urandom

# Writing script 'Flash EEPROM firmware (stable/pieeprom-2024-01-01.bin)' to /tmp/pre-0.sh
cat >/tmp/pre-0.sh << "CMPROVISIONINGEOF"
#!/bin/sh
set -e
curl --retry 10 --silent --show-error -g -o pieeprom.bin "http://172.20.0.1/uploads/pieeprom.bin"
echo "cc00000000000000000000000000000000000000000000000000000000000000  pieeprom.bin" | sha256sum -c
flashrom -p "linux_spi:dev=/dev/spidev0.0,spispeed=16000" -w "pieeprom.bin"

CMPROVISIONINGEOF
# Writing script 'Verifying written image' to /tmp/post-0.sh
cat >/tmp/post-0.sh << "CMPROVISIONINGEOF"
#!/bin/sh
set -e
sync; echo 3 > /proc/sys/vm/drop_caches
READ_SHA256=$(dd if=/dev/mmcblk0 bs=1M count=1024 | sha256sum | awk '{print $1}')
echo Computed SHA256: "$READ_SHA256"
if [ "$READ_SHA256" = "aa00000000000000000000000000000000000000000000000000000000000000" ]; then echo Verification successful!; else echo Verification failed; exit 2; fi

CMPROVISIONINGEOF
# Writing script 'post-a' to /tmp/post-1.sh
cat >/tmp/post-1.sh << "CMPROVISIONINGEOF"
#!/bin/sh
echo 'post-a' "$STORAGE"

CMPROVISIONINGEOF


echo "Running pre-install scripts"
echo "===" >> /tmp/pre.log
echo "Running pre-installation script 'Flash EEPROM firmware (stable/pieeprom-2024-01-01.bin)'" >> /tmp/pre.log
echo "===" >> /tmp/pre.log
sh -v /tmp/pre-0.sh >>/tmp/pre.log 2>&1  
RETCODE=$?
if [ $RETCODE -ne 0 ]; then
    echo "Pre-installation script failed."
    curl --retry 10 -g -F 'log=@/tmp/pre.log' "http://172.20.0.1/scriptexecute?serial=10000000abcdef12&retcode=$RETCODE&phase=preinstall"
    exit 1
fi
curl --retry 10 -g -F 'log=@/tmp/pre.log' "http://172.20.0.1/scriptexecute?serial=10000000abcdef12&retcode=0&phase=preinstall"

echo Sending BLKDISCARD to $STORAGE
blkdiscard -v $STORAGE || true

echo Writing image from http://172.20.0.1/uploads/abcdefghijklmnopqrstuvwxyz0123456789ABCD.gz to $STORAGE
curl --retry 10 -g "http://172.20.0.1/uploads/abcdefghijklmnopqrstuvwxyz0123456789ABCD.gz" \
 | gzip -dc  \
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
echo "===" >> /tmp/post.log
echo "Running post-installation script 'post-a'" >> /tmp/post.log
echo "===" >> /tmp/post.log
sh -v /tmp/post-1.sh >>/tmp/post.log 2>&1  
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
