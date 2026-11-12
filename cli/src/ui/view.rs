use super::{model::*, Update};
use iocraft::prelude::*;
use std::{
    sync::{mpsc, Mutex},
    time::{Duration, Instant},
};

#[derive(Default, Props)]
pub(super) struct WizardProps {
    pub receiver: Option<Mutex<mpsc::Receiver<Update>>>,
    pub sender: Option<mpsc::Sender<Update>>,
    // A supplied model is used by the render regressions as well as the normal startup path.
    pub initial: Option<Model>,
}

#[component]
pub(super) fn Wizard(mut hooks: Hooks, props: &mut WizardProps) -> impl Into<AnyElement<'static>> {
    let mut model = hooks.use_state(|| props.initial.take().unwrap_or_else(Model::new));
    let sender = props.sender.clone().expect("wizard sender");
    let receiver = props
        .receiver
        .take()
        .map(|receiver| receiver.into_inner().unwrap());
    let future_sender = sender.clone();
    hooks.use_future(async move {
        if let Some(receiver) = receiver {
            loop {
                while let Ok(update) = receiver.try_recv() {
                    let mut state = model.write();
                    match update {
                        Update::Question(question, notes, reply) => {
                            let load_keys = matches!(question, super::Question::Key);
                            state.show(question, notes, reply);
                            if load_keys {
                                let (id, sender) = (state.id, future_sender.clone());
                                std::thread::spawn(move || {
                                    super::PRIVATE_WORKER.with(|private| private.set(true));
                                    let result = std::panic::catch_unwind(|| {
                                        crate::gpg::list_signing_keys(None)
                                    })
                                    .unwrap_or_else(|_| {
                                        Err(anyhow::anyhow!(
                                            "Local key discovery stopped unexpectedly."
                                        ))
                                    });
                                    let _ = sender.send(Update::Keys(id, result));
                                });
                            }
                        }
                        Update::Notes(notes) => state.notes = notes,
                        Update::Finished(notes, success) => {
                            state.id += 1;
                            state.reply.take();
                            state.timeout = None;
                            state.notes = notes;
                            state.focus = 0;
                            state.overlay = None;
                            state.screen = Screen::Finished { success };
                        }
                        Update::Keys(id, result) => state.apply_keys(id, result),
                        Update::Clipboard(id, focus, result)
                            if id == state.id && focus == state.focus =>
                        {
                            match result {
                                Ok(text) => state.paste(&text),
                                Err(error) => state.open_overlay(error),
                            }
                        }
                        Update::Clipboard(..) => {}
                    }
                }
                if model.read().timeout.is_some() {
                    model.write().tick(Instant::now());
                }
                smol::Timer::after(Duration::from_millis(50)).await;
            }
        }
    });
    let mut scroll = hooks.use_ref_default::<ScrollViewHandle>();
    let mut notes_scroll = hooks.use_ref_default::<ScrollViewHandle>();
    let mut last_focus = hooks.use_ref(|| None::<(u64, usize, bool)>);
    let (width, height) = hooks.use_terminal_size();
    hooks.use_terminal_events(move |event| match event {
        TerminalEvent::Key(key) if key.kind != KeyEventKind::Release => {
            #[cfg(test)]
            if key.code == KeyCode::F(12) {
                model.write().exit = true;
                return;
            }
            if matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) {
                model.write().interaction();
                let delta = if key.code == KeyCode::PageUp { -5 } else { 5 };
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    notes_scroll.write().scroll_by(delta);
                } else {
                    scroll.write().scroll_by(delta);
                }
            } else {
                model.write().key(key, &sender);
            }
        }
        TerminalEvent::BracketedPaste(text) => {
            let mut state = model.write();
            state.interaction();
            state.paste(&text);
        }
        TerminalEvent::FullscreenMouse(event) if !matches!(event.kind, MouseEventKind::Moved) => {
            model.write().interaction()
        }
        _ => {}
    });
    let state = model.read();
    let mut system = hooks.use_context_mut::<SystemContext>();
    if state.exit {
        system.exit();
    }
    let focus_changed =
        *last_focus.read() != Some((state.id, state.focus, state.overlay.is_some()));
    if focus_changed {
        let offset = match state.focused() {
            Some(Control::Field(index)) => index * 4,
            Some(Control::Check(index)) if matches!(state.screen, Screen::Menu { .. }) => index * 3,
            Some(Control::Key(index)) => index * 2,
            _ => 0,
        };
        scroll.write().scroll_to(offset as i32);
        *last_focus.write() = Some((state.id, state.focus, state.overlay.is_some()));
    }
    let title = match &state.screen {
        Screen::Busy => "Working on your repository",
        Screen::Text { .. } => "Configure setup",
        Screen::Confirm { .. } => "Review and confirm",
        Screen::Menu { .. } => "Setup deployment",
        Screen::Saved(_) => "Reuse saved answer",
        Screen::Key { .. } => "Maven Central signing key",
        Screen::Form { title, .. } => title,
        Screen::Finished { success: true } => "Setup finished",
        Screen::Finished { success: false } => "Setup needs attention",
    }
    .to_string();
    let content_width = usize::from(width.saturating_sub(8));
    let mut rows = Vec::<AnyElement<'static>>::new();
    if let Some(overlay) = &state.overlay {
        rows.push(element!(Text(content: overlay.clone(), color: Color::White)).into_any());
    } else {
        match &state.screen {
            Screen::Busy => rows.push(element!(Text(content: "Working… Your accepted answers are saved as setup progresses.", color: Color::Grey)).into_any()),
            Screen::Text { label, secret, edit } => {
                rows.push(element!(Text(content: label.clone(), weight: Weight::Bold, color: Color::White)).into_any());
                rows.push(input(model, state.id, Control::Field(0), edit.display(*secret, state.focused() == Some(Control::Field(0)), content_width), state.focused() == Some(Control::Field(0))));
            },
            Screen::Confirm { label, checked } => {
                rows.push(element!(Text(content: label.clone(), color: Color::White)).into_any());
                rows.push(check(model, state.id, Control::Check(0), *checked, "I have reviewed this and want to continue".into(), state.focused() == Some(Control::Check(0))));
            },
            Screen::Saved(label) => rows.push(element!(Text(content: format!("A saved value is available for {label}."), color: Color::White)).into_any()),
            Screen::Menu { operations, checked } => for (index, ((label, help), checked)) in operations.iter().zip(checked).enumerate() {
                let control = Control::Check(index);
                rows.push(element! {
                    View(height: 3, min_height: 3, flex_shrink: 0.0_f32, flex_direction: FlexDirection::Column) {
                        #(Some(check(model, state.id, control, *checked, label.clone(), state.focused() == Some(control))))
                        View(height: 1, overflow: Overflow::Hidden, padding_left: 4) { Text(content: help.clone(), color: Color::Grey) }
                    }
                }.into_any());
            },
            Screen::Form { fields, .. } => for (index, field) in fields.iter().enumerate() {
                let control = Control::Field(index);
                let focused = state.focused() == Some(control);
                rows.push(element! {
                    View(height: 4, min_height: 4, flex_shrink: 0.0_f32, flex_direction: FlexDirection::Column) {
                        View(height: 1, overflow: Overflow::Hidden) { Text(content: field.field.label.clone(), weight: Weight::Bold, color: Color::White) }
                        #(Some(input(model, state.id, control, field.edit.display(field.field.secret, focused, content_width), focused)))
                        View(height: 1, overflow: Overflow::Hidden) { Text(content: field.field.hint.clone(), color: Color::Grey) }
                    }
                }.into_any());
            },
            Screen::Key { keys, selected, status } => {
                // Status is also in the persistent panel subtitle; rows stay a fixed height for focus scrolling.
                if keys.is_empty() { rows.push(element!(Text(content: status.clone(), color: Color::Grey)).into_any()); }
                for (index, key) in keys.iter().enumerate() {
                    let control = Control::Key(index);
                    let focused = state.focused() == Some(control);
                    let id = state.id;
                    rows.push(element! {
                        Button(has_focus: false, handler: move |_| activate(model, id, control)) {
                            View(height: 2, min_height: 2, flex_shrink: 0.0_f32, flex_direction: FlexDirection::Column, overflow: Overflow::Hidden, background_color: if focused { Color::DarkBlue } else { Color::Rgb { r: 15, g: 23, b: 42 } }) {
                                Text(content: format!("{} {}", if index == *selected { "●" } else { "○" }, key.identity), color: Color::White)
                                Text(content: format!("  {}", key.fingerprint), color: Color::Grey)
                            }
                        }
                    }.into_any());
                }
            },
            Screen::Finished { .. } => rows.push(element!(Text(content: "Accepted answers remain saved in the vault. Review the progress log below.", color: Color::White)).into_any()),
        }
    }
    let subtitle = match &state.screen {
        Screen::Key {
            keys,
            selected,
            status,
        } => keys
            .get(*selected)
            .map(|key| {
                format!(
                    "{} · Created {} · Expires {}",
                    key.fingerprint, key.created, key.expires
                )
            })
            .unwrap_or_else(|| status.clone()),
        _ => "Accepted answers are saved as you go".into(),
    };
    let actions = state
        .controls()
        .into_iter()
        .filter_map(|control| {
            let Control::Action(action) = control else {
                return None;
            };
            let label = match action {
                Action::Continue => "Continue",
                Action::Cancel => "Back / Cancel",
                Action::Apply => "Apply",
                Action::Help => "What will this do?",
                Action::Setup => "Setup",
                Action::ViewVault => "View vault",
                Action::RemakeVault => "Remake vault",
                Action::Exit => "Close / Exit",
                Action::Reuse => "Use vault value",
                Action::New => "Set new value",
                Action::CreateKey => "Create new key",
                Action::UseKey => "Use selected key",
                Action::Close => "Close details",
            };
            let enabled = action != Action::Apply
                || matches!(state.screen, Screen::Confirm { checked: true, .. });
            Some(button(
                model,
                state.id,
                control,
                label,
                state.focused() == Some(control),
                enabled,
            ))
        })
        .collect::<Vec<_>>();
    let status = if let Some((deadline, _)) = &state.timeout {
        format!(
            "Use the default in {}s · interact to pause",
            deadline.saturating_duration_since(Instant::now()).as_secs() + 1
        )
    } else if state.paused {
        "Automatic confirmation paused.".into()
    } else if matches!(state.screen, Screen::Confirm { .. }) {
        "Apply requires your checked confirmation.".into()
    } else {
        "Tab / Shift+Tab move · Enter selects · Esc cancels".into()
    };
    let notes = if state.notes.is_empty() {
        "Progress and context appear here.".into()
    } else {
        state.notes.clone()
    };
    let notes_height = if height >= 30 { 7 } else { 4 };
    element! {
        View(width, height, flex_direction: FlexDirection::Column, background_color: Color::Rgb { r: 15, g: 23, b: 42 }, padding_left: 1, padding_right: 1, overflow: Overflow::Hidden) {
            View(height: 2, min_height: 2, flex_direction: FlexDirection::Column) {
                Text(content: "ANDROID  /  REPOSITORY SETUP", weight: Weight::Bold, color: Color::Cyan)
                Text(content: "Configure · review · deploy", color: Color::Grey)
            }
            View(flex_grow: 1.0_f32, min_height: 5, flex_direction: FlexDirection::Column, border_style: BorderStyle::Round, border_color: Color::DarkCyan, padding_left: 1, padding_right: 1) {
                View(height: 1, min_height: 1, overflow: Overflow::Hidden) { Text(content: title, color: Color::Cyan, weight: Weight::Bold) }
                View(height: 1, min_height: 1, overflow: Overflow::Hidden) { Text(content: subtitle, color: Color::Grey) }
                View(flex_grow: 1.0_f32, min_height: 1) {
                    ScrollView(handle: scroll, keyboard_scroll: false) {
                        View(width: 100pct, flex_direction: FlexDirection::Column) { #(rows) }
                    }
                }
                View(flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, gap: 1, min_height: 3, flex_shrink: 0.0) { #(actions) }
            }
            View(height: notes_height, min_height: notes_height, flex_direction: FlexDirection::Column, border_style: BorderStyle::Round, border_color: Color::DarkGrey, padding_left: 1, padding_right: 1) {
                ScrollView(handle: notes_scroll, auto_scroll: true, keyboard_scroll: false) { Text(content: notes, color: Color::Grey) }
            }
            View(height: 2, min_height: 2, flex_direction: FlexDirection::Column) {
                Text(content: status, color: Color::Cyan)
                Text(content: "Ctrl+V paste · PgUp/PgDn scroll · Shift+PgUp/PgDn log", color: Color::Grey)
            }
        }
    }
}

fn activate(mut model: State<Model>, id: u64, control: Control) {
    let mut state = model.write();
    if state.id == id {
        state.activate(control);
    }
}
fn button(
    model: State<Model>,
    id: u64,
    control: Control,
    label: &str,
    focused: bool,
    enabled: bool,
) -> AnyElement<'static> {
    let label = format!("{}{}", if focused { "› " } else { "" }, label);
    element! {
        Button(has_focus: false, handler: move |_| { if enabled { activate(model, id, control); } }) {
            View(border_style: BorderStyle::Round, border_color: if focused { Color::Cyan } else { Color::DarkGrey }, padding_left: 1, padding_right: 1) {
                Text(content: label, color: if enabled { Color::White } else { Color::DarkGrey })
            }
        }
    }.into_any()
}
fn input(
    model: State<Model>,
    id: u64,
    control: Control,
    display: String,
    focused: bool,
) -> AnyElement<'static> {
    element! {
        Button(has_focus: false, handler: move |_| activate(model, id, control)) {
            View(height: 1, min_height: 1, flex_shrink: 0.0_f32, width: 100pct, overflow: Overflow::Hidden, background_color: if focused { Color::DarkBlue } else { Color::DarkGrey }) {
                Text(content: display, wrap: TextWrap::NoWrap, color: Color::White)
            }
        }
    }.into_any()
}
fn check(
    model: State<Model>,
    id: u64,
    control: Control,
    checked: bool,
    label: String,
    focused: bool,
) -> AnyElement<'static> {
    // iocraft's Checkbox indicator has no foreground prop. Use its native Button
    // primitive with an explicitly styled indicator so light terminals remain legible.
    element! {
        Button(has_focus: false, handler: move |_| activate(model, id, control)) {
            View(background_color: if focused { Color::DarkBlue } else { Color::Rgb { r: 15, g: 23, b: 42 } }) {
                Text(content: format!("{}{} {}", if focused { "› " } else { "" }, if checked { "[x]" } else { "[ ]" }, label), color: Color::White)
            }
        }
    }.into_any()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{Answer, Field, Question, SigningKey};
    use futures::StreamExt;
    fn setup(question: Question) -> (Model, mpsc::Receiver<Answer>) {
        let mut model = Model::new();
        let (tx, rx) = mpsc::channel();
        model.show(question, "A context line\n".repeat(50), tx);
        (model, rx)
    }
    fn key(code: KeyCode) -> TerminalEvent {
        TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, code))
    }
    fn render(model: Model, mut events: Vec<TerminalEvent>) -> Vec<String> {
        let (sender, _) = mpsc::channel();
        smol::block_on(async move {
            events.push(key(KeyCode::F(12)));
            let duration = Duration::from_secs(5);
            let events = futures::stream::iter(events).then(|event| async move {
                smol::Timer::after(Duration::from_millis(5)).await;
                event
            });
            element!(Wizard(initial: Some(model), sender: Some(sender)))
                .mock_terminal_render_loop(MockTerminalConfig::with_events(events))
                .take_until(smol::Timer::after(duration))
                .map(|canvas| canvas.to_string())
                .collect()
                .await
        })
    }
    #[test]
    fn mock_secret_input_masks_typing_paste_and_moves_focus() {
        let (model, _) = setup(Question::Text {
            label: "Vault password".into(),
            default: None,
            secret: true,
        });
        let frames = render(
            model,
            vec![
                TerminalEvent::Resize(80, 24),
                TerminalEvent::BracketedPaste("fixture-private-value".into()),
                key(KeyCode::Left),
                key(KeyCode::Char('中')),
                key(KeyCode::Tab),
            ],
        );
        assert!(frames.len() >= 2);
        assert!(frames
            .iter()
            .all(|frame| !frame.contains("fixture-private") && !frame.contains('中')));
        assert!(frames.iter().any(|frame| frame.contains('•')));
        assert!(frames.last().unwrap().contains("› Continue"));
    }
    #[test]
    fn mock_tab_navigation_keeps_last_form_field_visible_at_80_by_24() {
        let fields = (0..15)
            .map(|index| Field {
                label: format!("Field {index}"),
                hint: "A useful hint".into(),
                value: format!("value-{index}"),
                secret: false,
            })
            .collect();
        let (model, _) = setup(Question::Form {
            title: "Long form".into(),
            fields,
            validate: |_| Ok(()),
        });
        let mut events = vec![TerminalEvent::Resize(80, 24)];
        events.extend((0..14).map(|_| key(KeyCode::Tab)));
        let frames = render(model, events);
        assert!(
            frames.last().unwrap().contains("Field 14"),
            "focused field must be visible: {}",
            frames.last().unwrap()
        );
        assert!(frames.last().unwrap().contains("Continue"));
        assert!(frames.last().unwrap().lines().count() <= 24);
    }
    #[test]
    fn mock_long_key_list_keeps_selected_key_visible_and_accepts_it() {
        let (mut model, rx) = setup(Question::Key);
        let keys = (0..20)
            .map(|index| SigningKey {
                identity: format!("Local identity {index}"),
                fingerprint: format!("FINGERPRINT-{index}"),
                created: "today".into(),
                expires: "never".into(),
            })
            .collect();
        model.apply_keys(model.id, Ok(keys));
        let mut events = vec![TerminalEvent::Resize(80, 24)];
        events.extend((0..19).map(|_| key(KeyCode::Tab)));
        // Move to the persistent Use selected key action and accept exactly once.
        events.extend([key(KeyCode::Tab), key(KeyCode::Tab), key(KeyCode::Enter)]);
        let frames = render(model, events);
        assert!(
            frames
                .iter()
                .any(|frame| frame.contains("Local identity 19")),
            "selected key visible: {}",
            frames.get(frames.len().saturating_sub(2)).unwrap()
        );
        assert!(
            matches!(rx.try_recv(), Ok(Answer::Key(crate::ui::KeyChoice::Existing(key))) if key.identity == "Local identity 19")
        );
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn mock_keyboard_confirmation_never_approves_unchecked_apply() {
        let (model, rx) = setup(Question::Confirm("Apply changes?".into()));
        render(
            model,
            vec![
                key(KeyCode::Tab),
                key(KeyCode::Enter),
                key(KeyCode::BackTab),
                key(KeyCode::Char(' ')),
                key(KeyCode::Tab),
                key(KeyCode::Enter),
            ],
        );
        assert!(matches!(rx.try_recv(), Ok(Answer::Confirm(true))));
        assert!(rx.try_recv().is_err());
    }
}

#[cfg(test)]
mod mouse_tests {
    use super::*;
    use crate::ui::{Answer, Question};
    use futures::StreamExt;
    fn key(code: KeyCode) -> TerminalEvent {
        TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, code))
    }
    fn find(frame: &str, needle: &str) -> (u16, u16) {
        frame
            .lines()
            .enumerate()
            .find_map(|(y, line)| {
                line.find(needle)
                    .map(|byte| (line[..byte].chars().count() as u16, y as u16))
            })
            .unwrap_or_else(|| panic!("visible {needle}: {frame}"))
    }
    #[test]
    fn actual_checkbox_and_apply_mouse_targets_require_confirmation() {
        smol::block_on(async {
            let (sender, _) = mpsc::channel();
            let (answer, receive) = mpsc::channel();
            let mut model = Model::new();
            model.show(
                Question::Confirm("Apply changes?".into()),
                String::new(),
                answer,
            );
            let (events, input) = futures::channel::mpsc::unbounded();
            let mut app = element!(Wizard(initial: Some(model), sender: Some(sender)));
            let frames = app
                .mock_terminal_render_loop(MockTerminalConfig::with_events(input))
                .take_until(smol::Timer::after(Duration::from_secs(5)));
            futures::pin_mut!(frames);
            events
                .unbounded_send(TerminalEvent::Resize(80, 24))
                .unwrap();
            let initial = loop {
                let frame = frames.next().await.unwrap().to_string();
                if frame.contains("[ ]") {
                    break frame;
                }
            };
            let check = find(&initial, "[ ]");
            let (x, y) = find(&initial, "│ Apply │");
            let apply = (x + 2, y);
            for (x, y) in [apply, check, apply] {
                events
                    .unbounded_send(TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(
                        MouseEventKind::Down(MouseButton::Left),
                        x,
                        y,
                    )))
                    .unwrap();
                // A harmless resize drives all local and global callbacks before the next click.
                events
                    .unbounded_send(TerminalEvent::Resize(80, 24))
                    .unwrap();
                smol::Timer::after(Duration::from_millis(5)).await;
                // Poll the renderer until the queued click is delivered and any resulting frame is drawn.
                for _ in 0..4 {
                    let _ = futures::poll!(frames.next());
                }
                if (x, y) == check {
                    assert!(receive.try_recv().is_err());
                }
            }
            events.unbounded_send(key(KeyCode::F(12))).unwrap();
            while frames.next().await.is_some() {}
            assert!(matches!(receive.try_recv(), Ok(Answer::Confirm(true))));
            assert!(receive.try_recv().is_err());
        });
    }
}
