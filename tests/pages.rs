mod common;

use axum::http::{StatusCode, header};
use cmprovision::models;
use common::App;

const PAGES: &[&str] = &["/dashboard", "/cms", "/images", "/projects", "/projects/new", "/scripts", "/scripts/new", "/labels", "/labels/new", "/firmware", "/settings", "/settings/static-ips/new"];

#[tokio::test]
async fn every_page_renders_for_users_and_redirects_guests() {
    let app = App::new().await;
    let mut guest = app.browser();
    let mut user = app.logged_in().await;
    for path in PAGES {
        assert_eq!(guest.get(path).await.location.as_deref(), Some("/login"), "{path} as guest");
        let page = user.get(path).await;
        assert_eq!(page.status, StatusCode::OK, "{path}: {}", page.body);
    }
}

async fn seed_cm(app: &App, serial: &str) {
    sqlx::query(
        "INSERT INTO cms (serial, mac, model, project_id, provisioning_started_at, provisioning_complete_at, pre_script_output) \
         VALUES (?, 'b8:27:eb:00:00:01', 'CM4', NULL, '2024-01-02 03:04:05', '2024-01-02 03:06:10', 'two words, \"quoted\"')",
    )
    .bind(serial)
    .execute(&app.state.db)
    .await
    .unwrap();
}

#[tokio::test]
async fn cms_list_detail_export_and_delete() {
    let app = App::new().await;
    let mut b = app.logged_in().await;
    seed_cm(&app, "10000000aaaaaaaa").await;

    let page = b.get("/cms").await;
    assert!(page.body.contains("10000000aaaaaaaa"));

    let page = b.get("/cms/1").await;
    assert!(page.body.contains("2m 5s"), "duration shown");
    assert!(page.body.contains("Pre-install script output"));

    let page = b.get("/cms/export.csv?project=0").await;
    assert_eq!(page.headers[header::CONTENT_TYPE], "text/csv; charset=UTF-8");
    assert!(page.headers[header::CONTENT_DISPOSITION].to_str().unwrap().starts_with("attachment; filename=export-cm-"));
    let mut lines = page.body.lines();
    assert!(lines.next().unwrap().starts_with("id,serial,mac,model,"));
    assert!(lines.next().unwrap().contains(r#","two words, ""quoted""","#));

    let redirect = b.post("/cms/1/delete", &[]).await;
    let page = b.follow(redirect).await;
    assert!(page.body.contains("Cm deleted."));
    assert!(!page.body.contains("10000000aaaaaaaa"));
}

#[tokio::test]
async fn image_upload_and_delete_through_the_page() {
    let app = App::new().await;
    let mut b = app.logged_in().await;

    let redirect = b.upload("/addImage", "image", "os.img.zip", b"nope").await;
    let page = b.follow(redirect).await;
    assert!(page.body.contains("Only .gz, .bz2 and .xz images are supported"));

    let redirect = b.upload("/addImage", "image", "os.img.gz", b"not really gzip").await;
    let page = b.follow(redirect).await;
    assert!(page.body.contains("Image added.") && page.body.contains("os.img.gz"));
    let image = models::images(&app.state.db).await.unwrap().remove(0);
    assert!(app.path("public/uploads").join(&image.filename_on_server).exists());

    let redirect = b.post(&format!("/images/{}/delete", image.id), &[]).await;
    let page = b.follow(redirect).await;
    assert!(page.body.contains("Image deleted."));
    assert!(!app.path("public/uploads").join(&image.filename_on_server).exists());
}

fn install_firmware(app: &App) {
    let dir = app.path("storage/app/firmware/latest");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/eeprom/pieeprom.bin"), dir.join("pieeprom-2026-10-15.bin")).unwrap();
}

#[tokio::test]
async fn project_validation() {
    let app = App::new().await;
    let mut b = app.logged_in().await;

    let page = b
        .post("/projects", &[("name", ""), ("storage", "/dev/mmcblk0; reboot"), ("label_moment", "preinstall"), ("label_id", ""), ("firmware", "latest/missing.bin")])
        .await;

    assert_eq!(page.status, StatusCode::UNPROCESSABLE_ENTITY);
    for message in ["The name field is required", "Enter a device path", "The selected firmware is not available.", "Choose a label to print."] {
        assert!(page.body.contains(message), "missing {message:?}");
    }
}

#[tokio::test]
async fn activating_a_project_stages_patched_eeprom_firmware() {
    let app = App::new().await;
    install_firmware(&app);
    let mut b = app.logged_in().await;

    let defaults = b.get("/firmware/bootconf?path=latest/pieeprom-2026-10-15.bin").await.body;
    assert_eq!(defaults.as_bytes(), include_bytes!("fixtures/eeprom/bootconf.txt"));

    let settings = std::str::from_utf8(include_bytes!("fixtures/eeprom/patched-settings.txt")).unwrap();
    let page = b
        .post(
            "/projects",
            &[
                ("name", "Line 1"),
                ("storage", "/dev/mmcblk0"),
                ("image_id", ""),
                ("firmware", "latest/pieeprom-2026-10-15.bin"),
                ("eeprom_settings", &settings.replace('\n', "\r\n")),
                ("label_moment", "never"),
                ("scripts", "1"),
                ("scripts", "3"),
                ("active", "on"),
            ],
        )
        .await;
    assert_eq!(page.location.as_deref(), Some("/projects"), "{}", page.body);

    let project = models::active_project(&app.state.db).await.unwrap().unwrap();
    assert_eq!(project.name, "Line 1");
    assert_eq!(models::project_script_ids(&app.state.db, project.id).await.unwrap(), [1, 3]);
    let staged = std::fs::read(app.path("public/uploads/pieeprom.bin")).unwrap();
    assert!(staged == include_bytes!("fixtures/eeprom/pieeprom-patched.bin"), "settings patched like PHP, with CRLF removed");
    let sha = models::setting(&app.state.db, "active_eeprom_sha256").await.unwrap().unwrap();
    assert_eq!(sha, hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&staged)));

    b.post("/projects/1/delete", &[]).await;
    assert!(models::active_project_id(&app.state.db).await.unwrap().is_none());
}

#[tokio::test]
async fn project_without_firmware_removes_staged_image() {
    let app = App::new().await;
    std::fs::create_dir_all(app.path("public/uploads")).unwrap();
    std::fs::write(app.path("public/uploads/pieeprom.bin"), b"old").unwrap();
    let mut b = app.logged_in().await;

    b.post("/projects", &[("name", "Plain"), ("storage", "/dev/sda"), ("label_moment", "never"), ("active", "on")]).await;

    assert!(!app.path("public/uploads/pieeprom.bin").exists());
}

#[tokio::test]
async fn scripts_crud_and_heredoc_guard() {
    let app = App::new().await;
    let mut b = app.logged_in().await;

    let page = b.post("/scripts", &[("name", "Bad"), ("script_type", "postinstall"), ("priority", "10"), ("script", "echo\nCMPROVISIONINGEOF\nrm -rf /")]).await;
    assert!(page.body.contains("may not contain a line reading CMPROVISIONINGEOF"));

    let page = b.post("/scripts", &[("name", "Resize ext4 partition"), ("script_type", "postinstall"), ("priority", "10"), ("script", "x")]).await;
    assert!(page.body.contains("The name has already been taken."), "seeded script name");

    let page = b.post("/scripts", &[("name", "Hello"), ("script_type", "preinstall"), ("priority", "5"), ("bg", "on"), ("script", "echo hi\r\n")]).await;
    assert_eq!(page.location.as_deref(), Some("/scripts"));
    let script = models::scripts(&app.state.db).await.unwrap().into_iter().find(|s| s.name == "Hello").unwrap();
    assert_eq!((script.script_type.as_str(), script.priority, script.bg, script.script.as_str()), ("preinstall", 5, true, "echo hi\n"));

    b.post(&format!("/scripts/{}", script.id), &[("name", "Hello"), ("script_type", "postinstall"), ("priority", "7"), ("script", "echo bye")]).await;
    let script = models::script(&app.state.db, script.id).await.unwrap().unwrap();
    assert_eq!((script.script_type.as_str(), script.bg), ("postinstall", false));
}

#[tokio::test]
async fn label_password_is_masked_and_kept() {
    let app = App::new().await;
    let mut b = app.logged_in().await;

    let page = b.get("/labels/1/edit").await;
    assert!(page.body.contains("****************") && !page.body.contains("ftpprint"), "seeded password masked");

    let fields = [
        ("name", "Datamatrix on Brady/CAB printer"),
        ("printer_type", "ftp"),
        ("ftp_hostname", "printer.local"),
        ("ftp_username", "print"),
        ("ftp_password", "****************"),
        ("file_extension", "jscript"),
        ("template", "$mac"),
    ];
    assert_eq!(b.post("/labels/1", &fields).await.location.as_deref(), Some("/labels"));
    let label = models::label(&app.state.db, Some(1)).await.unwrap().unwrap();
    assert_eq!((label.ftp_hostname.as_deref(), label.ftp_password.as_deref()), (Some("printer.local"), Some("ftpprint")));
}

#[tokio::test]
async fn label_test_print_uses_unsaved_form() {
    let app = App::new().await;
    let mut b = app.logged_in().await;
    let out = app.path("label.out");
    let command = format!("cp $file '{}'", out.display());

    let page = b
        .post("/labels/test", &[("name", "Cmd"), ("printer_type", "command"), ("print_command", &command), ("file_extension", "txt"), ("template", "$serial $mac $provisionboard")])
        .await;

    assert!(page.body.contains("Printed test label"), "{}", page.body);
    assert_eq!(std::fs::read_to_string(out).unwrap(), "1000000012345678 11:22:33:44:55:66 000 (0)");
}

#[tokio::test]
async fn static_ips_rewrite_dnsmasq_conf_on_add_and_delete() {
    let app = App::new().await;
    std::fs::create_dir_all(app.path("etc")).unwrap();
    std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/etc/dnsmasq.conf"), app.path("etc/dnsmasq.conf")).unwrap();
    let mut b = app.logged_in().await;

    let page = b.get("/settings/static-ips/new").await;
    assert!(page.body.contains("value=\"172.20.0.2\""));

    let page = b.post("/settings/static-ips", &[("ip", "172.20.0.300"), ("mac", "nope")]).await;
    assert!(page.body.contains("valid IPv4 address") && page.body.contains("mac format is invalid"));

    b.post("/settings/static-ips", &[("ip", "172.20.0.2"), ("mac", "DC:A6:32:00:00:01")]).await;
    let conf = std::fs::read_to_string(app.path("etc/dnsmasq.conf")).unwrap();
    assert!(conf.ends_with("dhcp-host=dc:a6:32:00:00:01,set:client_is_a_pi,172.20.0.2\n"));

    let page = b.post("/settings/static-ips", &[("ip", "172.20.0.3"), ("mac", "dc:a6:32:00:00:01")]).await;
    assert!(page.body.contains("The mac has already been taken."));

    b.post("/settings/static-ips/1/delete", &[]).await;
    assert!(!std::fs::read_to_string(app.path("etc/dnsmasq.conf")).unwrap().contains("dhcp-host="));
}

#[tokio::test]
async fn service_logs_only_for_known_services() {
    let app = App::new().await;
    let mut b = app.logged_in().await;
    assert_eq!(b.get("/settings/log/cmprovision-dnsmasq").await.status, StatusCode::OK);
    assert_eq!(b.get("/settings/log/sshd").await.status, StatusCode::NOT_FOUND);
}
