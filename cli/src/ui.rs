//! Channel-backed full-screen setup wizard. Slow setup and GPG work stay off the renderer.
mod model;
#[cfg(any(windows, test))]
mod mouse_input;
#[cfg(any(windows, test))]
mod terminal;
mod view;
pub(crate) mod web;
use crate::gpg::SigningKey;
use anyhow::{anyhow, bail, Result};
use iocraft::prelude::*;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    sync::{mpsc, Arc, Mutex, Once},
};
use zeroize::Zeroizing;

thread_local! { static PRIVATE_WORKER: Cell<bool> = const { Cell::new(false) }; }
thread_local! { static CLIENT: RefCell<Option<Client>> = const { RefCell::new(None) }; }
struct Client {
    sink: mpsc::Sender<Update>,
    notes: String,
    web: bool,
    browser_vault: Option<Arc<Mutex<BrowserVault>>>,
    // Some(empty) means the browser submitted a scoped setup form. In that
    // mode blank fields reuse existing vault values without extra questions.
    prepared: Option<BTreeMap<String, Zeroizing<String>>>,
}
// Shared by the local HTTP session and its worker. Locking the browser
// session revokes the worker's ability to reuse a vault passphrase that has
// not yet been consumed by an in-flight operation.
struct BrowserVault {
    unlocked: bool,
    credential: Option<Zeroizing<String>>,
}
impl BrowserVault {
    fn locked() -> Self {
        Self { unlocked: false, credential: None }
    }

    fn lock(&mut self) {
        self.unlocked = false;
        self.credential = None;
    }

    fn unlock(&mut self, credential: Option<Zeroizing<String>>) {
        self.unlocked = true;
        self.credential = credential;
    }
}
enum Update {
    Question(Question, String, mpsc::Sender<Answer>),
    Notes(String),
    Finished(String, bool),
    Keys(u64, Result<Vec<SigningKey>>),
    Clipboard(u64, usize, Result<String, String>),
    Phase(&'static str),
}

pub enum Answer {
    Text(String),
    Confirm(bool),
    Menu(Menu),
    Key(KeyChoice),
    Fields(Vec<String>),
    Cancel,
}
pub enum Menu {
    Setup(Vec<usize>),
    ScopedSetup {
        module_id: String,
        selected: Vec<usize>,
        prepared: BTreeMap<String, Zeroizing<String>>,
    },
    ViewVault,
    RemakeVault,
    Exit,
}
pub enum KeyChoice {
    Create,
    Existing(SigningKey),
}
#[derive(Clone)]
pub struct Field {
    pub label: String,
    pub hint: String,
    pub value: String,
    pub secret: bool,
}
#[derive(Clone)]
pub(crate) struct ModuleChoice {
    pub id: String,
    pub path: String,
    pub kind: crate::discovery::Kind,
    pub directory: String,
    pub label: String,
    pub description: String,
    pub selected: bool,
}
#[derive(Clone)]
pub(crate) struct OperationChoice {
    pub id: String,
    pub label: String,
    pub description: String,
}
enum Question {
    Text {
        label: String,
        default: Option<String>,
        secret: bool,
    },
    Confirm(String),
    Menu {
        operations: Vec<OperationChoice>,
        first_run: bool,
    },
    Modules(Vec<ModuleChoice>),
    Key,
    Form {
        title: String,
        fields: Vec<Field>,
        validate: fn(&[String]) -> Result<()>,
    },
    Saved(String),
}

pub fn active() -> bool {
    CLIENT.with(|client| client.borrow().is_some())
}

pub fn browser_active() -> bool {
    CLIENT.with(|client| client.borrow().as_ref().is_some_and(|client| client.web))
}

pub(crate) fn browser_vault_credential() -> Option<Zeroizing<String>> {
    CLIENT.with(|client| {
        let client = client.borrow();
        let vault = client.as_ref()?.browser_vault.as_ref()?.lock().ok()?;
        vault.unlocked.then(|| vault.credential.clone()).flatten()
    })
}

pub(crate) fn remember_browser_vault_credential(credential: Zeroizing<String>) {
    CLIENT.with(|client| {
        let client = client.borrow();
        if let Some(vault) = client.as_ref().and_then(|client| client.browser_vault.as_ref()) {
            if let Ok(mut vault) = vault.lock() {
                if vault.unlocked { vault.credential = Some(credential); }
            }
        }
    });
}

pub(crate) fn forget_browser_vault_credential() {
    CLIENT.with(|client| {
        let client = client.borrow();
        if let Some(vault) = client.as_ref().and_then(|client| client.browser_vault.as_ref()) {
            if let Ok(mut vault) = vault.lock() { vault.credential = None; }
        }
    });
}

pub fn phase(value: &'static str) {
    CLIENT.with(|client| {
        if let Some(client) = client.borrow().as_ref() {
            let _ = client.sink.send(Update::Phase(value));
        }
    });
}

pub fn message(value: String) {
    CLIENT.with(|client| {
        let mut client = client.borrow_mut();
        if let Some(client) = client.as_mut() {
            client.notes.push_str(&value);
            client.notes.push('\n');
            let _ = client.sink.send(Update::Notes(client.notes.clone()));
        } else {
            println!("{value}");
        }
    });
}

fn request(question: Question) -> Result<Answer> {
    let (send, receive) = mpsc::channel();
    CLIENT.with(|client| -> Result<()> {
        let mut client = client.borrow_mut();
        let client = client
            .as_mut()
            .ok_or_else(|| anyhow!("Setup UI is not running."))?;
        let notes = std::mem::take(&mut client.notes);
        client
            .sink
            .send(Update::Question(question, notes, send))
            .map_err(|_| anyhow!("Setup closed."))
    })?;
    receive.recv().map_err(|_| anyhow!("Setup closed."))
}

pub(crate) fn prepare_browser_answers(answers: BTreeMap<String, Zeroizing<String>>) {
    CLIENT.with(|client| {
        let mut client = client.borrow_mut();
        if let Some(client) = client.as_mut().filter(|client| client.web) {
            client.prepared = Some(answers);
        }
    });
}

pub(crate) fn has_prepared_answer(label: &str) -> bool {
    CLIENT.with(|client| {
        let client = client.borrow();
        client.as_ref().and_then(|client| client.prepared.as_ref())
            .is_some_and(|answers| answers.contains_key(label))
    })
}

fn prepared_key(label: &str) -> &str {
    // The saved-value question has a shorter name than its actual text input.
    if label == "Android application IDs" {
        "Android application IDs (comma-separated installed package names)"
    } else {
        label
    }
}

pub fn prompt(label: &str, default: Option<&str>, secret: bool) -> Result<String> {
    let prepared = CLIENT.with(|client| {
        let mut client = client.borrow_mut();
        client.as_mut()?.prepared.as_mut()?.remove(label)
    });
    if let Some(value) = prepared {
        // Worker generated vault passphrases and review prompts must never be
        // supplied by an advance form. The HTTP layer also restricts labels.
        if !label.starts_with("Unlock local setup vault")
            && !label.starts_with("Create vault password")
            && !label.starts_with("Confirm vault password")
        {
            return Ok(if secret { value.to_string() } else { value.trim().to_owned() });
        }
    }
    match request(Question::Text {
        label: label.into(),
        default: default.map(str::to_owned),
        secret,
    })? {
        Answer::Text(value) => Ok(value),
        _ => bail!("Setup cancelled. Accepted answers remain in the vault."),
    }
}
pub fn confirm(label: &str) -> Result<bool> {
    match request(Question::Confirm(label.into()))? {
        Answer::Confirm(value) => Ok(value),
        _ => bail!("Setup cancelled. Accepted answers remain in the vault."),
    }
}
pub fn use_saved(label: &str) -> Result<bool> {
    let prepared_choice = CLIENT.with(|client| {
        let client = client.borrow();
        let staged = client.as_ref()?.prepared.as_ref()?;
        Some(!staged.contains_key(prepared_key(label)))
    });
    if let Some(reuse) = prepared_choice { return Ok(reuse); }
    match request(Question::Saved(label.into()))? {
        Answer::Text(value) => Ok(value == "reuse"),
        _ => bail!("Setup cancelled."),
    }
}
pub fn menu(operations: &[(&str, &str, &str)], first_run: bool) -> Result<Menu> {
    match request(Question::Menu {
        operations: operations
            .iter()
            .map(|(id, label, help)| OperationChoice {
                id: id.to_string(),
                label: label.to_string(),
                description: help.to_string(),
            })
            .collect(),
        first_run,
    })? {
        Answer::Menu(value) => Ok(value),
        _ => Ok(Menu::Exit),
    }
}
pub fn select_modules(entries: Vec<ModuleChoice>) -> Result<Option<Vec<usize>>> {
    match request(Question::Modules(entries))? {
        Answer::Menu(Menu::Setup(selected)) => Ok(Some(selected)),
        _ => Ok(None),
    }
}
pub fn key_choice() -> Result<Option<KeyChoice>> {
    match request(Question::Key)? {
        Answer::Key(value) => Ok(Some(value)),
        _ => Ok(None),
    }
}
pub fn form(
    title: &str,
    fields: Vec<Field>,
    validate: fn(&[String]) -> Result<()>,
) -> Result<Option<Vec<String>>> {
    match request(Question::Form {
        title: title.into(),
        fields,
        validate,
    })? {
        Answer::Fields(values) => Ok(Some(values)),
        _ => Ok(None),
    }
}

fn protect_worker_panics() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if !PRIVATE_WORKER.try_with(Cell::get).unwrap_or(false) {
                previous(info);
            }
        }));
    });
}

pub fn run(work: impl FnOnce() -> Result<()> + Send) -> Result<()> {
    crate::vault::interactive()?;
    protect_worker_panics();
    let (send, receive) = mpsc::channel();
    std::thread::scope(|scope| {
        let worker_send = send.clone();
        let worker = scope.spawn(move || {
            PRIVATE_WORKER.with(|private| private.set(true));
            CLIENT.with(|client| {
                *client.borrow_mut() = Some(Client {
                    sink: worker_send.clone(),
                    notes: String::new(),
                    web: false,
                    browser_vault: None,
                    prepared: None,
                })
            });
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
                .unwrap_or_else(|_| {
                    Err(anyhow!(
                        "Setup stopped unexpectedly. Rerun to inspect saved progress."
                    ))
                });
            let notes = CLIENT.with(|client| {
                client
                    .borrow_mut()
                    .take()
                    .map(|c| c.notes)
                    .unwrap_or_default()
            });
            let detail = match &result {
                Ok(_) => notes,
                Err(error) => format!("{notes}\n{error:#}"),
            };
            let _ = worker_send.send(Update::Finished(detail, result.is_ok()));
            result
        });
        let mut app = element!(
            view::Wizard(receiver: Some(std::sync::Mutex::new(receive)), sender: Some(send))
        );
        let render = app
            .render_loop()
            .fullscreen()
            .ignore_ctrl_c()
            .enable_bracketed_paste();
        #[cfg(windows)]
        let terminal_result = match terminal::new() {
            Ok(backend) => smol::block_on(render.backend(backend)),
            Err(error) => {
                drop(render);
                Err(error)
            }
        };
        #[cfg(not(windows))]
        let terminal_result = smol::block_on(render);
        // Drop the component, channel receiver and pending answer sender before joining:
        // even an initialization/render error must release a blocked setup worker.
        drop(app);
        let result = worker
            .join()
            .map_err(|_| anyhow!("Setup worker stopped."))?;
        terminal_result?;
        result
    })
}

#[cfg(test)]
mod prepared_tests {
    use super::*;

    #[test]
    fn scoped_browser_answers_replace_staged_values_and_reuse_blank_saved_values() -> Result<()> {
        let (sink, updates) = mpsc::channel();
        CLIENT.with(|client| *client.borrow_mut() = Some(Client {
            sink, notes: String::new(), web: true, browser_vault: None, prepared: None,
        }));
        prepare_browser_answers(BTreeMap::from([
            ("Keystore password".into(), Zeroizing::new("secret phrase".into())),
            ("Confirm keystore password".into(), Zeroizing::new("secret phrase".into())),
            ("Android application IDs (comma-separated installed package names)".into(),
                Zeroizing::new(" org.example.app ".into())),
            ("maven-central / MAVEN_CENTRAL_USERNAME new value".into(),
                Zeroizing::new("new username".into())),
        ]));
        assert!(!use_saved("Keystore password")?);
        assert!(!use_saved("Android application IDs")?);
        assert!(use_saved("GitHub token")?);
        assert!(has_prepared_answer("maven-central / MAVEN_CENTRAL_USERNAME new value"));
        assert!(!has_prepared_answer("other / MAVEN_CENTRAL_USERNAME new value"));
        assert_eq!(prompt("Keystore password", None, true)?, "secret phrase");
        assert_eq!(prompt("Confirm keystore password", None, true)?, "secret phrase");
        assert_eq!(prompt("Android application IDs (comma-separated installed package names)", None, false)?, "org.example.app");
        assert!(use_saved("Keystore password")?);
        assert!(matches!(updates.try_recv(), Err(mpsc::TryRecvError::Empty)));
        CLIENT.with(|client| { client.borrow_mut().take(); });
        Ok(())
    }
}
