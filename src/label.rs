use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

use suppaftp::FtpStream;
use suppaftp::types::FileType;

pub enum Printer {
    Ftp { hostname: String, username: String, password: String },
    Command { command: String },
}

pub struct Label {
    pub printer: Printer,
    pub file_extension: String,
    pub template: String,
}

const FTP_TIMEOUT: Duration = Duration::from_secs(30);

pub fn render(template: &str, mac: &str, serial: &str, board: &str) -> String {
    template
        .replace("$mac", mac)
        .replace("$serial", serial)
        .replace("$provisionboard", board)
}

/// Blocking: runs a print command or FTP upload, so call it from a blocking context.
/// Errors are human-readable messages destined for the provisioning log.
pub fn print(label: &Label, serial: &str, contents: &str) -> Result<(), String> {
    let mut file = tempfile::Builder::new()
        .prefix("label-")
        .tempfile()
        .map_err(|_| "Error creating temporary file for label".to_string())?;
    file.write_all(contents.as_bytes())
        .and_then(|_| file.flush())
        .map_err(|_| format!("Error creating temporary file for label '{}'", file.path().display()))?;

    match &label.printer {
        Printer::Ftp { hostname, username, password } => {
            let remote = format!("label-{serial}.{}", label.file_extension);
            upload(hostname, username, password, &remote, contents.as_bytes())
        }
        Printer::Command { command } => {
            let cmd = command.replace("$file", &shell_quote(&file.path().to_string_lossy()));
            run(&cmd)
        }
    }
}

fn upload(hostname: &str, username: &str, password: &str, remote: &str, mut contents: &[u8]) -> Result<(), String> {
    let mut ftp = connect(hostname).ok_or_else(|| format!("Error connecting to printer's FTP server {hostname}"))?;
    ftp.login(username, password)
        .map_err(|_| "Error logging in to printer's FTP server. Check username and password".to_string())?;
    ftp.transfer_type(FileType::Binary)
        .and_then(|_| ftp.put_file(remote, &mut contents))
        .map_err(|_| "Error uploading file to printer's FTP server".to_string())?;
    let _ = ftp.quit();
    Ok(())
}

fn connect(hostname: &str) -> Option<FtpStream> {
    use std::net::ToSocketAddrs;
    let addr = (hostname, 21).to_socket_addrs().ok()?.next()?;
    FtpStream::connect_timeout(addr, FTP_TIMEOUT).ok()
}

fn run(cmd: &str) -> Result<(), String> {
    // Only stdout is captured, like PHP's exec(); templates append 2>&1 when they want stderr.
    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|_| format!("Error executing '{cmd}'"))?;

    if output.status.success() {
        return Ok(());
    }
    let code = output.status.code().unwrap_or(-1);
    let text = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    Err(format!("Executing '{cmd}' returned exit code {code}. Program output:\n{}", lines.join("\n")))
}

pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(cmd: &str) -> Label {
        Label {
            printer: Printer::Command { command: cmd.into() },
            file_extension: "txt".into(),
            template: String::new(),
        }
    }

    #[test]
    fn renders_placeholders() {
        assert_eq!(
            render("$mac/$serial/$provisionboard/$mac", "aa:bb", "1234", "101 (5)"),
            "aa:bb/1234/101 (5)/aa:bb"
        );
    }

    #[test]
    fn quotes_for_shell() {
        assert_eq!(shell_quote("/tmp/a'b"), r"'/tmp/a'\''b'");
    }

    #[test]
    fn command_receives_label_file() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        let label = command(&format!("cp $file {}", shell_quote(&out.to_string_lossy())));

        print(&label, "1234", "label contents").unwrap();

        assert_eq!(std::fs::read_to_string(out).unwrap(), "label contents");
    }

    #[test]
    fn failing_command_reports_like_php() {
        let err = print(&command("printf 'one  \\ntwo\\n'; exit 3"), "1234", "x").unwrap_err();
        assert_eq!(err, "Executing 'printf 'one  \\ntwo\\n'; exit 3' returned exit code 3. Program output:\none\ntwo");
    }

    #[test]
    fn failing_command_message_matches_golden_format() {
        let err = print(&command("false $file"), "1234", "x").unwrap_err();
        let normalized = regex::Regex::new(r"'[^' ]*/label-[A-Za-z0-9]+'").unwrap().replace(&err, "'LABELFILE'");
        assert_eq!(normalized, "Executing 'false 'LABELFILE'' returned exit code 1. Program output:\n");
    }

    #[test]
    fn unreachable_ftp_server() {
        let label = Label {
            printer: Printer::Ftp { hostname: "127.0.0.1".into(), username: "u".into(), password: "p".into() },
            file_extension: "txt".into(),
            template: String::new(),
        };
        assert_eq!(print(&label, "1234", "x").unwrap_err(), "Error connecting to printer's FTP server 127.0.0.1");
    }
}
