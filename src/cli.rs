//! Interactive administration commands, replacing `artisan auth:create-user` and
//! `artisan ethernetswitch:configure`.

use std::io::{BufRead, Write};

use anyhow::Result;

use crate::switch::{EthernetSwitch, Snmp, SnmpSession};
use crate::{AppState, auth, models};

pub trait Prompt {
    fn ask(&mut self, question: &str) -> Result<String>;
    fn secret(&mut self, question: &str) -> Result<String>;
    fn say(&mut self, message: &str);
    fn error(&mut self, message: &str);
}

pub struct Terminal;

impl Prompt for Terminal {
    fn ask(&mut self, question: &str) -> Result<String> {
        print!("{question}: ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        if std::io::stdin().lock().read_line(&mut line)? == 0 {
            anyhow::bail!("input ended");
        }
        Ok(line.trim().to_string())
    }

    fn secret(&mut self, question: &str) -> Result<String> {
        Ok(rpassword::prompt_password(format!("{question}: "))?)
    }

    fn say(&mut self, message: &str) {
        println!("{message}");
    }

    fn error(&mut self, message: &str) {
        eprintln!("{message}");
    }
}

pub async fn create_user(state: &AppState, prompt: &mut impl Prompt) -> Result<()> {
    let email = loop {
        let email = prompt.ask("E-mail address of user")?;
        if !auth::looks_like_email(&email) || email.len() > 255 {
            prompt.error("The email must be a valid email address.");
        } else if auth::user_by_email(&state.db, &email).await?.is_some() {
            prompt.error("The email has already been taken.");
        } else {
            break email;
        }
    };
    let name = loop {
        let name = prompt.ask("Full name of user")?;
        if name.is_empty() || name.len() > 255 {
            prompt.error("The name field is required and may not be longer than 255 characters.");
        } else {
            break name;
        }
    };
    let password = loop {
        let password = prompt.secret("Password")?;
        if password.chars().count() < auth::MIN_PASSWORD {
            prompt.error(&format!("The password must be at least {} characters.", auth::MIN_PASSWORD));
            continue;
        }
        if prompt.secret("Repeat password")? == password {
            break password;
        }
        prompt.error("Passwords do not match");
    };

    let now = state.clock.timestamp();
    sqlx::query("INSERT INTO users (name, email, password, created_at, updated_at) VALUES (?, ?, ?, ?, ?)")
        .bind(&name)
        .bind(&email)
        .bind(auth::hash_password(&password).await?)
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await?;
    prompt.say("User created");
    Ok(())
}

pub async fn configure_switch(state: &AppState, prompt: &mut impl Prompt) -> Result<bool> {
    configure_switch_with(state, prompt, |ip, community| SnmpSession::connect(ip, community).ok()).await
}

/// Saves the switch only once it answered with a MAC table, so a typo cannot break provisioning.
pub async fn configure_switch_with<S: Snmp + Send + 'static>(
    state: &AppState,
    prompt: &mut impl Prompt,
    connect: impl FnOnce(&str, &str) -> Option<S> + Send + 'static,
) -> Result<bool> {
    let ip = prompt.ask("Ethernet switch IP-address (leave empty to disable module)")?;
    if ip.is_empty() {
        models::delete_setting(&state.db, "ethernetswitch_ip").await?;
        models::delete_setting(&state.db, "ethernetswitch_snmp_community").await?;
        prompt.say("Disabled Ethernet switch integration");
        return Ok(true);
    }
    let community = loop {
        let community = prompt.ask("SNMP v2c community name")?;
        if !community.is_empty() {
            break community;
        }
    };

    prompt.say("Trying to communicate with Ethernet switch...");
    let (switch_ip, switch_community) = (ip.clone(), community.clone());
    let table = tokio::task::spawn_blocking(move || EthernetSwitch::new(connect(&switch_ip, &switch_community)?).mac_to_port_name())
        .await?
        .filter(|table| !table.is_empty());
    let Some(table) = table else {
        prompt.error("Error communicating with switch. Unable to connect or does not support BRIDGE-MIB.");
        prompt.error("Settings unchanged.");
        return Ok(false);
    };

    let rule = "=================================";
    prompt.say(&format!("{rule}\nMac -> interface name/alias table\n{rule}"));
    for (mac, port) in &table {
        prompt.say(&format!("{mac} {port}"));
    }
    prompt.say(rule);
    models::set_setting(&state.db, "ethernetswitch_ip", &ip).await?;
    models::set_setting(&state.db, "ethernetswitch_snmp_community", &community).await?;
    prompt.say("\nCommunication successful. Enabled Ethernet switch integration.");
    Ok(true)
}
