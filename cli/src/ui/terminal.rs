//! Fullscreen Windows adapter: iocraft owns layout; crossterm owns the console.
//! Keep a single native input stream and repair IDE mouse reports before dispatch.
use super::mouse_input::MouseInput;
use crossterm::{cursor, event, terminal, QueueableCommand};
use futures::{stream::BoxStream, StreamExt};
use iocraft::{
    Canvas, FullscreenMouseEvent, KeyEvent, Output, Passthrough, TerminalBackend, TerminalEvent,
};
use std::io::{self, IsTerminal, Write};

pub(super) struct WindowsTerminal {
    output: Box<dyn Write + Send>,
    size: Option<(u16, u16)>,
    fullscreen: bool,
    mouse: bool,
    paste: bool,
    raw: bool,
    input_started: bool,
}

pub(super) fn new() -> io::Result<WindowsTerminal> {
    let mut backend = WindowsTerminal {
        output: Box::new(io::stdout()),
        size: terminal::size().ok(),
        fullscreen: false,
        mouse: false,
        paste: false,
        raw: false,
        input_started: false,
    };
    if !mouse_capture_requested(
        true,
        std::env::var("TERMINAL_EMULATOR").ok().as_deref(),
        std::env::var("ANDROID_REPO_SETUP_NO_MOUSE").ok().as_deref(),
    ) {
        // Clear both console and terminal-emulator tracking left by a previous
        // run. On Windows QueueableCommand uses WinAPI for this command, so
        // explicitly send the ANSI reset as well.
        backend.output.queue(event::DisableMouseCapture)?;
        write_mouse_reset(&mut backend.output)?;
        backend.output.flush()?;
    }
    Ok(backend)
}

impl TerminalBackend for WindowsTerminal {
    fn refresh_size(&mut self) {
        self.size = terminal::size().ok();
    }
    fn size(&self) -> Option<(u16, u16)> {
        self.size
    }
    fn set_fullscreen(&mut self, enabled: bool) -> io::Result<()> {
        if self.fullscreen != enabled {
            // Remember enabling before writing so Drop also restores partial writes.
            if enabled {
                self.fullscreen = true;
            }
            if enabled {
                self.output
                    .queue(terminal::EnterAlternateScreen)?
                    .queue(cursor::Hide)?;
            } else {
                self.output
                    .queue(cursor::Show)?
                    .queue(terminal::LeaveAlternateScreen)?;
            }
            self.output.flush()?;
            self.fullscreen = enabled;
        }
        Ok(())
    }
    fn set_mouse_capture(&mut self, enabled: bool) -> io::Result<()> {
        let enabled = mouse_capture_requested(
            enabled,
            std::env::var("TERMINAL_EMULATOR").ok().as_deref(),
            std::env::var("ANDROID_REPO_SETUP_NO_MOUSE").ok().as_deref(),
        );
        if self.mouse != enabled {
            // Remember enabling before writing so Drop also restores partial writes.
            if enabled {
                self.mouse = true;
            }
            if enabled {
                self.output.queue(event::EnableMouseCapture)?;
            } else {
                self.output.queue(event::DisableMouseCapture)?;
            }
            self.output.flush()?;
            self.mouse = enabled;
        }
        Ok(())
    }
    fn set_bracketed_paste(&mut self, enabled: bool) -> io::Result<()> {
        if self.paste != enabled {
            // Remember enabling before writing so Drop also restores partial writes.
            if enabled {
                self.paste = true;
            }
            if enabled {
                self.output.queue(event::EnableBracketedPaste)?;
            } else {
                self.output.queue(event::DisableBracketedPaste)?;
            }
            self.output.flush()?;
            self.paste = enabled;
        }
        Ok(())
    }
    fn begin_frame(&mut self) -> io::Result<()> {
        self.output.queue(terminal::BeginSynchronizedUpdate)?;
        Ok(())
    }
    fn end_frame(&mut self) -> io::Result<()> {
        self.output.queue(terminal::EndSynchronizedUpdate)?.flush()
    }
    fn clear_canvas(&mut self) -> io::Result<()> {
        self.output
            .queue(cursor::MoveTo(0, 0))?
            .queue(terminal::Clear(terminal::ClearType::All))?;
        Ok(())
    }
    fn write_canvas(&mut self, _: Option<&Canvas>, canvas: &Canvas) -> io::Result<()> {
        write_frame(&mut self.output, canvas)
    }

    fn print_above(&mut self, messages: &[Passthrough<'_>]) -> io::Result<()> {
        // This fullscreen wizard does not emit passthrough output. Still honor
        // the trait if a future component supplies it; the next frame redraws.
        for message in messages {
            let text = message.content.replace("\r\n", "\n").replace('\n', "\r\n");
            let mut writer: Box<dyn Write> = match message.stream {
                Output::Stdout => Box::new(io::stdout()),
                Output::Stderr => Box::new(io::stderr()),
            };
            writer.write_all(text.as_bytes())?;
            if message.newline {
                writer.write_all(b"\r\n")?;
            }
            writer.flush()?;
        }
        Ok(())
    }
    fn event_stream(&mut self) -> io::Result<BoxStream<'static, io::Result<TerminalEvent>>> {
        if self.input_started {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "terminal input is already active",
            ));
        }
        if !io::stdin().is_terminal() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "wizard input requires a terminal",
            ));
        }
        terminal::enable_raw_mode()?;
        self.raw = true;
        self.input_started = true;
        let events = event::EventStream::new()
            .filter_map(|event| async move {
                match event {
                    Ok(event::Event::Key(native)) => {
                        let mut key = KeyEvent::new(native.kind.into(), native.code.into());
                        key.modifiers = native.modifiers.into();
                        Some(Ok(TerminalEvent::Key(key)))
                    }
                    Ok(event::Event::Mouse(native)) => {
                        let mut mouse = FullscreenMouseEvent::new(
                            native.kind.into(),
                            native.column,
                            native.row,
                        );
                        mouse.modifiers = native.modifiers.into();
                        Some(Ok(TerminalEvent::FullscreenMouse(mouse)))
                    }
                    Ok(event::Event::Resize(width, height)) => {
                        Some(Ok(TerminalEvent::Resize(width, height)))
                    }
                    Ok(event::Event::Paste(text)) => Some(Ok(TerminalEvent::BracketedPaste(text))),
                    Ok(_) => None,
                    Err(error) => Some(Err(error)),
                }
            })
            .boxed();
        Ok(MouseInput::new(events).boxed())
    }
}

// Keep environment access outside the policy so tests do not mutate process state.
fn mouse_capture_requested(enabled: bool, terminal: Option<&str>, no_mouse: Option<&str>) -> bool {
    enabled
        && no_mouse != Some("1")
        && !terminal.is_some_and(|name| name.to_ascii_lowercase().starts_with("jetbrains"))
}

fn write_mouse_reset(output: &mut impl Write) -> io::Result<()> {
    // Explicit ANSI is required: QueueableCommand uses WinAPI on Windows.
    output.write_all(b"\x1b[?1006l\x1b[?1015l\x1b[?1003l\x1b[?1002l\x1b[?1000l")
}

fn write_frame(output: &mut impl Write, canvas: &Canvas) -> io::Result<()> {
    // Public Canvas serialization includes a final newline. Remove it so a
    // canvas filling the viewport does not scroll its first row off screen.
    let mut bytes = Vec::new();
    canvas.write_ansi(&mut bytes)?;
    if bytes.ends_with(b"\r\n") {
        bytes.truncate(bytes.len() - 2);
    }
    output
        .queue(cursor::MoveTo(0, 0))?
        .queue(terminal::Clear(terminal::ClearType::All))?;
    output.write_all(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keyboard_mode_resets_all_terminal_mouse_protocols() {
        let mut bytes = Vec::new();
        write_mouse_reset(&mut bytes).unwrap();
        assert_eq!(
            bytes,
            b"\x1b[?1006l\x1b[?1015l\x1b[?1003l\x1b[?1002l\x1b[?1000l"
        );
    }
    #[test]
    fn jetbrains_and_explicit_keyboard_mode_do_not_enable_mouse_reports() {
        assert!(!mouse_capture_requested(
            true,
            Some("JetBrains-JediTerm"),
            None
        ));
        assert!(!mouse_capture_requested(true, None, Some("1")));
        assert!(mouse_capture_requested(true, None, None));
        assert!(mouse_capture_requested(
            true,
            Some("Windows Terminal"),
            Some("0")
        ));
        assert!(!mouse_capture_requested(false, None, None));
    }
    #[test]
    fn fullscreen_frame_clears_before_paint_and_omits_final_newline() {
        let canvas = Canvas::new(80, 24);
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &canvas).unwrap();
        assert!(bytes.starts_with(b"\x1b[1;1H\x1b[2J"));
        assert!(!bytes.ends_with(b"\r\n"));
        assert_eq!(bytes.windows(2).filter(|pair| *pair == b"\r\n").count(), 23);
    }
    #[test]
    fn cleanup_attempts_every_mode_even_when_an_earlier_restoration_fails() {
        // Native Windows commands may bypass the writer entirely. Inject the
        // console operation boundary instead, without touching the host console.
        for failed in [
            Restore::Mouse,
            Restore::Paste,
            Restore::Fullscreen,
            Restore::Cursor,
            Restore::Raw,
        ] {
            let mut attempted = Vec::new();
            restore_modes(|mode| {
                attempted.push(mode);
                if mode == failed {
                    Err(io::ErrorKind::BrokenPipe.into())
                } else {
                    Ok(())
                }
            });
            assert_eq!(
                attempted,
                [
                    Restore::Mouse,
                    Restore::Paste,
                    Restore::Fullscreen,
                    Restore::Cursor,
                    Restore::Raw
                ]
            );
        }
    }
    #[test]
    fn renderer_reports_write_errors() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        assert_eq!(
            write_frame(&mut Broken, &Canvas::new(1, 1))
                .unwrap_err()
                .kind(),
            io::ErrorKind::BrokenPipe
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Restore {
    Mouse,
    Paste,
    Fullscreen,
    Cursor,
    Raw,
}

fn restore_modes(mut restore: impl FnMut(Restore) -> io::Result<()>) {
    // Each console operation is independent. A failed restoration must never
    // prevent the remaining modes from being restored, including raw input.
    for mode in [
        Restore::Mouse,
        Restore::Paste,
        Restore::Fullscreen,
        Restore::Cursor,
        Restore::Raw,
    ] {
        let _ = restore(mode);
    }
}

impl Drop for WindowsTerminal {
    fn drop(&mut self) {
        restore_modes(|mode| match mode {
            Restore::Mouse => self.set_mouse_capture(false),
            Restore::Paste => self.set_bracketed_paste(false),
            Restore::Fullscreen => self.set_fullscreen(false),
            Restore::Cursor => self.output.queue(cursor::Show).and_then(Write::flush),
            Restore::Raw if self.raw => terminal::disable_raw_mode(),
            Restore::Raw => Ok(()),
        });
    }
}
