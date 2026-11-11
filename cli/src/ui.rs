//! The terminal owns rendering. Setup work runs on a worker and exchanges typed
//! requests with it, so network, keytool and GnuPG never block input or resize.
#[cfg(any(windows, test))]
mod mouse_input;
use crate::gpg::SigningKey;
use anyhow::{anyhow, bail, Result};
use cursive::{
    event::{Event, EventResult, Key},
    theme::{BorderStyle, Color, ColorStyle, PaletteColor, PaletteStyle},
    view::{Nameable, Resizable, Scrollable, ViewWrapper},
    views::{Button, Checkbox, Dialog, EditView, LinearLayout, OnEventView, SelectView, TextView},
    CbSink, Cursive, View,
};
use std::{
    cell::RefCell,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};

thread_local! { static CLIENT: RefCell<Option<Client>> = const { RefCell::new(None) }; }
struct Client {
    sink: CbSink,
    notes: String,
}
struct State {
    reply: Option<mpsc::Sender<Answer>>,
    id: u64,
    touched: Arc<AtomicBool>,
    timeout: Option<(Instant, String)>,
    done: bool,
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
            let text = client.notes.clone();
            let _ = client.sink.send(Box::new(move |app| {
                app.call_on_name("context", |view: &mut TextView| view.set_content(text));
            }));
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
            .send(Box::new(move |app| {
                show_question(app, question, notes, send)
            }))
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

pub fn run(work: impl FnOnce() -> Result<()> + Send) -> Result<()> {
    crate::vault::interactive()?;
    let mut app = cursive::default();
    let touched = Arc::new(AtomicBool::new(false));
    app.set_user_data(State {
        reply: None,
        id: 0,
        touched,
        timeout: None,
        done: false,
    });
    let mut theme = app.current_theme().clone();
    theme.shadow = false;
    theme.borders = BorderStyle::Simple;
    theme.palette[PaletteColor::Background] = Color::Rgb(15, 23, 42);
    theme.palette[PaletteColor::View] = Color::Rgb(25, 38, 57);
    theme.palette[PaletteColor::Primary] = Color::Rgb(235, 242, 250);
    theme.palette[PaletteColor::Secondary] = Color::Rgb(159, 181, 203);
    theme.palette[PaletteColor::TitlePrimary] = Color::Rgb(103, 232, 249);
    theme.palette[PaletteColor::Highlight] = Color::Rgb(44, 108, 147);
    theme.palette[PaletteStyle::EditableText] =
        ColorStyle::new(Color::Rgb(235, 242, 250), Color::Rgb(42, 60, 82)).into();
    app.set_theme(theme);
    app.set_fps(5);
    app.add_global_callback(Event::Refresh, tick);
    app.set_on_pre_event(Event::CtrlChar('c'), cancel);
    app.set_on_pre_event(Event::Exit, cancel);
    app.add_global_callback(Event::CtrlChar('q'), cancel);
    app.add_global_callback(Key::Esc, cancel);
    busy(&mut app);
    let sink = app.cb_sink().clone();
    std::thread::scope(|scope| {
        let worker = scope.spawn(move || {
            CLIENT.with(|client| {
                *client.borrow_mut() = Some(Client {
                    sink: sink.clone(),
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
                    .map(|client| client.notes)
                    .unwrap_or_default()
            });
            let title = if result.is_ok() {
                "Setup finished"
            } else {
                "Setup needs attention"
            };
            let detail = match &result {
                Ok(_) => notes,
                Err(error) => format!("{notes}\n{error:#}"),
            };
            let _ = sink.send(Box::new(move |app| {
                app.user_data::<State>().unwrap().done = true;
                replace(
                    app,
                    Dialog::around(TextView::new(detail).scrollable())
                        .title(title)
                        .button("Close", Cursive::quit),
                );
            }));
            result
        });
        #[cfg(windows)]
        let terminal = app.try_run_with(mouse_input::init);
        #[cfg(not(windows))]
        let terminal = app.try_run_with(cursive::backends::crossterm::Backend::init);
        // Unblock the worker even if a backend ends with an outstanding question.
        app.user_data::<State>().unwrap().reply.take();
        drop(app);
        let result = worker
            .join()
            .map_err(|_| anyhow!("Setup worker stopped."))?;
        terminal?;
        result
    })
}

struct Activity<V> {
    inner: V,
    touched: Arc<AtomicBool>,
}
impl<V: View> ViewWrapper for Activity<V> {
    cursive::wrap_impl!(self.inner: V);
    fn wrap_on_event(&mut self, event: Event) -> EventResult {
        if !matches!(event, Event::Refresh | Event::WindowResize) {
            self.touched.store(true, Ordering::Relaxed);
        }
        self.inner.on_event(event)
    }
}
fn replace(app: &mut Cursive, view: impl View + 'static) {
    while app.pop_layer().is_some() {}
    let touched = app.user_data::<State>().unwrap().touched.clone();
    app.add_layer(Activity {
        inner: view.full_screen(),
        touched,
    });
}
fn context(notes: String) -> impl View {
    TextView::new(notes)
        .with_name("context")
        .scrollable()
        .full_height()
}
fn busy(app: &mut Cursive) {
    replace(
        app,
        Dialog::around(
            LinearLayout::vertical()
                .child(TextView::new(
                    "Working… Your accepted answers are saved as setup progresses.",
                ))
                .child(context(String::new())),
        )
        .title("Android Repository Setup"),
    );
}
fn cancel(app: &mut Cursive) {
    if app.user_data::<State>().unwrap().done {
        app.quit();
    } else if app.user_data::<State>().unwrap().reply.is_some() {
        respond(app, Answer::Cancel);
    }
}
fn respond(app: &mut Cursive, answer: Answer) {
    let state = app.user_data::<State>().unwrap();
    state.timeout = None;
    state.id += 1; // invalidates late key-list callbacks and old countdowns
    if let Some(reply) = state.reply.take() {
        let _ = reply.send(answer);
    }
    busy(app);
}
fn tick(app: &mut Cursive) {
    let state = app.user_data::<State>().unwrap();
    let Some((deadline, default)) = state.timeout.as_ref() else {
        return;
    };
    if state.touched.load(Ordering::Relaxed) {
        state.timeout = None;
        app.call_on_name("countdown", |view: &mut TextView| {
            view.set_content("Automatic confirmation paused.")
        });
        return;
    }
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        let answer = default.clone();
        respond(app, Answer::Text(answer));
    } else {
        app.call_on_name("countdown", |view: &mut TextView| {
            view.set_content(format!(
                "Use the default in {}s · interact to pause",
                remaining.as_secs() + 1
            ))
        });
    }
}
fn timer(app: &mut Cursive, default: String) {
    let state = app.user_data::<State>().unwrap();
    state.timeout = Some((Instant::now() + Duration::from_secs(20), default));
}
fn input(name: String, value: String, secret: bool) -> impl View {
    let mut field = EditView::new().content(value);
    field.set_secret(secret);
    let target = name.clone();
    OnEventView::new(field.with_name(name)).on_pre_event(Event::CtrlChar('v'), move |app| {
        match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_text()) {
            Ok(text) if !text.chars().any(char::is_control) => {
                app.call_on_name(&target, |field: &mut EditView| {
                    let cursor = field.get_cursor();
                    let mut value = (*field.get_content()).clone();
                    value.insert_str(cursor, &text);
                    field.set_content(value);
                    field.set_cursor(cursor+text.len());
                });
            }
            _ => { app.add_layer(Dialog::info("Clipboard unavailable or contains multiple lines. Paste a single line or type the value.")); }
        }
    }).full_width()
}
fn value(app: &mut Cursive, name: &str) -> String {
    app.call_on_name(name, |field: &mut EditView| (*field.get_content()).clone())
        .unwrap_or_default()
}

fn show_question(
    app: &mut Cursive,
    question: Question,
    notes: String,
    reply: mpsc::Sender<Answer>,
) {
    let state = app.user_data::<State>().unwrap();
    state.id += 1;
    state.reply = Some(reply);
    state.touched.store(false, Ordering::Relaxed);
    state.timeout = None;
    let menu_notes = notes.clone();
    let body = LinearLayout::vertical().child(context(notes));
    match question {
        Question::Text {
            label,
            default,
            secret,
        } => {
            let body = body
                .child(TextView::new(label))
                .child(TextView::new(" "))
                .child(input(
                    "answer".into(),
                    default.clone().unwrap_or_default(),
                    secret,
                ))
                .child(TextView::new(" ").with_name("countdown"));
            replace(
                app,
                Dialog::around(body)
                    .title("Configure setup")
                    .button("Continue", move |app| {
                        let answer = value(app, "answer");
                        respond(
                            app,
                            Answer::Text(if secret { answer } else { answer.trim().into() }),
                        );
                    })
                    .button("Cancel", |app| respond(app, Answer::Cancel)),
            );
            app.focus_name("answer").ok();
            if !secret {
                if let Some(default) = default {
                    timer(app, default);
                }
            }
        }
        Question::Confirm(label) => {
            let body = body
                .child(TextView::new(label))
                .child(TextView::new(" "))
                .child(
                    LinearLayout::horizontal()
                        .child(
                            Checkbox::new()
                                .on_change(|app, checked| {
                                    app.call_on_name("apply", |button: &mut Button| {
                                        button.set_enabled(checked)
                                    });
                                })
                                .with_name("confirmed"),
                        )
                        .child(TextView::new(" I have reviewed this and want to continue")),
                );
            let mut apply = Button::new("Apply", |app| {
                let confirmed = app
                    .call_on_name("confirmed", |field: &mut Checkbox| field.is_checked())
                    .unwrap_or(false);
                if confirmed {
                    respond(app, Answer::Confirm(true));
                }
            });
            apply.disable();
            let actions = LinearLayout::horizontal()
                .child(apply.with_name("apply"))
                .child(Button::new("Cancel", |app| {
                    respond(app, Answer::Confirm(false))
                }));
            replace(
                app,
                Dialog::around(body.child(TextView::new(" ")).child(actions))
                    .title("Review and confirm"),
            );
        }
        Question::Saved(label) => {
            replace(
                app,
                Dialog::around(
                    body.child(TextView::new(format!(
                        "A saved value is available for {label}."
                    )))
                    .child(TextView::new(" ").with_name("countdown")),
                )
                .title("Reuse saved answer")
                .button("Use vault value", |app| {
                    respond(app, Answer::Text("reuse".into()))
                })
                .button("Set new value", |app| {
                    respond(app, Answer::Text("new".into()))
                })
                .button("Cancel", |app| respond(app, Answer::Cancel)),
            );
            timer(app, "reuse".into());
        }
        Question::Menu {
            operations,
            first_run,
        } => show_menu(app, operations, first_run, menu_notes),
        Question::Key => show_key_picker(app),
        Question::Form {
            title,
            fields,
            validate,
        } => {
            let mut rows = LinearLayout::vertical();
            for (index, field) in fields.iter().enumerate() {
                rows.add_child(
                    LinearLayout::horizontal()
                        .child(TextView::new(format!("{} ", field.label)).fixed_width(22))
                        .child(input(
                            format!("field-{index}"),
                            field.value.clone(),
                            field.secret,
                        )),
                );
                rows.add_child(TextView::new(field.hint.clone()));
                rows.add_child(TextView::new(" "));
            }
            replace(
                app,
                Dialog::around(rows.scrollable())
                    .title(title)
                    .button("Continue", move |app| {
                        let values = (0..fields.len())
                            .map(|index| value(app, &format!("field-{index}")))
                            .collect::<Vec<_>>();
                        match validate(&values) {
                            Ok(()) => respond(app, Answer::Fields(values)),
                            Err(error) => app
                                .add_layer(Dialog::info(error.to_string()).title("Check the form")),
                        }
                    })
                    .button("Back", |app| respond(app, Answer::Cancel)),
            );
        }
    }
}

fn show_menu(app: &mut Cursive, operations: Vec<(String, String)>, first_run: bool, notes: String) {
    let mut rows = LinearLayout::vertical()
        .child(TextView::new("Choose the operations to perform."))
        .child(TextView::new(" "));
    if !notes.is_empty() {
        rows.add_child(TextView::new(notes));
    }
    for (index, (label, help)) in operations.iter().enumerate() {
        rows.add_child(
            LinearLayout::horizontal()
                .child(
                    Checkbox::new()
                        .with_checked(first_run)
                        .with_name(format!("operation-{index}")),
                )
                .child(TextView::new(format!(" {label}"))),
        );
        rows.add_child(TextView::new(help.clone()));
        rows.add_child(TextView::new(" "));
    }
    let count = operations.len();
    let descriptions = operations.clone();
    replace(
        app,
        Dialog::around(rows.scrollable())
            .title("Setup deployment")
            .button("What will this do?", move |app| {
                let help = descriptions
                    .iter()
                    .enumerate()
                    .filter_map(|(index, (label, help))| {
                        app.call_on_name(&format!("operation-{index}"), |field: &mut Checkbox| {
                            field.is_checked()
                        })
                        .unwrap_or(false)
                        .then(|| format!("{label}\n{help}"))
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                app.add_layer(Dialog::info(if help.is_empty() {
                    "Select an operation to see its explanation.".into()
                } else {
                    help
                }));
            })
            .button("Setup", move |app| {
                let selected = (0..count)
                    .filter(|index| {
                        app.call_on_name(&format!("operation-{index}"), |field: &mut Checkbox| {
                            field.is_checked()
                        })
                        .unwrap_or(false)
                    })
                    .collect::<Vec<_>>();
                if selected.is_empty() {
                    app.add_layer(Dialog::info("Select at least one setup operation."));
                } else {
                    respond(app, Answer::Menu(Menu::Setup(selected)));
                }
            })
            .button("View vault", |app| {
                respond(app, Answer::Menu(Menu::ViewVault))
            })
            .button("Remake vault", |app| {
                respond(app, Answer::Menu(Menu::RemakeVault))
            })
            .button("Exit", |app| respond(app, Answer::Menu(Menu::Exit))),
    );
}

fn show_key_picker(app: &mut Cursive) {
    let id = app.user_data::<State>().unwrap().id;
    let list = SelectView::<SigningKey>::new()
        .on_select(|app, key| {
            app.call_on_name("key-details", |view: &mut TextView| {
                view.set_content(format!(
                    "{}\nCreated {}    Expires {}",
                    key.fingerprint, key.created, key.expires
                ))
            });
        })
        .with_name("keys")
        .scrollable();
    replace(
        app,
        Dialog::around(
            LinearLayout::vertical()
                .child(TextView::new(
                    "Create a project key or choose a local GnuPG signing key.",
                ))
                .child(TextView::new("Loading local keys…").with_name("key-status"))
                .child(list.full_height())
                .child(TextView::new(" ").with_name("key-details")),
        )
        .title("Maven Central signing key")
        .button("Create new key", |app| {
            respond(app, Answer::Key(KeyChoice::Create))
        })
        .button("Use selected key", |app| {
            let key = app
                .call_on_name("keys", |list: &mut SelectView<SigningKey>| list.selection())
                .flatten();
            if let Some(key) = key {
                respond(app, Answer::Key(KeyChoice::Existing((*key).clone())));
            } else {
                app.add_layer(Dialog::info("Choose a loaded key or create a new key."));
            }
        })
        .button("Back", |app| respond(app, Answer::Cancel)),
    );
    let sink = app.cb_sink().clone();
    std::thread::spawn(move || {
        let keys = crate::gpg::list_signing_keys(None);
        let _ = sink.send(Box::new(move |app| {
            apply_keys(app, id, keys);
        }));
    });
}

fn apply_keys(app: &mut Cursive, id: u64, keys: Result<Vec<SigningKey>>) {
    if app.user_data::<State>().unwrap().id != id {
        return;
    }
    match keys {
        Ok(keys) => {
            let status = if keys.is_empty() {
                "No signing keys found. Create a new key to continue."
            } else {
                "Select a key to inspect its fingerprint and dates."
            };
            app.call_on_name("key-status", |view: &mut TextView| view.set_content(status));
            if let Some(key) = keys.first() {
                app.call_on_name("key-details", |view: &mut TextView| {
                    view.set_content(format!(
                        "{}\nCreated {}    Expires {}",
                        key.fingerprint, key.created, key.expires
                    ))
                });
            }
            app.call_on_name("keys", |list: &mut SelectView<SigningKey>| {
                for key in keys {
                    list.add_item(key.identity.clone(), key);
                }
            });
        }
        Err(error) => {
            app.call_on_name("key-status", |view: &mut TextView| {
                view.set_content(format!("Could not load keys. {error}"))
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> Cursive {
        let mut app = Cursive::new();
        app.set_user_data(State {
            reply: None,
            id: 0,
            touched: Arc::new(AtomicBool::new(false)),
            timeout: None,
            done: false,
        });
        app
    }

    fn show(app: &mut Cursive, question: Question) -> mpsc::Receiver<Answer> {
        let (send, receive) = mpsc::channel();
        show_question(app, question, "Review details".repeat(100), send);
        app.screen_mut().layout((80, 24).into());
        receive
    }

    #[test]
    fn apply_requires_checked_confirmation_and_sends_one_answer() {
        let mut app = app();
        let receive = show(&mut app, Question::Confirm("Apply changes?".into()));
        let event = app
            .call_on_name("apply", |button: &mut Button| {
                button.on_event(Event::Key(Key::Enter))
            })
            .unwrap();
        event.process(&mut app);
        assert!(receive.try_recv().is_err());
        let event = app
            .call_on_name("confirmed", |checkbox: &mut Checkbox| checkbox.check())
            .unwrap();
        event.process(&mut app);
        let event = app
            .call_on_name("apply", |button: &mut Button| {
                button.on_event(Event::Key(Key::Enter))
            })
            .unwrap();
        event.process(&mut app);
        assert!(matches!(receive.try_recv(), Ok(Answer::Confirm(true))));
        assert!(app.user_data::<State>().unwrap().reply.is_none());
    }

    #[test]
    fn typing_pauses_default_and_preserves_the_answer() {
        let mut app = app();
        let receive = show(
            &mut app,
            Question::Text {
                label: "Alias".into(),
                default: Some("demo".into()),
                secret: false,
            },
        );
        app.on_event(Event::Char('x'));
        app.user_data::<State>().unwrap().timeout = Some((Instant::now(), "demo".into()));
        tick(&mut app);
        assert!(receive.try_recv().is_err());
        assert!(app.user_data::<State>().unwrap().timeout.is_none());
        assert_eq!(value(&mut app, "answer"), "demox");
    }

    #[test]
    fn untouched_saved_answer_is_reused_after_countdown() {
        let mut app = app();
        let receive = show(&mut app, Question::Saved("GitHub token".into()));
        app.user_data::<State>().unwrap().timeout = Some((Instant::now(), "reuse".into()));
        tick(&mut app);
        assert!(matches!(receive.try_recv(),Ok(Answer::Text(value)) if value=="reuse"));
    }

    #[test]
    fn secret_input_has_no_countdown() {
        let mut app = app();
        let receive = show(
            &mut app,
            Question::Text {
                label: "Vault password".into(),
                default: Some("".into()),
                secret: true,
            },
        );
        tick(&mut app);
        assert!(app.user_data::<State>().unwrap().timeout.is_none());
        assert!(receive.try_recv().is_err());
    }

    #[test]
    fn late_key_listing_cannot_replace_a_new_form() {
        let mut app = app();
        let _receive = show(
            &mut app,
            Question::Text {
                label: "New key identity".into(),
                default: None,
                secret: false,
            },
        );
        let old_id = app.user_data::<State>().unwrap().id;
        respond(&mut app, Answer::Cancel);
        let receive = show(
            &mut app,
            Question::Text {
                label: "New key identity".into(),
                default: Some("Current identity".into()),
                secret: false,
            },
        );
        apply_keys(&mut app, old_id, Err(anyhow!("Old listing failed")));
        assert_eq!(value(&mut app, "answer"), "Current identity");
        assert!(receive.try_recv().is_err());
    }

    #[test]
    fn subsequent_setup_starts_with_all_operations_unchecked() {
        let mut app = app();
        let _receive = show(
            &mut app,
            Question::Menu {
                operations: vec![("APK signing".into(), "Create key".into())],
                first_run: false,
            },
        );
        assert!(!app
            .call_on_name("operation-0", |checkbox: &mut Checkbox| checkbox
                .is_checked())
            .unwrap());
    }
}
