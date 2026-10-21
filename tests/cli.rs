mod common;

use std::collections::VecDeque;

use anyhow::Result;
use cmprovision::cli::{self, Prompt};
use cmprovision::switch::{Snmp, SnmpValue};
use cmprovision::{auth, models};
use common::App;

#[derive(Default)]
struct Script {
    answers: VecDeque<&'static str>,
    output: Vec<String>,
    errors: Vec<String>,
}

impl Script {
    fn new(answers: &[&'static str]) -> Self {
        Script { answers: answers.iter().copied().collect(), ..Script::default() }
    }
}

impl Prompt for Script {
    fn ask(&mut self, _: &str) -> Result<String> {
        self.answers.pop_front().map(str::to_string).ok_or_else(|| anyhow::anyhow!("ran out of answers"))
    }

    fn secret(&mut self, question: &str) -> Result<String> {
        self.ask(question)
    }

    fn say(&mut self, message: &str) {
        self.output.push(message.to_string());
    }

    fn error(&mut self, message: &str) {
        self.errors.push(message.to_string());
    }
}

#[tokio::test]
async fn create_user_retries_until_valid() {
    let app = App::new().await;
    let mut prompt = Script::new(&[
        "not an email",
        common::EMAIL,
        "new@example.com",
        "",
        "New Admin",
        "short",
        "long enough",
        "different",
        "long enough",
        "long enough",
    ]);

    cli::create_user(&app.state, &mut prompt).await.unwrap();

    assert_eq!(
        prompt.errors,
        [
            "The email must be a valid email address.",
            "The email has already been taken.",
            "The name field is required and may not be longer than 255 characters.",
            "The password must be at least 8 characters.",
            "Passwords do not match",
        ]
    );
    let user = auth::user_by_email(&app.state.db, "new@example.com").await.unwrap().unwrap();
    assert_eq!(user.name, "New Admin");
    assert!(auth::verify_password("long enough", &user.password).await);
}

#[derive(Clone)]
struct Switch {
    answers: bool,
}

impl Snmp for Switch {
    fn get(&mut self, oid: &[u64]) -> Option<SnmpValue> {
        match oid.last()? {
            1 if oid.len() == 12 && oid[10] == 2 => Some(SnmpValue::Integer(101)),
            101 => Some(SnmpValue::Bytes(b"ge-0/0/1".to_vec())),
            _ => None,
        }
    }

    fn walk(&mut self, _: &[u64]) -> Option<Vec<(Vec<u64>, SnmpValue)>> {
        if !self.answers {
            return None;
        }
        let index = [0xdc, 0xa6, 0x32, 0, 0, 1];
        let oid = |column: u64| [&[1, 3, 6, 1, 2, 1, 17, 4, 3, 1, column][..], &index[..]].concat();
        Some(vec![(oid(1), SnmpValue::Bytes(index.iter().map(|&b| b as u8).collect())), (oid(2), SnmpValue::Integer(1))])
    }
}

#[tokio::test]
async fn configure_switch_saves_only_after_a_successful_walk() {
    let app = App::new().await;

    let mut prompt = Script::new(&["10.0.0.2", "", "public"]);
    let saved = cli::configure_switch_with(&app.state, &mut prompt, |_, _| Some(Switch { answers: false })).await.unwrap();
    assert!(!saved);
    assert!(models::setting(&app.state.db, "ethernetswitch_ip").await.unwrap().is_none());

    let mut prompt = Script::new(&["10.0.0.2", "public"]);
    let saved = cli::configure_switch_with(&app.state, &mut prompt, |ip, community| {
        assert_eq!((ip, community), ("10.0.0.2", "public"));
        Some(Switch { answers: true })
    })
    .await
    .unwrap();
    assert!(saved);
    assert!(prompt.output.iter().any(|line| line == "dc:a6:32:00:00:01 ge-0/0/1"), "{:?}", prompt.output);
    assert_eq!(models::setting(&app.state.db, "ethernetswitch_snmp_community").await.unwrap().as_deref(), Some("public"));

    let mut prompt = Script::new(&[""]);
    cli::configure_switch_with(&app.state, &mut prompt, |_, _| None::<Switch>).await.unwrap();
    assert!(models::setting(&app.state.db, "ethernetswitch_ip").await.unwrap().is_none());
    assert_eq!(prompt.output, ["Disabled Ethernet switch integration"]);
}
