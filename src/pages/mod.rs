pub mod cms;
pub mod dashboard;
pub mod firmware;
pub mod images;
pub mod labels;
pub mod profile;
pub mod projects;
pub mod scripts;
pub mod settings;

/// HTML checkboxes are only submitted when checked.
pub fn checked(value: &Option<String>) -> bool {
    value.is_some()
}

/// Gigabytes with one decimal, as the PHP pages showed sizes.
pub fn gigabytes(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1e9)
}
