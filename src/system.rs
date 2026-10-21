//! The services cmprovision runs alongside, managed through systemd.

use std::path::Path;

use tokio::process::Command;

pub const DNSMASQ: &str = "cmprovision-dnsmasq";
pub const RPIBOOT: &str = "cmprovision-rpiboot";
pub const SERVER: &str = "cmprovision";

/// Services whose journal may be shown in the web interface.
pub const LOGGED: &[(&str, &str)] = &[
    (DNSMASQ, "dnsmasq (DHCP server)"),
    (RPIBOOT, "rpiboot (USB file server)"),
    (SERVER, "cmprovision (web interface)"),
];

pub async fn is_active(service: &str) -> bool {
    Command::new("/bin/systemctl")
        .args(["is-active", "--quiet", service])
        .status()
        .await
        .is_ok_and(|s| s.success())
}

pub async fn journal(service: &str) -> String {
    match Command::new("/bin/journalctl").args(["--boot", "--no-pager", "-u", service]).output().await {
        Ok(output) => String::from_utf8_lossy(&output.stdout).into_owned(),
        Err(err) => format!("Unable to run journalctl: {err}"),
    }
}

/// Allowed for the service user by /etc/sudoers.d/010_cmprovision.
pub async fn restart_dnsmasq() -> bool {
    Command::new("sudo")
        .args(["-n", "/bin/systemctl", "restart", DNSMASQ])
        .status()
        .await
        .is_ok_and(|s| s.success())
}

pub fn free_space(path: &Path) -> Option<u64> {
    let stat = nix::sys::statvfs::statvfs(path).ok()?;
    Some(stat.blocks_available() as u64 * stat.fragment_size() as u64)
}
