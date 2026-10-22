use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Args;

#[derive(Args, Clone, Debug)]
pub struct Config {
    /// Installation directory; the layout matches the PHP package so upgrades work in place.
    #[arg(long, env = "CMPROVISION_DATA_DIR", default_value = "/var/lib/cmprovision")]
    pub data_dir: PathBuf,

    /// SQLite database file. Defaults to database/database.sqlite inside the data directory.
    #[arg(long, env = "CMPROVISION_DATABASE")]
    pub database: Option<PathBuf>,
}

impl Config {
    pub fn database(&self) -> PathBuf {
        self.database
            .clone()
            .unwrap_or_else(|| self.data_dir.join("database/database.sqlite"))
    }

    pub fn uploads_dir(&self) -> PathBuf {
        self.data_dir.join("public/uploads")
    }

    pub fn firmware_dir(&self) -> PathBuf {
        self.data_dir.join("storage/app/firmware")
    }

    pub fn dnsmasq_conf(&self) -> PathBuf {
        self.data_dir.join("etc/dnsmasq.conf")
    }
}

#[derive(Args, Clone, Debug)]
pub struct ServeArgs {
    #[arg(long, env = "CMPROVISION_LISTEN", default_value = "[::]:80")]
    pub listen: SocketAddr,
}
