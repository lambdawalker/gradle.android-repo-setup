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
    Ok(WindowsTerminal {
        output: Box::new(io::stdout()),
        size: terminal::size().ok(),
        fullscreen: false,
        mouse: false,
        paste: false,
        raw: false,
        input_started: false,
    })
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
    fn fullscreen_frame_clears_before_paint_and_omits_final_newline() {
        let canvas = Canvas::new(80, 24);
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &canvas).unwrap();
        assert!(bytes.starts_with(b"\x1b[1;1H\x1b[2J"));
        assert!(!bytes.ends_with(b"\r\n"));
        assert_eq!(bytes.windows(2).filter(|pair| *pair == b"\r\n").count(), 23);
    }
    #[test]
    fn drop_restores_other_console_modes_after_a_write_error() {
        use std::sync::{Arc, Mutex};
        struct FailFirst {
            failed: bool,
            bytes: Arc<Mutex<Vec<u8>>>,
        }
        impl Write for FailFirst {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if !self.failed {
                    self.failed = true;
                    return Err(io::ErrorKind::BrokenPipe.into());
                }
                self.bytes.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let bytes = Arc::new(Mutex::new(Vec::new()));
        drop(WindowsTerminal {
            output: Box::new(FailFirst {
                failed: false,
                bytes: bytes.clone(),
            }),
            size: None,
            fullscreen: true,
            mouse: true,
            paste: true,
            raw: false,
            input_started: false,
        });
        let bytes = bytes.lock().unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.contains("\x1b[?2004l"),
            "bracketed paste restoration was skipped"
        );
        assert!(
            text.contains("\x1b[?1049l"),
            "alternate screen restoration was skipped"
        );
        assert!(text.contains("\x1b[?25h"), "cursor restoration was skipped");
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

impl Drop for WindowsTerminal {
    fn drop(&mut self) {
        // Attempt every restoration even when a preceding console write fails.
        let _ = self.set_mouse_capture(false);
        let _ = self.set_bracketed_paste(false);
        let _ = self.set_fullscreen(false);
        let _ = self.output.queue(cursor::Show).and_then(Write::flush);
        if self.raw {
            let _ = terminal::disable_raw_mode();
        }
    }
}
