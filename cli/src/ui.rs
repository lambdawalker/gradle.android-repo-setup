//! Channel-backed full-screen setup wizard. Slow setup and GPG work stay off the renderer.
mod model;
#[cfg(any(windows, test))]
mod mouse_input;
#[cfg(any(windows, test))]
mod terminal;
mod view;
use crate::gpg::SigningKey;
use anyhow::{anyhow, bail, Result};
use iocraft::prelude::*;
use std::{
    cell::{Cell, RefCell},
    sync::{mpsc, Once},
};

thread_local! { static PRIVATE_WORKER: Cell<bool> = const { Cell::new(false) }; }
thread_local! { static CLIENT: RefCell<Option<Client>> = const { RefCell::new(None) }; }
struct Client {
    sink: mpsc::Sender<Update>,
    notes: String,
}
enum Update {
    Question(Question, String, mpsc::Sender<Answer>),
    Notes(String),
    Finished(String, bool),
    Keys(u64, Result<Vec<SigningKey>>),
    Clipboard(u64, usize, Result<String, String>),
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
enum Question {
    Text {
        label: String,
        default: Option<String>,
        secret: bool,
    },
    Confirm(String),
    Menu {
        operations: Vec<(String, String)>,
        first_run: bool,
    },
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

pub fn prompt(label: &str, default: Option<&str>, secret: bool) -> Result<String> {
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
    Ok(matches!(
        request(Question::Confirm(label.into()))?,
        Answer::Confirm(true)
    ))
}
pub fn use_saved(label: &str) -> Result<bool> {
    match request(Question::Saved(label.into()))? {
        Answer::Text(value) => Ok(value == "reuse"),
        _ => bail!("Setup cancelled."),
    }
}
pub fn menu(operations: &[(&str, &str, &str)], first_run: bool) -> Result<Menu> {
    match request(Question::Menu {
        operations: operations
            .iter()
            .map(|(_, label, help)| (label.to_string(), help.to_string()))
            .collect(),
        first_run,
    })? {
        Answer::Menu(value) => Ok(value),
        _ => Ok(Menu::Exit),
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
