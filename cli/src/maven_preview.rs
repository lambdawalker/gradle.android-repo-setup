use std::io::{self, IsTerminal};
use std::thread;

use anyhow::{ensure, Result};
use cursive::event::{Event, Key};
use cursive::theme::{BaseColor, BorderStyle, Color, ColorStyle, PaletteColor, PaletteStyle};
use cursive::traits::{Nameable, Resizable, Scrollable};
use cursive::views::{
    Button, Checkbox, Dialog, EditView, LinearLayout, OnEventView, SelectView, TextView,
};
use cursive::Cursive;

use crate::gpg::{self, SigningKey};

const KEY_LIST: &str = "key-list";
const KEY_DETAILS: &str = "key-details";
const KEY_STATUS: &str = "key-status";
const PASSPHRASE: &str = "passphrase";
const FEEDBACK: &str = "feedback";
const CONFIRMATION: &str = "confirmation";
const PREVIEW: &str = "preview";

pub(crate) fn run(sample_keys: bool) -> Result<()> {
    ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        "Run the Maven key preview in an interactive terminal."
    );

    let mut app = cursive::default();
    configure(&mut app, sample_keys);
    let updates = app.cb_sink().clone();
    thread::Builder::new()
        .name("maven-key-list".into())
        .spawn(move || {
            let keys = if sample_keys {
                Ok(vec![
                    SigningKey {
                        identity: "Alex Example (demo-common) <alex@example.com>".into(),
                        fingerprint: "0123456789ABCDEF0123456789ABCDEF01234567".into(),
                        created: "2025-09-01".into(),
                        expires: "2027-09-01".into(),
                    },
                    SigningKey {
                        identity: "Sam Example (permissions) <sam@example.com>".into(),
                        fingerprint: "89ABCDEF0123456789ABCDEF0123456789ABCDEF".into(),
                        created: "2025-10-01".into(),
                        expires: "Never".into(),
                    },
                ])
            } else {
                gpg::list_signing_keys(None)
            };
            let _ = updates.send(Box::new(move |app| show_keys(app, keys)));
        })?;
    app.try_run_with(cursive::backends::crossterm::Backend::init)?;
    Ok(())
}

fn configure(app: &mut Cursive, sample_keys: bool) {
    use PaletteColor::*;
    let mut theme = app.current_theme().clone();
    theme.shadow = false;
    theme.borders = BorderStyle::Simple;
    theme.palette[Background] = Color::Rgb(15, 23, 42);
    theme.palette[View] = Color::Rgb(25, 38, 57);
    theme.palette[Primary] = Color::Rgb(235, 242, 250);
    theme.palette[Secondary] = Color::Rgb(159, 181, 203);
    theme.palette[TitlePrimary] = Color::Rgb(103, 232, 249);
    theme.palette[Highlight] = Color::Rgb(44, 108, 147);
    theme.palette[HighlightText] = Color::Light(BaseColor::White);
    theme.palette[PaletteStyle::EditableText] =
        ColorStyle::new(Color::Rgb(235, 242, 250), Color::Rgb(42, 60, 82)).into();
    app.set_theme(theme);
    app.add_global_callback(Key::Esc, Cursive::quit);

    let keys = SelectView::<SigningKey>::new()
        .on_select(|app, key| {
            reset_confirmation(app);
            show_key_details(app, key);
        })
        .with_name(KEY_LIST)
        .scrollable()
        .max_height(5);
    let passphrase = OnEventView::new(EditView::new().secret().with_name(PASSPHRASE))
        .on_pre_event(Event::CtrlChar('v'), paste_passphrase)
        .full_width();
    let mut preview = Button::new("Preview selection", preview_selection);
    preview.disable();
    let form = LinearLayout::vertical()
        .child(TextView::new(
            "UI preview · Use a test passphrase. Nothing is saved or published.",
        ))
        .child(TextView::new(""))
        .child(TextView::new(if sample_keys {
            "Sample signing keys"
        } else {
            "Local GnuPG signing keys"
        }))
        .child(TextView::new("Loading signing keys…").with_name(KEY_STATUS))
        .child(keys)
        .child(TextView::new("").with_name(KEY_DETAILS))
        .child(TextView::new(""))
        .child(TextView::new("Test passphrase · Ctrl+V to paste"))
        .child(passphrase)
        .child(TextView::new(""))
        .child(
            LinearLayout::horizontal()
                .child(
                    Checkbox::new()
                        .on_change(|app, checked| {
                            app.call_on_name(PREVIEW, |button: &mut Button| {
                                button.set_enabled(checked)
                            });
                        })
                        .with_name(CONFIRMATION),
                )
                .child(TextView::new(" I have reviewed the selected key")),
        )
        .child(TextView::new("").with_name(FEEDBACK))
        .child(
            LinearLayout::horizontal()
                .child(preview.with_name(PREVIEW))
                .child(Button::new("Close", Cursive::quit)),
        );
    app.add_layer(Dialog::around(form.max_width(84)).title(" Maven Central signing key "));
}

fn show_keys(app: &mut Cursive, result: Result<Vec<SigningKey>>) {
    let keys = match result {
        Ok(keys) if !keys.is_empty() => keys,
        Ok(_) => {
            set_text(
                app,
                KEY_STATUS,
                "No signing keys found. Try --sample-keys to explore the UI.",
            );
            return;
        }
        Err(_) => {
            set_text(
                app,
                KEY_STATUS,
                "Could not list signing keys. Check GnuPG or try --sample-keys.",
            );
            return;
        }
    };
    set_text(app, KEY_STATUS, "Use the arrow keys to select a key.");
    show_key_details(app, &keys[0]);
    app.call_on_name(KEY_LIST, |list: &mut SelectView<SigningKey>| {
        for key in keys {
            list.add_item(key.identity.clone(), key);
        }
    });
    // Keep current focus and input intact when the background result arrives.
}

fn show_key_details(app: &mut Cursive, key: &SigningKey) {
    set_text(
        app,
        KEY_DETAILS,
        &format!(
            "{}\nCreated {}    Expires {}",
            key.fingerprint, key.created, key.expires
        ),
    );
}

fn paste_passphrase(app: &mut Cursive) {
    let Ok(text) = arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_text()) else {
        set_text(
            app,
            FEEDBACK,
            "Clipboard unavailable. You can type a test passphrase.",
        );
        return;
    };
    if text.chars().any(char::is_control) {
        set_text(
            app,
            FEEDBACK,
            "Paste a single line without control characters.",
        );
        return;
    }
    app.call_on_name(PASSPHRASE, |input: &mut EditView| {
        let cursor = input.get_cursor();
        let mut updated = (*input.get_content()).clone();
        updated.insert_str(cursor, &text);
        input.set_content(updated);
        input.set_cursor(cursor + text.len());
    });
    set_text(app, FEEDBACK, "");
}

fn preview_selection(app: &mut Cursive) {
    let selected = app
        .call_on_name(KEY_LIST, |list: &mut SelectView<SigningKey>| {
            list.selection()
        })
        .flatten();
    let Some(key) = selected else {
        set_text(app, FEEDBACK, "Select a signing key first.");
        return;
    };
    let entered = app
        .call_on_name(PASSPHRASE, |input: &mut EditView| {
            let entered = !input.get_content().is_empty();
            if entered {
                input.set_content("");
            }
            entered
        })
        .unwrap_or(false);
    if !entered {
        set_text(
            app,
            FEEDBACK,
            "Enter a test passphrase to exercise the masked field.",
        );
        return;
    }
    reset_confirmation(app);
    set_text(app, FEEDBACK, "");
    app.add_layer(
        Dialog::info(format!(
            "Selected {}\n{}\n\nThe test passphrase was cleared.",
            key.identity, key.fingerprint
        ))
        .title(" Selection preview "),
    );
}

fn reset_confirmation(app: &mut Cursive) {
    app.call_on_name(CONFIRMATION, |checkbox: &mut Checkbox| checkbox.uncheck());
    app.call_on_name(PREVIEW, Button::disable);
}

fn set_text(app: &mut Cursive, view: &str, text: &str) {
    app.call_on_name(view, |status: &mut TextView| status.set_content(text));
}
