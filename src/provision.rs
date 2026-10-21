//! `/scriptexecute`: the unauthenticated endpoint Compute Modules talk to while booted
//! into the scriptexecute initramfs. Its query parameters, multipart fields and
//! responses are a contract with that image and must not change.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::LazyLock;

use anyhow::Context as _;
use axum::extract::{ConnectInfo, FromRequest, Multipart, Request, State};
use axum::http::{StatusCode, header};
use axum::response::Response;
use rand::distr::{Alphanumeric, SampleString};
use regex::Regex;

use crate::models::{self, Project};
use crate::script::{self, Step};
use crate::switch::{EthernetSwitch, SnmpSession};
use crate::web::{self, Error, Result};
use crate::{AppState, label};

const MAX_LOG_SIZE: usize = 1024 * 1024;

static SERIAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9]+$").unwrap());
static HOST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Za-z0-9.-]+|\[[0-9A-Fa-f:.]+\])(:[0-9]+)?$").unwrap());
static BUILD_TIMESTAMP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"BUILD_TIMESTAMP=([0-9]+)").unwrap());

/// Query parameters after Laravel's TrimStrings and ConvertEmptyStringsToNull middleware.
struct Query(HashMap<String, String>);

impl Query {
    fn parse(query: Option<&str>) -> Self {
        let pairs = form_urlencoded::parse(query.unwrap_or_default().as_bytes());
        Query(
            pairs
                .map(|(k, v)| (k.into_owned(), v.trim().to_string()))
                .filter(|(_, v)| !v.is_empty())
                .collect(),
        )
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    /// PHP truthiness: present, and not "0".
    fn truthy(&self, key: &str) -> Option<&str> {
        self.get(key).filter(|v| *v != "0")
    }
}

struct Device {
    state: AppState,
    serial: String,
    ip: Option<String>,
}

impl Device {
    async fn log(&self, board: Option<&str>, loglevel: &str, msg: &str) -> Result<()> {
        let now = self.state.clock.timestamp();
        models::log(&self.state.db, &now, &self.serial, board, self.ip.as_deref(), loglevel, msg).await?;
        Ok(())
    }

    async fn print_label(&self, label: &models::Label, mac: &str, board: Option<&str>) -> Result<()> {
        let printer = label.printer();
        let contents = label::render(&printer.template, mac, &self.serial, board.unwrap_or_default());
        let serial = self.serial.clone();
        let result = tokio::task::spawn_blocking(move || label::print(&printer, &serial, &contents)).await?;
        if let Err(msg) = result {
            self.log(board, "error", &msg).await?;
        }
        Ok(())
    }
}

pub async fn scriptexecute(State(state): State<AppState>, request: Request) -> Result<Response> {
    let query = Query::parse(request.uri().query());
    let Some(serial) = query.truthy("serial").filter(|s| SERIAL.is_match(s)).map(str::to_string) else {
        return Err(Error::Status(StatusCode::UNAUTHORIZED));
    };
    let ip = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip().to_canonical().to_string());
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .filter(|h| HOST.is_match(h))
        .map(str::to_string);
    let device = Device { state: state.clone(), serial, ip };

    if query.truthy("alldone").is_some() {
        return provisioning_complete(&device, &query).await;
    }

    let files = uploaded_files(request, &state).await?;
    if let Some(log) = files.get("log") {
        register_log(&device, &query, log).await
    } else if let Some(version) = files.get("eeprom_version") {
        register_firmware(&device, version).await
    } else {
        let host = host.ok_or(Error::Status(StatusCode::BAD_REQUEST))?;
        start_provisioning(&device, &query, &host).await
    }
}

async fn uploaded_files(request: Request, state: &AppState) -> Result<HashMap<String, Vec<u8>>> {
    let is_multipart = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("multipart/form-data"));
    let mut files = HashMap::new();
    if !is_multipart {
        return Ok(files);
    }

    let mut multipart = Multipart::from_request(request, state)
        .await
        .map_err(|_| Error::Status(StatusCode::BAD_REQUEST))?;
    while let Some(field) = multipart.next_field().await.map_err(|_| Error::Status(StatusCode::BAD_REQUEST))? {
        if field.file_name().is_none() {
            continue;
        }
        let name = field.name().unwrap_or_default().to_string();
        let bytes = field.bytes().await.map_err(|_| Error::Status(StatusCode::BAD_REQUEST))?;
        files.insert(name, bytes.to_vec());
    }
    Ok(files)
}

/// The jumper reading arrives with its bits inverted; shown as bits and decimal value.
fn board_from_jumper(jumper: &str) -> String {
    let bits: String = jumper.chars().map(|c| if c == '0' { '1' } else { '0' }).collect();
    let value = u128::from_str_radix(&bits, 2).unwrap_or(0);
    format!("{bits} ({value})")
}

/// Raspberry Pi MAC addresses derive from the serial; PHP's substr() clamps short serials.
fn mac_from_serial(serial: &str) -> String {
    let tail = |from_end: usize| {
        let start = serial.len().saturating_sub(from_end);
        serial.get(start..(start + 2).min(serial.len())).unwrap_or_default()
    };
    format!("b8:27:eb:{}:{}:{}", tail(6), tail(4), tail(2))
}

async fn switch_settings(state: &AppState) -> Result<Option<(String, String)>> {
    let ip = models::setting(&state.db, "ethernetswitch_ip").await?.filter(|v| !v.is_empty());
    let community = models::setting(&state.db, "ethernetswitch_snmp_community").await?.filter(|v| !v.is_empty());
    Ok(ip.zip(community))
}

async fn board_from_switch((ip, community): (String, String), mac: &str) -> Result<Option<String>> {
    let mac = mac.to_string();
    let board = tokio::task::spawn_blocking(move || {
        let session = SnmpSession::connect(&ip, &community).ok()?;
        EthernetSwitch::new(session).port_name_by_mac(&mac)
    })
    .await?;
    Ok(board)
}

fn partitions(storage: &str) -> (String, String) {
    if storage.ends_with(|c: char| c.is_ascii_digit()) {
        (format!("{storage}p1"), format!("{storage}p2"))
    } else {
        (format!("{storage}1"), format!("{storage}2"))
    }
}

/// The CM reaches us over USB at our IPv6 link-local address, which needs its zone.
fn server_for_device(host: &str) -> String {
    match host.find(']') {
        Some(end) if host.starts_with('[') => format!("{}%usb0{}", &host[..end], &host[end..]),
        _ => host.to_string(),
    }
}

async fn start_provisioning(device: &Device, query: &Query, host: &str) -> Result<Response> {
    let state = &device.state;
    let project = models::active_project(&state.db).await?;
    let image = match &project {
        Some(p) => models::image(&state.db, p.image_id).await?,
        None => None,
    };

    let mac_param = query.truthy("mac");
    let switch = switch_settings(state).await?;
    let board = match (mac_param, switch, query.truthy("inversejumper")) {
        (Some(mac), Some(switch), _) => board_from_switch(switch, mac).await?,
        (_, _, Some(jumper)) => Some(board_from_jumper(jumper)),
        _ => None,
    };
    let memory_in_gb = query
        .truthy("memorysize")
        .and_then(|m| m.parse::<f64>().ok())
        .map(|m| ((m + 200000.0) / 1024.0 / 1024.0).round() as i64);
    let storage_bytes = query.truthy("storagesize").and_then(|s| s.parse::<i64>().ok()).map(|s| s * 512);
    let mac = mac_param.map(str::to_string).unwrap_or_else(|| mac_from_serial(&device.serial));
    let now = state.clock.timestamp();

    sqlx::query(
        "INSERT INTO cms (serial, mac, model, memory_in_gb, storage, firmware, cid, csd, pre_script_output, \
         post_script_output, script_return_code, temp1, temp2, project_id, image_filename, image_sha256, \
         provisioning_board, provisioning_started_at, provisioning_complete_at, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, NULL, ?, NULL, ?, ?, ?, ?, ?, NULL, ?, ?) \
         ON CONFLICT (serial) DO UPDATE SET mac = excluded.mac, model = excluded.model, \
         memory_in_gb = excluded.memory_in_gb, storage = excluded.storage, firmware = excluded.firmware, \
         cid = excluded.cid, csd = excluded.csd, pre_script_output = NULL, post_script_output = NULL, \
         script_return_code = NULL, temp1 = excluded.temp1, temp2 = NULL, project_id = excluded.project_id, \
         image_filename = excluded.image_filename, image_sha256 = excluded.image_sha256, \
         provisioning_board = excluded.provisioning_board, \
         provisioning_started_at = excluded.provisioning_started_at, provisioning_complete_at = NULL, \
         updated_at = excluded.updated_at",
    )
    .bind(&device.serial)
    .bind(&mac)
    .bind(query.get("model"))
    .bind(memory_in_gb)
    .bind(storage_bytes)
    .bind(project.as_ref().and_then(|p| p.eeprom_firmware.as_deref()))
    .bind(query.get("cid"))
    .bind(query.get("csd"))
    .bind(query.get("temp"))
    .bind(project.as_ref().map(|p| p.id))
    .bind(image.as_ref().map(|i| i.filename.as_str()))
    .bind(image.as_ref().map(|i| i.sha256.as_str()))
    .bind(board.as_deref())
    .bind(&now)
    .bind(&now)
    .bind(&now)
    .execute(&state.db)
    .await?;

    let board = board.as_deref();
    let Some(project) = project else {
        device.log(board, "error", "Could not provision, because there is no active project").await?;
        return Ok(web::html("echo 'No active project set in CMprovisioning'"));
    };

    if let Some(message) = preflight_error(&project, image.as_ref(), storage_bytes) {
        device.log(board, "error", &message.log).await?;
        return Ok(web::html(format!("echo '{}'", message.reply)));
    }

    let mut preinstall = steps(models::project_scripts(&state.db, project.id, "preinstall").await?);
    let mut postinstall = steps(models::project_scripts(&state.db, project.id, "postinstall").await?);
    let server = server_for_device(host);

    if let Some(firmware) = &project.eeprom_firmware {
        let sha256 = models::setting(&state.db, "active_eeprom_sha256")
            .await?
            .ok_or(Error::Status(StatusCode::NOT_FOUND))?;
        preinstall.insert(0, flash_eeprom_step(firmware, &server, &sha256));
    }
    if let (true, Some(image)) = (project.verify, &image) {
        postinstall.insert(0, verify_step(&project.storage, image));
    }

    let label = models::label(&state.db, project.label_id).await?;
    let label_now = label.as_ref().filter(|_| project.label_moment == "preinstall");
    let mut msg = String::from("Provisioning started.");
    if label_now.is_some() {
        msg.push_str(" Printing label.");
    }
    if !preinstall.is_empty() {
        msg.push_str(" Starting preinstall scripts.");
    } else if image.is_some() {
        msg.push_str(" Starting to write image.");
    } else {
        msg.push_str(" No image to write.");
    }
    device.log(board, "info", &msg).await?;
    if let Some(label) = label_now {
        device.print_label(label, &mac, board).await?;
    }

    let (part1, part2) = partitions(&project.storage);
    let random = Alphanumeric.sample_string(&mut rand::rng(), 64);
    let body = script::render(&script::Context {
        serial: &device.serial,
        server: &server,
        storage: &project.storage,
        part1: &part1,
        part2: &part2,
        random: &random,
        preinstall: &preinstall,
        postinstall: &postinstall,
        flashes_eeprom: project.eeprom_firmware.is_some(),
        bootmode: query.get("bootmode"),
        image: image.as_ref().map(|i| script::Image {
            url: format!("http://{server}/uploads/{}", i.filename_on_server),
            extension: i.filename_extension.clone(),
        }),
        verify: project.verify,
    });
    Ok(web::text(body))
}

struct Preflight {
    log: String,
    reply: &'static str,
}

impl Preflight {
    fn same(msg: &'static str) -> Self {
        Preflight { log: msg.to_string(), reply: msg }
    }
}

fn preflight_error(project: &Project, image: Option<&models::Image>, storage_bytes: Option<i64>) -> Option<Preflight> {
    let image = image?;
    if project.verify {
        if image.uncompressed_sha256.as_deref().unwrap_or_default().is_empty() {
            return Some(Preflight::same("Verification enabled, but uncompressed SHA256 not computed yet, try again later..."));
        }
        if image.uncompressed_size.unwrap_or(0) % 512 != 0 {
            return Some(Preflight::same(
                "Image is not a valid disk image. Uncompressed size not dividable by sector size of 512 bytes.",
            ));
        }
    }
    if project.storage == "/dev/mmcblk0" {
        let Some(available) = storage_bytes else {
            return Some(Preflight::same("Missing eMMC/SD card."));
        };
        if let Some(size) = image.uncompressed_size.filter(|&size| size != 0 && available < size) {
            return Some(Preflight {
                log: format!(
                    "Image does not fit in storage. Uncompressed image size: {size} bytes. Available space: {available} bytes."
                ),
                reply: "Image does not fit in storage.",
            });
        }
    }
    None
}

fn steps(scripts: Vec<models::Script>) -> Vec<Step> {
    scripts.into_iter().map(|s| Step { id: s.id, name: s.name, bg: s.bg, script: s.script }).collect()
}

fn flash_eeprom_step(firmware: &str, server: &str, sha256: &str) -> Step {
    Step {
        id: 0,
        name: format!("Flash EEPROM firmware ({firmware})"),
        bg: false,
        script: format!(
            "#!/bin/sh\nset -e\n\
             curl --retry 10 --silent --show-error -g -o pieeprom.bin \"http://{server}/uploads/pieeprom.bin\"\n\
             echo \"{sha256}  pieeprom.bin\" | sha256sum -c\n\
             flashrom -p \"linux_spi:dev=/dev/spidev0.0,spispeed=16000\" -w \"pieeprom.bin\"\n"
        ),
    }
}

fn verify_step(storage: &str, image: &models::Image) -> Step {
    let size = image.uncompressed_size.unwrap_or(0);
    let dd = if size % 1048576 == 0 {
        format!("dd if={storage} bs=1M count={}", size / 1048576)
    } else {
        format!("dd if={storage} count={}", size / 512)
    };
    let expected = image.uncompressed_sha256.as_deref().unwrap_or_default();
    Step {
        id: 0,
        name: "Verifying written image".into(),
        bg: false,
        script: format!(
            "#!/bin/sh\nset -e\n\
             sync; echo 3 > /proc/sys/vm/drop_caches\n\
             READ_SHA256=$({dd} | sha256sum | awk '{{print $1}}')\n\
             echo Computed SHA256: \"$READ_SHA256\"\n\
             if [ \"$READ_SHA256\" = \"{expected}\" ]; then echo Verification successful!; else echo Verification failed; exit 2; fi\n"
        ),
    }
}

async fn existing_cm(device: &Device) -> Result<models::Cm> {
    models::cm_by_serial(&device.state.db, &device.serial)
        .await?
        .ok_or(Error::Status(StatusCode::NOT_FOUND))
}

async fn cm_project(device: &Device, cm: &models::Cm) -> Result<Option<Project>> {
    match cm.project_id {
        Some(id) => Ok(models::project(&device.state.db, id).await?),
        None => Ok(None),
    }
}

async fn provisioning_complete(device: &Device, query: &Query) -> Result<Response> {
    let db = &device.state.db;
    let cm = existing_cm(device).await?;
    let project = cm_project(device, &cm).await?;
    let now = device.state.clock.timestamp();
    sqlx::query("UPDATE cms SET provisioning_complete_at = ?, temp2 = ?, updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(query.get("temp"))
        .bind(&now)
        .bind(cm.id)
        .execute(db)
        .await?;

    let label = match project.as_ref().filter(|p| p.label_moment == "postinstall") {
        Some(p) => models::label(db, p.label_id).await?,
        None => None,
    };
    let mut msg = String::from("Provisioning completed.");
    if query.truthy("verify").is_some() {
        msg.push_str(" Verification successful.");
    }
    if label.is_some() {
        msg.push_str(" Printing label.");
    }
    let board = cm.provisioning_board.as_deref();
    device.log(board, "info", &msg).await?;
    if let Some(label) = &label {
        device.print_label(label, &cm.mac, board).await?;
    }
    Ok(web::html(""))
}

async fn register_log(device: &Device, query: &Query, log: &[u8]) -> Result<Response> {
    let db = &device.state.db;
    let cm = existing_cm(device).await?;
    let truncated = log.len() > MAX_LOG_SIZE;
    let mut log = String::from_utf8_lossy(&log[..log.len().min(MAX_LOG_SIZE)]).into_owned();
    if truncated {
        log.push_str("\nLog was bigger than max allowed size. Truncated.");
    }
    let phase = query.get("phase").unwrap_or_default();
    let retcode = query.get("retcode");
    let failed = query.truthy("retcode").is_some();
    let clears_image = failed && (phase == "dd" || phase == "preinstall");
    let now = device.state.clock.timestamp();

    sqlx::query(
        "UPDATE cms SET script_return_code = ?, \
         pre_script_output = CASE WHEN ? = 'preinstall' THEN ? ELSE pre_script_output END, \
         post_script_output = CASE WHEN ? = 'postinstall' THEN ? ELSE post_script_output END, \
         image_filename = CASE WHEN ? THEN NULL ELSE image_filename END, \
         image_sha256 = CASE WHEN ? THEN NULL ELSE image_sha256 END, \
         updated_at = ? WHERE id = ?",
    )
    .bind(retcode)
    .bind(phase)
    .bind(&log)
    .bind(phase)
    .bind(&log)
    .bind(clears_image)
    .bind(clears_image)
    .bind(&now)
    .bind(cm.id)
    .execute(db)
    .await?;

    let board = cm.provisioning_board.as_deref();
    if failed {
        let msg = format!("Error during {phase}. Return code {}. Script output:\n\n{log}", retcode.unwrap_or_default());
        device.log(board, "error", &msg).await?;
    } else if phase == "preinstall" {
        let has_image = match cm_project(device, &cm).await? {
            Some(p) => models::image(db, p.image_id).await?.is_some(),
            None => false,
        };
        let msg = if has_image { "Preinstall script complete. Starting to write image." } else { "Preinstall script complete." };
        device.log(board, "info", msg).await?;
    }
    Ok(web::html(""))
}

async fn register_firmware(device: &Device, version: &[u8]) -> Result<Response> {
    let cm = existing_cm(device).await?;
    let mut firmware = String::from_utf8_lossy(version).into_owned();
    let built = BUILD_TIMESTAMP
        .captures(&firmware)
        .and_then(|c| c[1].parse::<i64>().ok())
        .and_then(|ts| chrono::DateTime::from_timestamp(ts, 0));
    if let Some(built) = built {
        firmware.push_str(&built.to_rfc2822());
    }
    let now = device.state.clock.timestamp();
    sqlx::query("UPDATE cms SET firmware = ?, updated_at = ? WHERE id = ?")
        .bind(&firmware)
        .bind(&now)
        .bind(cm.id)
        .execute(&device.state.db)
        .await
        .context("storing firmware version")?;
    Ok(web::html(""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverts_jumper_bits() {
        assert_eq!(board_from_jumper("1101"), "0010 (2)");
    }

    #[test]
    fn derives_mac_from_serial() {
        assert_eq!(mac_from_serial("10000000abcdef12"), "b8:27:eb:cd:ef:12");
    }

    #[test]
    fn partition_names() {
        assert_eq!(partitions("/dev/mmcblk0"), ("/dev/mmcblk0p1".into(), "/dev/mmcblk0p2".into()));
        assert_eq!(partitions("/dev/sda"), ("/dev/sda1".into(), "/dev/sda2".into()));
    }

    #[test]
    fn adds_usb_zone_to_ipv6_host() {
        assert_eq!(server_for_device("[fe80::1]"), "[fe80::1%usb0]");
        assert_eq!(server_for_device("[fe80::1]:8080"), "[fe80::1%usb0]:8080");
        assert_eq!(server_for_device("172.20.0.1"), "172.20.0.1");
    }

    #[test]
    fn query_trims_and_drops_empty_values() {
        let q = Query::parse(Some("serial=%20abc%20&storagesize=&a+b=c+d&zero=0"));
        assert_eq!(q.get("serial"), Some("abc"));
        assert_eq!(q.get("storagesize"), None);
        assert_eq!(q.get("a b"), Some("c d"));
        assert_eq!(q.truthy("zero"), None);
    }
}
