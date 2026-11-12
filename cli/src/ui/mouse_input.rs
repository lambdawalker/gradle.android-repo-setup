//! Decode mouse reports delivered as keystrokes by Windows IDE terminals.
use futures::{stream::BoxStream, Stream};
use iocraft::{
    FullscreenMouseEvent, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
    TerminalEvent,
};
use std::{
    collections::VecDeque,
    io,
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, Instant},
};

const ESCAPE_WAIT: Duration = Duration::from_millis(50);
const REPORT_WAIT: Duration = Duration::from_millis(250);

#[derive(Default)]
struct Decoder {
    pending: Vec<TerminalEvent>,
    characters: Vec<char>,
    started: Option<Instant>,
    ready: VecDeque<TerminalEvent>,
    pressed: Option<MouseButton>,
}
impl Decoder {
    fn flush(&mut self) {
        self.ready.extend(self.pending.drain(..));
        self.characters.clear();
        self.started = None;
    }
    fn deadline(&self) -> Option<Instant> {
        self.started.map(|start| {
            start
                + if self.characters.len() <= 2 {
                    ESCAPE_WAIT
                } else {
                    REPORT_WAIT
                }
        })
    }
    fn expire(&mut self, now: Instant) {
        if self.deadline().is_some_and(|deadline| now >= deadline) {
            self.flush();
        }
    }
    fn accept(&mut self, event: TerminalEvent, now: Instant) {
        let prefix = match &event {
            TerminalEvent::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Esc | KeyCode::Char('\x1b') if key.modifiers.is_empty() => Some(false),
                KeyCode::Char('[') if key.modifiers == KeyModifiers::ALT => Some(true),
                _ => None,
            },
            _ => None,
        };
        if let Some(alt_bracket) = prefix {
            self.flush();
            self.characters.push('\x1b');
            if alt_bracket {
                self.characters.push('[');
            }
            self.started = Some(now);
            self.pending.push(event);
            return;
        }
        if self.pending.is_empty() {
            self.ready.push_back(event);
            return;
        }
        match &event {
            TerminalEvent::Key(key)
                if key.kind != KeyEventKind::Release
                    && (key.modifiers - KeyModifiers::SHIFT).is_empty() =>
            {
                if let KeyCode::Char(c) = key.code {
                    self.characters.push(c);
                } else {
                    self.flush();
                    self.ready.push_back(event);
                    return;
                }
            }
            // Native events and key releases cannot form report bytes.
            TerminalEvent::Resize(..) | TerminalEvent::FullscreenMouse(..) => {
                self.ready.push_back(event);
                return;
            }
            TerminalEvent::Key(key) if key.kind == KeyEventKind::Release => {
                self.ready.push_back(event);
                return;
            }
            _ => {
                self.flush();
                self.ready.push_back(event);
                return;
            }
        }
        self.pending.push(event);
        match parse(&self.characters) {
            Report::Incomplete => {}
            Report::Invalid => self.flush(),
            Report::Complete {
                code,
                x,
                y,
                release,
                legacy,
            } => {
                self.pending.clear();
                self.characters.clear();
                self.started = None;
                let button = match code & 3 {
                    0 => Some(MouseButton::Left),
                    1 => Some(MouseButton::Middle),
                    2 => Some(MouseButton::Right),
                    _ => None,
                };
                let kind = if code & 128 != 0 {
                    None
                } else if code & 64 != 0 {
                    match code & 3 {
                        0 => Some(MouseEventKind::ScrollUp),
                        1 => Some(MouseEventKind::ScrollDown),
                        2 => Some(MouseEventKind::ScrollLeft),
                        _ => Some(MouseEventKind::ScrollRight),
                    }
                } else if release || (legacy && code & 3 == 3 && code & 32 == 0) {
                    let released = if legacy {
                        self.pressed.take()
                    } else {
                        self.pressed = None;
                        button
                    };
                    released.map(MouseEventKind::Up)
                } else if code & 32 != 0 {
                    button.map(MouseEventKind::Drag)
                } else {
                    button.map(|b| {
                        self.pressed = Some(b);
                        MouseEventKind::Down(b)
                    })
                };
                if let Some(kind) = kind {
                    let mut event = FullscreenMouseEvent::new(kind, (x - 1) as u16, (y - 1) as u16);
                    if code & 4 != 0 {
                        event.modifiers |= KeyModifiers::SHIFT;
                    }
                    if code & 8 != 0 {
                        event.modifiers |= KeyModifiers::ALT;
                    }
                    if code & 16 != 0 {
                        event.modifiers |= KeyModifiers::CONTROL;
                    }
                    self.ready.push_back(TerminalEvent::FullscreenMouse(event));
                }
            }
        }
    }
}

/// The timer is polled alongside input so a lone Escape wakes the render loop.
pub(super) struct MouseInput {
    inner: BoxStream<'static, io::Result<TerminalEvent>>,
    decoder: Decoder,
    timer: Option<(Instant, Pin<Box<smol::Timer>>)>,
    ended: bool,
    error: Option<io::Error>,
}
impl MouseInput {
    pub(super) fn new(inner: BoxStream<'static, io::Result<TerminalEvent>>) -> Self {
        Self {
            inner,
            decoder: Decoder::default(),
            timer: None,
            ended: false,
            error: None,
        }
    }
}
impl Stream for MouseInput {
    type Item = io::Result<TerminalEvent>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(event) = self.decoder.ready.pop_front() {
            return Poll::Ready(Some(Ok(event)));
        }
        // Drain queued tails before considering timeouts, with a fairness budget.
        for _ in 0..128 {
            if self.ended {
                break;
            }
            match self.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(event))) => self.decoder.accept(event, Instant::now()),
                Poll::Ready(result) => {
                    self.ended = true;
                    self.error = result.and_then(Result::err);
                    self.decoder.flush();
                }
                Poll::Pending => {
                    self.decoder.expire(Instant::now());
                    if let Some(deadline) = self.decoder.deadline() {
                        if self.timer.as_ref().map(|(at, _)| *at) != Some(deadline) {
                            self.timer = Some((deadline, Box::pin(smol::Timer::at(deadline))));
                        }
                        if std::future::Future::poll(self.timer.as_mut().unwrap().1.as_mut(), cx)
                            .is_ready()
                        {
                            self.decoder.expire(Instant::now());
                        }
                    } else {
                        self.timer = None;
                    }
                    if let Some(event) = self.decoder.ready.pop_front() {
                        return Poll::Ready(Some(Ok(event)));
                    }
                    return Poll::Pending;
                }
            }
            if let Some(event) = self.decoder.ready.pop_front() {
                return Poll::Ready(Some(Ok(event)));
            }
        }
        if self.ended {
            if let Some(error) = self.error.take() {
                Poll::Ready(Some(Err(error)))
            } else {
                Poll::Ready(None)
            }
        } else {
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

enum Report {
    Incomplete,
    Invalid,
    Complete {
        code: u16,
        x: usize,
        y: usize,
        release: bool,
        legacy: bool,
    },
}
fn parse(chars: &[char]) -> Report {
    if chars.len() < 2 {
        return Report::Incomplete;
    }
    if chars[1] != '[' {
        return Report::Invalid;
    }
    if chars.len() < 3 {
        return Report::Incomplete;
    }
    match chars[2] {
        'M' => {
            if chars.len() < 6 {
                return Report::Incomplete;
            }
            let (code, x, y) = (chars[3] as u32, chars[4] as u32, chars[5] as u32);
            if code < 32 || code > 287 || x <= 32 || y <= 32 || x > 65568 || y > 65568 {
                return Report::Invalid;
            }
            Report::Complete {
                code: (code - 32) as u16,
                x: (x - 32) as usize,
                y: (y - 32) as usize,
                release: false,
                legacy: true,
            }
        }
        '<' => {
            if chars.len() > 48 {
                return Report::Invalid;
            }
            let last = *chars.last().unwrap();
            let complete = last == 'M' || last == 'm';
            let fields = &chars[3..chars.len() - usize::from(complete)];
            if fields.iter().any(|c| !c.is_ascii_digit() && *c != ';') {
                return Report::Invalid;
            }
            if !complete {
                return Report::Incomplete;
            }
            let text: String = fields.iter().collect();
            let values: Vec<_> = text.split(';').map(str::parse::<u16>).collect();
            if let [Ok(code), Ok(x), Ok(y)] = values.as_slice() {
                if *x > 0 && *y > 0 {
                    return Report::Complete {
                        code: *code,
                        x: *x as usize,
                        y: *y as usize,
                        release: last == 'm',
                        legacy: false,
                    };
                }
            }
            Report::Invalid
        }
        _ => Report::Invalid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{stream, StreamExt};
    use iocraft::KeyEvent;
    fn key(code: KeyCode) -> TerminalEvent {
        TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, code))
    }
    fn chars(text: &str) -> Vec<TerminalEvent> {
        text.chars().map(|c| key(KeyCode::Char(c))).collect()
    }
    fn debug(events: &[TerminalEvent]) -> String {
        format!("{events:?}")
    }
    fn run(events: Vec<TerminalEvent>) -> Vec<TerminalEvent> {
        smol::block_on(
            MouseInput::new(stream::iter(events.into_iter().map(Ok)).boxed())
                .map(Result::unwrap)
                .collect(),
        )
    }
    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> TerminalEvent {
        TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(kind, column, row))
    }
    fn equal(actual: Vec<TerminalEvent>, expected: Vec<TerminalEvent>) {
        assert_eq!(debug(&actual), debug(&expected));
    }
    #[iocraft::component]
    fn PasswordProbe(mut hooks: iocraft::Hooks) -> impl Into<iocraft::AnyElement<'static>> {
        use iocraft::prelude::*;
        let mut password = hooks.use_state(|| String::from("keep-this-password"));
        let mut system = hooks.use_context_mut::<SystemContext>();
        hooks.use_terminal_events(move |event| {
            if let TerminalEvent::Key(key) = event {
                if key.kind != KeyEventKind::Release {
                    if let KeyCode::Char(c) = key.code {
                        let next = format!("{}{c}", password.read().as_str());
                        password.set(next);
                    }
                }
            }
        });
        if password.read().ends_with('z') {
            system.exit();
        }
        element!(Text(content: password.read().to_string()))
    }
    #[test]
    fn decoded_stream_integrates_with_iocraft_mock_terminal() {
        use iocraft::prelude::*;
        let input = MouseInput::new(
            stream::iter(chars("\x1b[<35;8;4M\x1b[MC>9z").into_iter().map(Ok)).boxed(),
        );
        let canvases = smol::block_on(
            element!(PasswordProbe)
                .mock_terminal_render_loop(MockTerminalConfig::with_events(
                    input.map(Result::unwrap),
                ))
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            canvases.last().unwrap().to_string(),
            "keep-this-passwordz\n"
        );
    }
    #[test]
    fn windows_ide_mouse_motion_does_not_type_in_password_field() {
        for report in ["\x1b[<35;30;12M", "\x1b[MC>9", "\x1b[<3;30;12M"] {
            // No raw key reaches the password editor; its existing text stays intact.
            assert!(run(chars(report)).is_empty(), "report {report:?}");
        }
    }
    #[test]
    fn mouse_reports_still_deliver_clicks_and_wheel_events() {
        equal(
            run(chars("\x1b[<0;8;4M\x1b[<0;8;4m\x1b[<64;8;4M\x1b[<65;8;4M")),
            [
                MouseEventKind::Down(MouseButton::Left),
                MouseEventKind::Up(MouseButton::Left),
                MouseEventKind::ScrollUp,
                MouseEventKind::ScrollDown,
            ]
            .into_iter()
            .map(|kind| mouse(kind, 7, 3))
            .collect(),
        );
    }
    #[test]
    fn ordinary_text_and_native_keys_remain_unchanged() {
        let mut events = chars("abc[<35;8;4M contraseña🔑");
        events.push(key(KeyCode::Tab));
        equal(run(events.clone()), events);
    }
    #[test]
    fn queued_mouse_tail_survives_a_delayed_poll() {
        let mut input =
            MouseInput::new(stream::iter(chars("[<35;8;4Mz").into_iter().map(Ok)).boxed());
        input
            .decoder
            .accept(key(KeyCode::Esc), Instant::now() - Duration::from_secs(1));
        equal(
            smol::block_on(input.map(Result::unwrap).collect()),
            chars("z"),
        );
    }
    #[test]
    fn escape_and_incomplete_input_have_bounded_wait_without_losing_text() {
        let mut decoder = Decoder::default();
        let now = Instant::now();
        decoder.accept(key(KeyCode::Esc), now);
        decoder.expire(now + ESCAPE_WAIT);
        equal(decoder.ready.drain(..).collect(), vec![key(KeyCode::Esc)]);
        for event in chars("\x1b[<12;") {
            decoder.accept(event, now);
        }
        decoder.expire(now + REPORT_WAIT);
        equal(decoder.ready.drain(..).collect(), chars("\x1b[<12;"));
        assert!(decoder.pending.is_empty());
    }
    #[test]
    fn invalid_escape_sequences_and_native_mouse_events_are_preserved() {
        let mut events = chars("a\x1b[not-a-reportb");
        events.push(mouse(MouseEventKind::ScrollDown, 2, 3));
        equal(run(events.clone()), events);
    }
    #[test]
    fn fragmented_reports_do_not_reach_the_view_and_leave_following_typing_intact() {
        let mut decoder = Decoder::default();
        let now = Instant::now();
        for chunk in ["\x1b[<35;", "8;4M", "\x1b[MC>9"] {
            for event in chars(chunk) {
                decoder.accept(event, now);
            }
            assert!(decoder.ready.is_empty());
        }
        decoder.accept(key(KeyCode::Char('é')), now);
        equal(decoder.ready.drain(..).collect(), chars("é"));
    }
    #[test]
    fn a_new_mouse_prefix_is_decoded_after_interrupted_escape_input() {
        let mut events = vec![key(KeyCode::Esc), key(KeyCode::Esc)];
        events.extend(chars("[<35;8;4M"));
        equal(run(events), vec![key(KeyCode::Esc)]);
        equal(run(chars("\x1b[<12;\x1b[<35;8;4Mz")), chars("\x1b[<12;z"));
    }
    #[test]
    fn legacy_click_drag_release_and_wheel_keep_their_positions() {
        equal(
            run(chars("\x1b[M ($\x1b[M@)$\x1b[M#)$\x1b[M`)$")),
            vec![
                mouse(MouseEventKind::Down(MouseButton::Left), 7, 3),
                mouse(MouseEventKind::Drag(MouseButton::Left), 8, 3),
                mouse(MouseEventKind::Up(MouseButton::Left), 8, 3),
                mouse(MouseEventKind::ScrollUp, 8, 3),
            ],
        );
    }
    #[test]
    fn alt_bracket_prefix_and_sgr_drag_are_decoded() {
        let mut prefix = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('['));
        prefix.modifiers = KeyModifiers::ALT;
        let mut events = vec![TerminalEvent::Key(prefix)];
        events.extend(chars("<32;8;4M"));
        equal(
            run(events),
            vec![mouse(MouseEventKind::Drag(MouseButton::Left), 7, 3)],
        );
    }
    #[test]
    fn lone_escape_timer_wakes_without_new_input() {
        smol::block_on(async {
            let source = stream::iter(vec![Ok(key(KeyCode::Esc))]).chain(stream::pending());
            let mut input = MouseInput::new(source.boxed());
            let started = Instant::now();
            let result = futures::future::select(
                input.next(),
                Box::pin(smol::Timer::after(Duration::from_secs(2))),
            )
            .await;
            match result {
                futures::future::Either::Left((Some(Ok(event)), _)) => {
                    equal(vec![event], vec![key(KeyCode::Esc)])
                }
                _ => panic!("Escape failed to wake its own stream"),
            }
            assert!(started.elapsed() >= ESCAPE_WAIT);
        });
    }
    #[test]
    fn resize_paste_key_release_and_repeat_preserve_input() {
        let mut repeat = KeyEvent::new(KeyEventKind::Repeat, KeyCode::Char('é'));
        repeat.modifiers = KeyModifiers::SHIFT;
        let events = vec![
            TerminalEvent::Resize(80, 24),
            TerminalEvent::BracketedPaste("\x1b[<35;8;4M contraseña🔑".into()),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Release, KeyCode::Char('x'))),
            TerminalEvent::Key(repeat),
        ];
        equal(run(events.clone()), events);
    }
    #[test]
    fn errors_and_eof_replay_pending_input_then_finish() {
        let events = stream::iter(vec![
            Ok(key(KeyCode::Esc)),
            Err(io::Error::from(io::ErrorKind::BrokenPipe)),
        ]);
        let actual = smol::block_on(MouseInput::new(events.boxed()).collect::<Vec<_>>());
        assert_eq!(actual.len(), 2);
        assert!(matches!(&actual[0], Ok(TerminalEvent::Key(event)) if event.code == KeyCode::Esc));
        assert_eq!(
            actual[1].as_ref().unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
        equal(run(chars("\x1b[<12;")), chars("\x1b[<12;"));
    }
    #[test]
    fn malformed_report_length_is_bounded_and_unicode_is_replayed() {
        let input = format!("\x1b[<{}🔑", "1".repeat(80));
        equal(run(chars(&input)), chars(&input));
    }
    #[test]
    fn mouse_motion_flood_yields_and_eventually_delivers_text() {
        let input = format!("{}z", "\x1b[<35;8;4M".repeat(1000));
        equal(run(chars(&input)), chars("z"));
    }
}
