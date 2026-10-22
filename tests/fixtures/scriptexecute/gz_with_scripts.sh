set -o pipefail

export SERIAL="10000000abcdef12"
export SERVER="172.20.0.1"
export STORAGE="/dev/mmcblk0"
export PART1="/dev/mmcblk0p1"
export PART2="/dev/mmcblk0p2"

# Make sure we have random entropy
echo "RANDOM64" >/dev/urandom

# Writing script 'pre-b' to /tmp/pre-1.sh
cat >/tmp/pre-1.sh << "CMPROVISIONINGEOF"
#!/bin/sh
echo 'pre-b' "$STORAGE"

CMPROVISIONINGEOF
# Writing script 'pre-a' to /tmp/pre-2.sh
cat >/tmp/pre-2.sh << "CMPROVISIONINGEOF"
#!/bin/sh
echo 'pre-a' "$STORAGE"

CMPROVISIONINGEOF
# Writing script 'pre-late' to /tmp/pre-3.sh
cat >/tmp/pre-3.sh << "CMPROVISIONINGEOF"
#!/bin/sh
echo 'pre-late' "$STORAGE"

CMPROVISIONINGEOF
# Writing script 'post-bg' to /tmp/post-4.sh
cat >/tmp/post-4.sh << "CMPROVISIONINGEOF"
#!/bin/sh
echo 'post-bg' "$STORAGE"

CMPROVISIONINGEOF

echo Querying and registering EEPROM version
vcgencmd bootloader_version >/tmp/eeprom_version || true
if [ -f /tmp/eeprom_version ]; then
    curl --retry 10 -g -F 'eeprom_version=@/tmp/eeprom_version' "http://172.20.0.1/scriptexecute?serial=10000000abcdef12"
fi

echo "Running pre-install scripts"
echo "===" >> /tmp/pre.log
echo "Running pre-installation script 'pre-b'" >> /tmp/pre.log
echo "===" >> /tmp/pre.log
sh -v /tmp/pre-1.sh >>/tmp/pre.log 2>&1  
RETCODE=$?
if [ $RETCODE -ne 0 ]; then
    echo "Pre-installation script failed."
    curl --retry 10 -g -F 'log=@/tmp/pre.log' "http://172.20.0.1/scriptexecute?serial=10000000abcdef12&retcode=$RETCODE&phase=preinstall"
    exit 1
fi
echo "===" >> /tmp/pre.log
echo "Running pre-installation script 'pre-a'" >> /tmp/pre.log
echo "===" >> /tmp/pre.log
sh -v /tmp/pre-2.sh >>/tmp/pre.log 2>&1  
RETCODE=$?
if [ $RETCODE -ne 0 ]; then
    echo "Pre-installation script failed."
    curl --retry 10 -g -F 'log=@/tmp/pre.log' "http://172.20.0.1/scriptexecute?serial=10000000abcdef12&retcode=$RETCODE&phase=preinstall"
    exit 1
fi
echo "===" >> /tmp/pre.log
echo "Running pre-installation script 'pre-late'" >> /tmp/pre.log
echo "===" >> /tmp/pre.log
sh -v /tmp/pre-3.sh >>/tmp/pre.log 2>&1  
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
echo "Running post-installation script 'post-bg'" >> /tmp/post.log
echo "===" >> /tmp/post.log
sh -v /tmp/post-4.sh >>/tmp/post.log 2>&1  &  
RETCODE=$?
if [ $RETCODE -ne 0 ]; then
    echo "Postinstallation script failed."
    curl --retry 10 -g -F 'log=@/tmp/post.log' "http://172.20.0.1/scriptexecute?serial=10000000abcdef12&retcode=$RETCODE&phase=postinstall"
    exit 1
fi
curl --retry 10 -g -F 'log=@/tmp/post.log' "http://172.20.0.1/scriptexecute?serial=10000000abcdef12&retcode=0&phase=postinstall"

TEMP=`vcgencmd measure_temp`
curl --retry 10 -g "http://172.20.0.1/scriptexecute?serial=10000000abcdef12&alldone=1&temp=${TEMP:5}&verify="

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
