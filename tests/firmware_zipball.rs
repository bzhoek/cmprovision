//! Extracts a real rpi-eeprom zipball. Too large to commit, so it is opt-in:
//! download it to target/rpi-eeprom.zip and run `cargo test -- --ignored`.

use std::path::Path;

use cmprovision::firmware;

#[test]
#[ignore]
fn extracts_upstream_zipball() {
    let zip = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/target/rpi-eeprom.zip"));
    let dir = tempfile::tempdir().unwrap();

    firmware::extract_zipball(std::fs::File::open(zip).unwrap(), dir.path()).unwrap();

    let listed = firmware::list(dir.path());
    let channels: std::collections::BTreeSet<_> = listed.iter().map(|f| f.channel.as_str()).collect();
    assert!(channels.contains("default") && channels.contains("latest"), "{channels:?}");
    let newest = &listed.iter().find(|f| f.channel == "latest").unwrap().path;
    let legacy = newest.replace("latest/", "stable/");
    assert!(firmware::resolve(dir.path(), &legacy).is_some(), "{legacy} resolves through the stable alias");
    let image = std::fs::read(firmware::resolve(dir.path(), newest).unwrap()).unwrap();
    assert!(cmprovision::eeprom::settings(&image).is_ok(), "{newest} has a readable bootconf.txt");
    eprintln!("{} images in {channels:?}, newest {newest}", listed.len());
}
