//! The shell script a booted Compute Module runs to provision itself.
//!
//! Output is byte-identical to the former Blade template for ordinary input, including
//! its whitespace quirks and the missing shebang (PHP strips a leading `#!` line from
//! compiled views). The scriptexecute initramfs runs it with `sh`, so neither matters.

use std::fmt::Write;

pub struct Step {
    pub id: i64,
    pub name: String,
    pub bg: bool,
    pub script: String,
}

pub struct Image {
    pub url: String,
    pub extension: String,
}

pub struct Context<'a> {
    pub serial: &'a str,
    pub server: &'a str,
    pub storage: &'a str,
    pub part1: &'a str,
    pub part2: &'a str,
    pub random: &'a str,
    pub preinstall: &'a [Step],
    pub postinstall: &'a [Step],
    pub flashes_eeprom: bool,
    pub bootmode: Option<&'a str>,
    pub image: Option<Image>,
    pub verify: bool,
}

/// Admin-entered text interpolated inside double quotes or a comment; Blade's HTML
/// escaping did not make that safe, so escape for the shell instead.
fn text(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            '\n' | '\r' => vec![' '],
            '\\' | '"' | '$' | '`' => vec!['\\', c],
            c => vec![c],
        })
        .collect()
}

pub fn render(c: &Context) -> String {
    let mut s = String::new();
    let base = format!("http://{}/scriptexecute?serial={}", c.server, c.serial);

    writeln!(s, "set -o pipefail").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "export SERIAL=\"{}\"", c.serial).unwrap();
    writeln!(s, "export SERVER=\"{}\"", c.server).unwrap();
    writeln!(s, "export STORAGE=\"{}\"", text(c.storage)).unwrap();
    writeln!(s, "export PART1=\"{}\"", text(c.part1)).unwrap();
    writeln!(s, "export PART2=\"{}\"", text(c.part2)).unwrap();
    writeln!(s).unwrap();
    writeln!(s, "# Make sure we have random entropy").unwrap();
    writeln!(s, "echo \"{}\" >/dev/urandom", c.random).unwrap();
    writeln!(s).unwrap();

    for (phase, steps) in [("pre", c.preinstall), ("post", c.postinstall)] {
        for step in steps {
            writeln!(s, "# Writing script '{}' to /tmp/{phase}-{}.sh", text(&step.name), step.id).unwrap();
            writeln!(s, "cat >/tmp/{phase}-{}.sh << \"CMPROVISIONINGEOF\"", step.id).unwrap();
            writeln!(s, "{}", step.script).unwrap();
            writeln!(s, "CMPROVISIONINGEOF").unwrap();
        }
    }
    writeln!(s).unwrap();

    if !c.flashes_eeprom {
        writeln!(s, "echo Querying and registering EEPROM version").unwrap();
        if c.bootmode == Some("3") {
            writeln!(s, "flashrom -p \"linux_spi:dev=/dev/spidev0.0,spispeed=16000\" -r \"/tmp/pieeprom.bin\" || true").unwrap();
            writeln!(s, "strings /tmp/pieeprom.bin |grep VERSION: >/tmp/eeprom_version").unwrap();
            writeln!(s, "strings /tmp/pieeprom.bin |grep BUILD_TIMESTAMP= >>/tmp/eeprom_version").unwrap();
        } else {
            writeln!(s, "vcgencmd bootloader_version >/tmp/eeprom_version || true").unwrap();
        }
        writeln!(s, "if [ -f /tmp/eeprom_version ]; then").unwrap();
        writeln!(s, "    curl --retry 10 -g -F 'eeprom_version=@/tmp/eeprom_version' \"{base}\"").unwrap();
        writeln!(s, "fi").unwrap();
    }
    writeln!(s).unwrap();

    run_steps(&mut s, &base, c.preinstall, "pre", "pre-install", "pre-installation", "Pre-installation", "preinstall");
    writeln!(s).unwrap();

    if let Some(image) = &c.image {
        let decompress = match image.extension.as_str() {
            "gz" => " | gzip -dc ",
            "xz" => " | xz -dc ",
            "bz2" => " | bunzip2 -dc ",
            _ => "",
        };
        writeln!(s, "echo Sending BLKDISCARD to $STORAGE").unwrap();
        writeln!(s, "blkdiscard -v $STORAGE || true").unwrap();
        writeln!(s).unwrap();
        writeln!(s, "echo Writing image from {} to $STORAGE", image.url).unwrap();
        writeln!(s, "curl --retry 10 -g \"{}\" \\", image.url).unwrap();
        writeln!(s, "{decompress} \\").unwrap();
        writeln!(s, " | dd of=$STORAGE conv=fsync obs=1M >/tmp/dd.log 2>&1").unwrap();
        writeln!(s, "RETCODE=$?").unwrap();
        writeln!(s, "if [ $RETCODE -eq 0 ]; then").unwrap();
        writeln!(s, "    echo Original image written successfully").unwrap();
        writeln!(s, "else").unwrap();
        writeln!(s, "    echo Writing image failed.").unwrap();
        writeln!(s, "    curl --retry 10 -g -F 'log=@/tmp/dd.log' \"{base}&retcode=$RETCODE&phase=dd\"").unwrap();
        writeln!(s, "    exit 1").unwrap();
        writeln!(s, "fi").unwrap();
        writeln!(s).unwrap();
        writeln!(s, "partprobe $STORAGE").unwrap();
        writeln!(s, "sleep 0.1").unwrap();
    }
    writeln!(s).unwrap();

    run_steps(&mut s, &base, c.postinstall, "post", "post-install", "post-installation", "Postinstallation", "postinstall");
    writeln!(s).unwrap();

    writeln!(s, "TEMP=`vcgencmd measure_temp`").unwrap();
    writeln!(s, "curl --retry 10 -g \"{base}&alldone=1&temp=${{TEMP:5}}&verify={}\"", if c.verify { "1" } else { "" }).unwrap();
    s.push_str(TRAILER);
    s
}

#[allow(clippy::too_many_arguments)]
fn run_steps(s: &mut String, base: &str, steps: &[Step], file: &str, title: &str, noun: &str, failed: &str, phase: &str) {
    if steps.is_empty() {
        return;
    }
    writeln!(s, "echo \"Running {title} scripts\"").unwrap();
    for step in steps {
        writeln!(s, "echo \"===\" >> /tmp/{file}.log").unwrap();
        writeln!(s, "echo \"Running {noun} script '{}'\" >> /tmp/{file}.log", text(&step.name)).unwrap();
        writeln!(s, "echo \"===\" >> /tmp/{file}.log").unwrap();
        writeln!(s, "sh -v /tmp/{file}-{}.sh >>/tmp/{file}.log 2>&1 {} ", step.id, if step.bg { " & " } else { "" }).unwrap();
        writeln!(s, "RETCODE=$?").unwrap();
        writeln!(s, "if [ $RETCODE -ne 0 ]; then").unwrap();
        writeln!(s, "    echo \"{failed} script failed.\"").unwrap();
        writeln!(s, "    curl --retry 10 -g -F 'log=@/tmp/{file}.log' \"{base}&retcode=$RETCODE&phase={phase}\"").unwrap();
        writeln!(s, "    exit 1").unwrap();
        writeln!(s, "fi").unwrap();
    }
    writeln!(s, "curl --retry 10 -g -F 'log=@/tmp/{file}.log' \"{base}&retcode=0&phase={phase}\"").unwrap();
}

const TRAILER: &str = r#"
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
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_admin_text_for_double_quotes() {
        assert_eq!(text("a \"b\" $(c) `d` \\e\nf"), "a \\\"b\\\" \\$(c) \\`d\\` \\\\e f");
    }
}
