use super::{Answer, Field, KeyChoice, Menu, Question, SigningKey, Update};
use anyhow::Result;
use iocraft::prelude::*;
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
use unicode_segmentation::UnicodeSegmentation;

/// Cursor offsets are UTF-8 boundaries between extended grapheme clusters.
/// The buffer is never formatted with Debug or passed to an unmasked widget.
pub(super) struct EditBuffer {
    pub value: String,
    pub cursor: usize,
}
impl EditBuffer {
    pub fn new(value: String) -> Self {
        let cursor = value.len();
        Self { value, cursor }
    }
    pub fn left(&mut self) {
        self.cursor = self.value[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(i, _)| i);
    }
    pub fn right(&mut self) {
        if let Some(g) = self.value[self.cursor..].graphemes(true).next() {
            self.cursor += g.len();
        }
    }
    pub fn insert(&mut self, text: &str) -> bool {
        if text.chars().any(char::is_control) {
            return false;
        }
        self.value.insert_str(self.cursor, text);
        self.cursor += text.len();
        // Insertion can join clusters (combining marks / ZWJ). Keep a valid boundary.
        self.normalize_cursor();
        true
    }
    fn normalize_cursor(&mut self) {
        while self.cursor < self.value.len()
            && !self
                .value
                .grapheme_indices(true)
                .any(|(i, _)| i == self.cursor)
        {
            self.cursor += self.value[self.cursor..].chars().next().unwrap().len_utf8();
        }
    }
    pub fn backspace(&mut self) {
        let end = self.cursor;
        self.left();
        self.value.replace_range(self.cursor..end, "");
        self.normalize_cursor();
    }
    pub fn delete(&mut self) {
        let start = self.cursor;
        self.right();
        self.value.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.normalize_cursor();
    }
    pub fn display(&self, secret: bool, focused: bool, width: usize) -> String {
        let clusters = self.value.graphemes(true).collect::<Vec<_>>();
        let cursor = self.value[..self.cursor].graphemes(true).count();
        // Reserve two cells per cluster for wide Unicode and a cursor. Secrets use one mask per cluster.
        let capacity = if secret {
            width.saturating_sub(2)
        } else {
            width.saturating_sub(2) / 2
        }
        .max(1);
        let start = cursor.saturating_sub(capacity.saturating_sub(1));
        let end = (start + capacity).min(clusters.len());
        let mut out = String::new();
        if start > 0 {
            out.push('‹');
        }
        for (index, cluster) in clusters.iter().enumerate().take(end).skip(start) {
            if focused && index == cursor {
                out.push('▏');
            }
            out.push_str(if secret { "•" } else { cluster });
        }
        if focused && cursor == end {
            out.push('▏');
        }
        if end < clusters.len() {
            out.push('›');
        }
        if out.is_empty() {
            out.push(' ');
        }
        out
    }
}

pub(super) struct FormField {
    pub field: Field,
    pub edit: EditBuffer,
}
pub(super) enum Screen {
    Busy,
    Text {
        label: String,
        secret: bool,
        edit: EditBuffer,
    },
    Confirm {
        label: String,
        checked: bool,
    },
    Menu {
        operations: Vec<(String, String)>,
        checked: Vec<bool>,
        modules: bool,
    },
    Saved(String),
    Key {
        keys: Vec<SigningKey>,
        selected: usize,
        status: String,
    },
    Form {
        title: String,
        fields: Vec<FormField>,
        validate: fn(&[String]) -> Result<()>,
    },
    Finished {
        success: bool,
    },
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Action {
    Continue,
    Cancel,
    Apply,
    Help,
    Setup,
    ViewVault,
    RemakeVault,
    Exit,
    Reuse,
    New,
    CreateKey,
    UseKey,
    Close,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Control {
    Field(usize),
    Check(usize),
    Key(usize),
    Action(Action),
}

pub(super) struct Model {
    pub screen: Screen,
    pub notes: String,
    pub focus: usize,
    pub id: u64,
    pub reply: Option<mpsc::Sender<Answer>>,
    pub timeout: Option<(Instant, String)>,
    pub paused: bool,
    pub overlay: Option<String>,
    pub exit: bool,
}
impl Model {
    pub fn new() -> Self {
        Self {
            screen: Screen::Busy,
            notes: String::new(),
            focus: 0,
            id: 0,
            reply: None,
            timeout: None,
            paused: false,
            overlay: None,
            exit: false,
        }
    }
    pub fn show(&mut self, question: Question, notes: String, reply: mpsc::Sender<Answer>) {
        if let Some(previous) = self.reply.take() {
            let _ = previous.send(Answer::Cancel);
        }
        self.id += 1;
        self.notes = notes;
        self.reply = Some(reply);
        self.focus = 0;
        self.timeout = None;
        self.paused = false;
        self.overlay = None;
        self.screen = match question {
            Question::Text {
                label,
                default,
                secret,
            } => {
                if !secret {
                    if let Some(default) = &default {
                        self.timeout =
                            Some((Instant::now() + Duration::from_secs(20), default.clone()));
                    }
                }
                Screen::Text {
                    label,
                    secret,
                    edit: EditBuffer::new(default.unwrap_or_default()),
                }
            }
            Question::Confirm(label) => Screen::Confirm {
                label,
                checked: false,
            },
            Question::Menu {
                operations,
                first_run,
            } => {
                let checked = vec![first_run; operations.len()];
                Screen::Menu {
                    operations,
                    checked,
                    modules: false,
                }
            }
            Question::Modules(entries) => Screen::Menu {
                checked: entries.iter().map(|e| e.2).collect(),
                operations: entries
                    .into_iter()
                    .map(|(label, help, _)| (label, help))
                    .collect(),
                modules: true,
            },
            Question::Saved(label) => {
                self.timeout = Some((Instant::now() + Duration::from_secs(20), "reuse".into()));
                Screen::Saved(label)
            }
            Question::Key => Screen::Key {
                keys: Vec::new(),
                selected: 0,
                status: "Loading local keys…".into(),
            },
            Question::Form {
                title,
                fields,
                validate,
            } => Screen::Form {
                title,
                fields: fields
                    .into_iter()
                    .map(|field| {
                        let edit = EditBuffer::new(field.value.clone());
                        FormField { field, edit }
                    })
                    .collect(),
                validate,
            },
        };
    }
    pub fn controls(&self) -> Vec<Control> {
        use Action::*;
        if self.overlay.is_some() {
            return vec![Control::Action(Close)];
        }
        let (mut controls, actions) = match &self.screen {
            Screen::Busy => (vec![], vec![]),
            Screen::Text { .. } => (vec![Control::Field(0)], vec![Continue, Cancel]),
            Screen::Confirm { .. } => (vec![Control::Check(0)], vec![Apply, Cancel]),
            Screen::Menu {
                operations,
                modules,
                ..
            } => (
                (0..operations.len()).map(Control::Check).collect(),
                if *modules {
                    vec![Setup, Cancel]
                } else {
                    vec![Help, Setup, ViewVault, RemakeVault, Exit]
                },
            ),
            Screen::Saved(_) => (vec![], vec![Reuse, New, Cancel]),
            Screen::Key { keys, .. } => (
                (0..keys.len()).map(Control::Key).collect(),
                vec![CreateKey, UseKey, Cancel],
            ),
            Screen::Form { fields, .. } => (
                (0..fields.len()).map(Control::Field).collect(),
                vec![Continue, Cancel],
            ),
            Screen::Finished { .. } => (vec![], vec![Exit]),
        };
        controls.extend(actions.into_iter().map(Control::Action));
        controls
    }
    pub fn focused(&self) -> Option<Control> {
        self.controls().get(self.focus).copied()
    }
    pub fn interaction(&mut self) {
        if self.timeout.take().is_some() {
            self.paused = true;
        }
    }
    pub fn respond(&mut self, answer: Answer) {
        self.timeout = None;
        self.id += 1;
        self.focus = 0;
        self.overlay = None;
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(answer);
        }
        self.screen = Screen::Busy;
    }
    pub fn cancel(&mut self) {
        self.interaction();
        if self.overlay.take().is_some() {
            return;
        }
        if matches!(self.screen, Screen::Finished { .. }) {
            self.exit = true;
        } else if self.reply.is_some() {
            self.respond(Answer::Cancel);
        }
    }
    pub fn tick(&mut self, now: Instant) {
        if let Some((deadline, value)) = &self.timeout {
            if now >= *deadline {
                let value = value.clone();
                self.respond(Answer::Text(value));
            }
        }
    }
    pub fn apply_keys(&mut self, id: u64, result: Result<Vec<SigningKey>>) {
        if self.id != id {
            return;
        }
        if let Screen::Key { keys, status, .. } = &mut self.screen {
            match result {
                Ok(found) => {
                    *status = if found.is_empty() {
                        "No signing keys found. Create a new key to continue."
                    } else {
                        "Select a key to inspect its fingerprint and dates."
                    }
                    .into();
                    *keys = found;
                    self.focus = 0;
                }
                Err(error) => *status = format!("Could not load keys. {error}"),
            }
        }
    }
    pub fn edit_mut(&mut self) -> Option<&mut EditBuffer> {
        let Control::Field(index) = self.focused()? else {
            return None;
        };
        match &mut self.screen {
            Screen::Text { edit, .. } => Some(edit),
            Screen::Form { fields, .. } => fields.get_mut(index).map(|f| &mut f.edit),
            _ => None,
        }
    }
    pub fn open_overlay(&mut self, text: String) {
        self.overlay = Some(text);
        self.focus = 0;
    }
    pub fn paste(&mut self, text: &str) {
        if let Some(edit) = self.edit_mut() {
            if !edit.insert(text) {
                self.open_overlay("Paste a single line without control characters.".into());
            }
        }
    }
    pub fn key(&mut self, event: KeyEvent, sender: &mpsc::Sender<Update>) {
        if event.kind == KeyEventKind::Release {
            return;
        }
        self.interaction();
        if event.code == KeyCode::Esc
            || (event.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(event.code, KeyCode::Char('c' | 'q')))
        {
            self.cancel();
            return;
        }
        let controls = self.controls();
        match event.code {
            KeyCode::Tab | KeyCode::BackTab if !controls.is_empty() => {
                let backward =
                    event.code == KeyCode::BackTab || event.modifiers.contains(KeyModifiers::SHIFT);
                self.focus = if backward {
                    (self.focus + controls.len() - 1) % controls.len()
                } else {
                    (self.focus + 1) % controls.len()
                };
                self.select_focused_key();
            }
            KeyCode::Up | KeyCode::Down if !controls.is_empty() && self.edit_mut().is_none() => {
                self.focus = if event.code == KeyCode::Up {
                    (self.focus + controls.len() - 1) % controls.len()
                } else {
                    (self.focus + 1) % controls.len()
                };
                self.select_focused_key();
            }
            KeyCode::Enter => {
                if let Some(control) = self.focused() {
                    if matches!(control, Control::Field(_)) {
                        self.activate(Control::Action(Action::Continue));
                    } else {
                        self.activate(control);
                    }
                }
            }
            KeyCode::Char('v')
                if event.modifiers.contains(KeyModifiers::CONTROL) && self.edit_mut().is_some() =>
            {
                let (id, focus, sender) = (self.id, self.focus, sender.clone());
                std::thread::spawn(move || {
                    let text = arboard::Clipboard::new()
                        .and_then(|mut c| c.get_text())
                        .map_err(|_| {
                            "Clipboard unavailable. Paste a single line or type the value."
                                .to_string()
                        });
                    let _ = sender.send(Update::Clipboard(id, focus, text));
                });
            }
            KeyCode::Char(' ') if self.edit_mut().is_none() => {
                if let Some(control) = self.focused() {
                    self.activate(control);
                }
            }
            code => {
                if !event
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                {
                    if let Some(edit) = self.edit_mut() {
                        match code {
                            KeyCode::Char(ch) => {
                                edit.insert(&ch.to_string());
                            }
                            KeyCode::Backspace => edit.backspace(),
                            KeyCode::Delete => edit.delete(),
                            KeyCode::Left => edit.left(),
                            KeyCode::Right => edit.right(),
                            KeyCode::Home => edit.cursor = 0,
                            KeyCode::End => edit.cursor = edit.value.len(),
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    fn select_focused_key(&mut self) {
        if let Some(Control::Key(index)) = self.focused() {
            if let Screen::Key { selected, .. } = &mut self.screen {
                *selected = index;
            }
        }
    }
    pub fn activate(&mut self, control: Control) {
        self.interaction();
        if self.overlay.is_some() {
            if control == Control::Action(Action::Close) {
                self.overlay = None;
                self.focus = 0;
            }
            return;
        }
        if !self.controls().contains(&control) {
            return;
        }
        self.focus = self
            .controls()
            .iter()
            .position(|c| *c == control)
            .unwrap_or(0);
        match control {
            Control::Field(_) => {}
            Control::Check(index) => match &mut self.screen {
                Screen::Confirm { checked, .. } => *checked = !*checked,
                Screen::Menu { checked, .. } => checked[index] = !checked[index],
                _ => {}
            },
            Control::Key(index) => {
                if let Screen::Key { selected, .. } = &mut self.screen {
                    *selected = index;
                }
            }
            Control::Action(action) => {
                use Action::*;
                let answer = match action {
                    Continue => match &self.screen {
                        Screen::Text { secret, edit, .. } => Some(Answer::Text(if *secret {
                            edit.value.clone()
                        } else {
                            edit.value.trim().into()
                        })),
                        Screen::Form {
                            fields, validate, ..
                        } => {
                            let values = fields
                                .iter()
                                .map(|f| f.edit.value.clone())
                                .collect::<Vec<_>>();
                            match validate(&values) {
                                Ok(()) => Some(Answer::Fields(values)),
                                Err(error) => {
                                    self.overlay = Some(error.to_string());
                                    None
                                }
                            }
                        }
                        _ => None,
                    },
                    Apply => {
                        if matches!(self.screen, Screen::Confirm { checked: true, .. }) {
                            Some(Answer::Confirm(true))
                        } else {
                            None
                        }
                    }
                    Cancel => Some(if matches!(self.screen, Screen::Confirm { .. }) {
                        Answer::Confirm(false)
                    } else {
                        Answer::Cancel
                    }),
                    Help => {
                        if let Screen::Menu {
                            operations,
                            checked,
                            ..
                        } = &self.screen
                        {
                            let help = operations
                                .iter()
                                .zip(checked)
                                .filter(|(_, selected)| **selected)
                                .map(|((label, help), _)| format!("{label}\n{help}"))
                                .collect::<Vec<_>>()
                                .join("\n\n");
                            self.overlay = Some(if help.is_empty() {
                                "Select an operation to see its explanation.".into()
                            } else {
                                help
                            });
                        }
                        None
                    }
                    Setup => {
                        if let Screen::Menu { checked, .. } = &self.screen {
                            let selected = checked
                                .iter()
                                .enumerate()
                                .filter_map(|(i, yes)| yes.then_some(i))
                                .collect::<Vec<_>>();
                            if selected.is_empty() {
                                self.overlay = Some("Select at least one item to continue.".into());
                                None
                            } else {
                                Some(Answer::Menu(Menu::Setup(selected)))
                            }
                        } else {
                            None
                        }
                    }
                    ViewVault => Some(Answer::Menu(Menu::ViewVault)),
                    RemakeVault => Some(Answer::Menu(Menu::RemakeVault)),
                    Exit => {
                        if matches!(self.screen, Screen::Finished { .. }) {
                            self.exit = true;
                            None
                        } else {
                            Some(Answer::Menu(Menu::Exit))
                        }
                    }
                    Reuse => Some(Answer::Text("reuse".into())),
                    New => Some(Answer::Text("new".into())),
                    CreateKey => Some(Answer::Key(KeyChoice::Create)),
                    UseKey => {
                        if let Screen::Key { keys, selected, .. } = &self.screen {
                            if let Some(key) = keys.get(*selected) {
                                Some(Answer::Key(KeyChoice::Existing(key.clone())))
                            } else {
                                self.overlay =
                                    Some("Choose a loaded key or create a new key.".into());
                                None
                            }
                        } else {
                            None
                        }
                    }
                    Close => None,
                };
                if let Some(answer) = answer {
                    self.respond(answer);
                } else if self.overlay.is_some() {
                    self.focus = 0;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_cursor_paste_and_deletion_are_grapheme_safe() {
        let mut edit = EditBuffer::new("a👩‍💻é".into());
        edit.left();
        assert!(edit.insert("中"));
        assert_eq!(edit.value, "a👩‍💻中é");
        edit.backspace();
        edit.backspace();
        assert_eq!(edit.value, "aé");
        assert!(!edit.insert("line\nbreak"));
        assert_eq!(edit.value, "aé");
    }

    #[test]
    fn confirmation_needs_checkbox_and_sends_exactly_once() {
        let mut model = Model::new();
        let (tx, rx) = std::sync::mpsc::channel();
        model.show(
            Question::Confirm("Apply changes?".into()),
            String::new(),
            tx,
        );
        model.activate(Control::Action(Action::Apply));
        assert!(rx.try_recv().is_err());
        model.activate(Control::Check(0));
        model.activate(Control::Action(Action::Apply));
        assert!(matches!(rx.try_recv(), Ok(Answer::Confirm(true))));
        model.activate(Control::Action(Action::Apply));
        assert!(rx.try_recv().is_err());
    }
}

#[cfg(test)]
mod regressions {
    use super::*;
    fn show(question: Question) -> (Model, mpsc::Receiver<Answer>) {
        let (tx, rx) = mpsc::channel();
        let mut model = Model::new();
        model.show(question, String::new(), tx);
        (model, rx)
    }
    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(KeyEventKind::Press, code)
    }
    #[test]
    fn editing_and_navigation_pause_defaults_but_release_and_resize_do_not() {
        let (mut state, rx) = show(Question::Text {
            label: "Alias".into(),
            default: Some("demo".into()),
            secret: false,
        });
        let (tx, _) = mpsc::channel();
        state.key(
            KeyEvent::new(KeyEventKind::Release, KeyCode::Char('x')),
            &tx,
        );
        assert!(state.timeout.is_some());
        state.key(key(KeyCode::Char('x')), &tx);
        state.tick(Instant::now() + Duration::from_secs(30));
        assert!(state.timeout.is_none());
        assert!(state.paused);
        assert!(rx.try_recv().is_err());
        assert_eq!(state.edit_mut().unwrap().value, "demox");
    }
    #[test]
    fn saved_answers_expire_and_secrets_never_do() {
        let (mut saved, rx) = show(Question::Saved("token".into()));
        saved.tick(Instant::now() + Duration::from_secs(30));
        assert!(matches!(rx.try_recv(), Ok(Answer::Text(value)) if value == "reuse"));
        let (mut secret, rx) = show(Question::Text {
            label: "Password".into(),
            default: Some("fixture secret".into()),
            secret: true,
        });
        secret.tick(Instant::now() + Duration::from_secs(30));
        assert!(secret.timeout.is_none());
        assert!(rx.try_recv().is_err());
        let Screen::Text { edit, .. } = &secret.screen else {
            panic!("text screen")
        };
        assert!(!edit.display(true, true, 80).contains("fixture"));
    }
    #[test]
    fn module_menu_restores_selection_and_cancels_without_saving() {
        let entries = vec![
            (":library".into(), "Library".into(), true),
            (":app".into(), "App".into(), false),
        ];
        let (mut state, rx) = show(Question::Modules(entries.clone()));
        assert!(!state
            .controls()
            .contains(&Control::Action(Action::ViewVault)));
        assert!(state.timeout.is_none());
        state.activate(Control::Check(1));
        state.activate(Control::Action(Action::Setup));
        assert!(
            matches!(rx.try_recv(), Ok(Answer::Menu(Menu::Setup(values))) if values == vec![0,1])
        );
        let (mut state, rx) = show(Question::Modules(entries));
        state.activate(Control::Action(Action::Cancel));
        assert!(matches!(rx.try_recv(), Ok(Answer::Cancel)));
    }
    #[test]
    fn first_run_and_returning_menu_and_vault_actions() {
        for first_run in [false, true] {
            let (mut state, rx) = show(Question::Menu {
                operations: vec![("Sign".into(), "Create a key".into())],
                first_run,
            });
            let Screen::Menu { checked, .. } = &state.screen else {
                panic!("menu")
            };
            assert_eq!(checked, &[first_run]);
            state.activate(Control::Action(Action::Setup));
            if first_run {
                assert!(
                    matches!(rx.try_recv(), Ok(Answer::Menu(Menu::Setup(values))) if values == vec![0])
                );
            } else {
                assert!(state.overlay.is_some());
                assert!(rx.try_recv().is_err());
            }
        }
        for (action, is_view) in [(Action::ViewVault, true), (Action::RemakeVault, false)] {
            let (mut state, rx) = show(Question::Menu {
                operations: vec![],
                first_run: false,
            });
            state.activate(Control::Action(action));
            assert!(match rx.try_recv() {
                Ok(Answer::Menu(Menu::ViewVault)) => is_view,
                Ok(Answer::Menu(Menu::RemakeVault)) => !is_view,
                _ => false,
            });
        }
    }
    #[test]
    fn help_and_form_validation_do_not_answer_until_accepted() {
        let (mut state, rx) = show(Question::Menu {
            operations: vec![("Sign".into(), "Create a key".into())],
            first_run: true,
        });
        state.activate(Control::Action(Action::Help));
        assert!(state.overlay.as_ref().unwrap().contains("Create a key"));
        assert!(rx.try_recv().is_err());
        state.cancel();
        assert!(state.overlay.is_none());
        assert!(state.reply.is_some());
        let (mut state, rx) = show(Question::Form {
            title: "Identity".into(),
            fields: vec![Field {
                label: "Name".into(),
                hint: "Required".into(),
                value: String::new(),
                secret: false,
            }],
            validate: |values| {
                anyhow::ensure!(!values[0].is_empty(), "Name required");
                Ok(())
            },
        });
        state.activate(Control::Action(Action::Continue));
        assert_eq!(state.overlay.as_deref(), Some("Name required"));
        assert!(rx.try_recv().is_err());
        state.activate(Control::Action(Action::Close));
        state.paste("Name");
        state.activate(Control::Action(Action::Continue));
        assert!(matches!(rx.try_recv(), Ok(Answer::Fields(values)) if values == vec!["Name"]));
    }
    #[test]
    fn stale_keys_and_clipboard_cannot_touch_a_new_question() {
        let (mut state, _) = show(Question::Key);
        let old = state.id;
        state.cancel();
        let (tx, rx) = mpsc::channel();
        state.show(
            Question::Text {
                label: "Identity".into(),
                default: Some("current".into()),
                secret: false,
            },
            String::new(),
            tx,
        );
        state.apply_keys(old, Err(anyhow::anyhow!("stale failure")));
        assert!(matches!(state.screen, Screen::Text { .. }));
        assert!(state.overlay.is_none());
        assert!(rx.try_recv().is_err());
        state.cancel();
        assert!(matches!(rx.try_recv(), Ok(Answer::Cancel)));
    }
    #[test]
    fn dropping_renderer_state_releases_pending_worker() {
        let (state, rx) = show(Question::Confirm("Apply?".into()));
        drop(state);
        assert!(matches!(rx.recv(), Err(mpsc::RecvError)));
    }
}

#[cfg(test)]
mod editing_regressions {
    use super::*;
    #[test]
    fn deletion_repairs_boundaries_when_surrounding_clusters_join() {
        for (value, expected) in [("ᄀxᅡ", "가"), ("🇦x🇧", "🇦🇧")] {
            let mut edit = EditBuffer::new(value.into());
            edit.cursor = value.find('x').unwrap();
            edit.delete();
            assert_eq!(edit.value, expected);
            assert!(
                edit.cursor == edit.value.len()
                    || edit
                        .value
                        .grapheme_indices(true)
                        .any(|(i, _)| i == edit.cursor)
            );
            edit.left();
            edit.right();
            edit.backspace();
            assert!(edit.value.is_empty());
        }
    }
    #[test]
    fn enter_can_close_help_and_validation_from_a_later_control() {
        let (tx, _) = mpsc::channel();
        let mut model = Model::new();
        model.show(
            Question::Menu {
                operations: vec![("A".into(), "Details".into()); 7],
                first_run: true,
            },
            String::new(),
            tx,
        );
        model.activate(Control::Action(Action::Help));
        assert!(model.focused() == Some(Control::Action(Action::Close)));
        let (sender, _) = mpsc::channel();
        model.key(KeyEvent::new(KeyEventKind::Press, KeyCode::Enter), &sender);
        assert!(model.overlay.is_none());
    }
}
