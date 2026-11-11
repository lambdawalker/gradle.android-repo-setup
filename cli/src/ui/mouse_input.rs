//! Decode mouse reports delivered as keystrokes by Windows IDE terminals.
use cursive::{
    backend::Backend,
    event::{Event, Key, MouseButton, MouseEvent},
};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

const ESCAPE_WAIT: Duration = Duration::from_millis(50);
const REPORT_WAIT: Duration = Duration::from_millis(250);

pub(super) struct MouseInput {
    inner: Box<dyn Backend>,
    pending: Vec<Event>,
    characters: Vec<char>,
    started: Option<Instant>,
    ready: VecDeque<Event>,
    pressed: Option<MouseButton>,
}
impl MouseInput {
    fn new(inner: Box<dyn Backend>) -> Self {
        Self {
            inner,
            pending: Vec::new(),
            characters: Vec::new(),
            started: None,
            ready: VecDeque::new(),
            pressed: None,
        }
    }
    fn flush(&mut self) {
        self.ready.extend(self.pending.drain(..));
        self.characters.clear();
        self.started = None;
    }
    fn expire(&mut self, now: Instant) {
        let wait = if self.characters.len() <= 2 {
            ESCAPE_WAIT
        } else {
            REPORT_WAIT
        };
        if self
            .started
            .is_some_and(|start| now.saturating_duration_since(start) >= wait)
        {
            self.flush();
        }
    }
    fn accept(&mut self, event: Event, now: Instant) {
        // A new report may immediately follow a real Escape or interrupted report.
        // Replay the old input, then parse this prefix instead of leaking its tail.
        if !self.pending.is_empty()
            && matches!(
                event,
                Event::Key(Key::Esc) | Event::Char('\x1b') | Event::AltChar('[')
            )
        {
            self.flush();
        }
        if self.pending.is_empty() {
            match event {
                Event::Key(Key::Esc) | Event::Char('\x1b') => self.characters.push('\x1b'),
                Event::AltChar('[') => self.characters.extend(['\x1b', '[']),
                _ => {
                    self.ready.push_back(event);
                    return;
                }
            }
            self.started = Some(now);
            self.pending.push(event);
            return;
        }
        // Resize/refresh events can interleave with a fragmented report.
        if matches!(event, Event::Refresh | Event::WindowResize) {
            self.ready.push_back(event);
            return;
        }
        if let Event::Char(c) = event {
            self.characters.push(c);
            self.pending.push(Event::Char(c));
        } else {
            self.flush();
            self.ready.push_back(event);
            return;
        }
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
                let event = if code & 128 != 0 {
                    None
                } else if code & 64 != 0 {
                    match code & 3 {
                        0 => Some(MouseEvent::WheelUp),
                        1 => Some(MouseEvent::WheelDown),
                        _ => None,
                    }
                } else if release || (legacy && code & 3 == 3 && code & 32 == 0) {
                    let released = if legacy {
                        self.pressed.take()
                    } else {
                        self.pressed = None;
                        button
                    };
                    released.map(MouseEvent::Release)
                } else if code & 32 != 0 {
                    button.map(MouseEvent::Hold)
                } else {
                    button.map(|b| {
                        self.pressed = Some(b);
                        MouseEvent::Press(b)
                    })
                };
                if let Some(event) = event {
                    self.ready.push_back(Event::Mouse {
                        offset: (0, 0).into(),
                        position: (x - 1, y - 1).into(),
                        event,
                    });
                }
            }
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
            if code < 32 || code > 287 || x <= 32 || y <= 32 {
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
impl Backend for MouseInput {
    fn poll_event(&mut self) -> Option<Event> {
        if let Some(event) = self.ready.pop_front() {
            return Some(event);
        }
        // Bound work per poll so a mouse-motion flood cannot starve rendering or callbacks.
        for _ in 0..128 {
            let now = Instant::now();
            match self.inner.poll_event() {
                Some(event) => self.accept(event, now),
                None => {
                    // Drain already queued bytes before timing out a prefix: the UI may have
                    // paused between polls even though the rest of the report is available.
                    self.expire(now);
                    return self.ready.pop_front();
                }
            }
            if let Some(event) = self.ready.pop_front() {
                return Some(event);
            }
        }
        None
    }
    fn set_title(&mut self, title: String) {
        self.inner.set_title(title)
    }
    fn refresh(&mut self) {
        self.inner.refresh()
    }
    fn has_colors(&self) -> bool {
        self.inner.has_colors()
    }
    fn screen_size(&self) -> cursive::Vec2 {
        self.inner.screen_size()
    }
    fn move_to(&self, p: cursive::Vec2) {
        self.inner.move_to(p)
    }
    fn print(&self, t: &str) {
        self.inner.print(t)
    }
    fn clear(&self, c: cursive::theme::Color) {
        self.inner.clear(c)
    }
    fn set_color(&self, c: cursive::theme::ColorPair) -> cursive::theme::ColorPair {
        self.inner.set_color(c)
    }
    fn set_effect(&self, e: cursive::theme::Effect) {
        self.inner.set_effect(e)
    }
    fn unset_effect(&self, e: cursive::theme::Effect) {
        self.inner.unset_effect(e)
    }
    fn is_persistent(&self) -> bool {
        self.inner.is_persistent()
    }
    fn name(&self) -> &str {
        self.inner.name()
    }
}

#[cfg(windows)]
pub(super) fn init() -> std::io::Result<Box<dyn Backend>> {
    let backend = cursive::backends::crossterm::Backend::init()?;
    Ok(Box::new(MouseInput::new(backend)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cursive::{
        event::{Key, MouseButton, MouseEvent},
        view::View,
        views::EditView,
    };
    use std::cell::RefCell;
    struct Events(RefCell<VecDeque<Event>>);
    impl Backend for Events {
        fn poll_event(&mut self) -> Option<Event> {
            self.0.borrow_mut().pop_front()
        }
        fn set_title(&mut self, _: String) {}
        fn refresh(&mut self) {}
        fn has_colors(&self) -> bool {
            false
        }
        fn screen_size(&self) -> cursive::Vec2 {
            (80, 24).into()
        }
        fn move_to(&self, _: cursive::Vec2) {}
        fn print(&self, _: &str) {}
        fn clear(&self, _: cursive::theme::Color) {}
        fn set_color(&self, c: cursive::theme::ColorPair) -> cursive::theme::ColorPair {
            c
        }
        fn set_effect(&self, _: cursive::theme::Effect) {}
        fn unset_effect(&self, _: cursive::theme::Effect) {}
    }
    fn run(events: Vec<Event>) -> Vec<Event> {
        let mut backend = MouseInput::new(Box::new(Events(RefCell::new(events.into()))));
        let mut out = Vec::new();
        for _ in 0..256 {
            if let Some(e) = backend.poll_event() {
                out.push(e)
            }
        }
        out
    }
    fn chars(text: &str) -> Vec<Event> {
        text.chars().map(Event::Char).collect()
    }
    #[test]
    fn windows_ide_mouse_motion_does_not_type_in_password_field() {
        for report in ["\x1b[<35;30;12M", "\x1b[MC>9", "\x1b[<3;30;12M"] {
            let mut field = EditView::new().secret().content("keep-this-password");
            for event in run(chars(report)) {
                field.on_event(event);
            }
            assert_eq!(
                &*field.get_content(),
                "keep-this-password",
                "report {report:?}"
            );
        }
    }
    #[test]
    fn mouse_reports_still_deliver_clicks_and_wheel_events() {
        let actual = run(chars("\x1b[<0;8;4M\x1b[<0;8;4m\x1b[<64;8;4M\x1b[<65;8;4M"));
        let expected = [
            MouseEvent::Press(MouseButton::Left),
            MouseEvent::Release(MouseButton::Left),
            MouseEvent::WheelUp,
            MouseEvent::WheelDown,
        ]
        .map(|event| Event::Mouse {
            offset: (0, 0).into(),
            position: (7, 3).into(),
            event,
        });
        assert_eq!(actual, expected);
    }
    #[test]
    fn ordinary_text_and_native_keys_remain_unchanged() {
        let mut events = chars("abc[<35;8;4M contraseña🔑");
        events.push(Event::Key(Key::Tab));
        assert_eq!(run(events.clone()), events);
    }
    #[test]
    fn queued_mouse_tail_survives_a_delayed_poll() {
        let mut backend =
            MouseInput::new(Box::new(Events(RefCell::new(chars("[<35;8;4Mz").into()))));
        backend.accept(Event::Char('\x1b'), Instant::now() - Duration::from_secs(1));
        assert_eq!(backend.poll_event(), Some(Event::Char('z')));
    }

    #[test]
    fn escape_and_incomplete_input_have_bounded_wait_without_losing_text() {
        let mut backend = MouseInput::new(Box::new(Events(RefCell::new(VecDeque::new()))));
        let now = Instant::now();
        backend.accept(Event::Key(Key::Esc), now);
        backend.expire(now + ESCAPE_WAIT);
        assert_eq!(backend.ready.pop_front(), Some(Event::Key(Key::Esc)));
        for event in chars("\x1b[<12;") {
            backend.accept(event, now);
        }
        backend.expire(now + REPORT_WAIT);
        assert_eq!(
            backend.ready.drain(..).collect::<Vec<_>>(),
            chars("\x1b[<12;")
        );
        assert!(backend.pending.is_empty());
    }

    #[test]
    fn invalid_escape_sequences_and_native_mouse_events_are_preserved() {
        let mut events = chars("a\x1b[not-a-reportb");
        events.push(Event::Mouse {
            offset: (0, 0).into(),
            position: (2, 3).into(),
            event: MouseEvent::WheelDown,
        });
        assert_eq!(run(events.clone()), events);
    }

    #[test]
    fn fragmented_reports_do_not_reach_the_view_and_leave_following_typing_intact() {
        let mut backend = MouseInput::new(Box::new(Events(RefCell::new(VecDeque::new()))));
        let now = Instant::now();
        for chunk in ["\x1b[<35;", "8;4M", "\x1b[MC>9"] {
            for event in chars(chunk) {
                backend.accept(event, now);
            }
            assert!(backend.ready.is_empty());
        }
        backend.accept(Event::Char('é'), now);
        assert_eq!(backend.ready.pop_front(), Some(Event::Char('é')));
    }
    #[test]
    fn a_new_mouse_prefix_is_decoded_after_interrupted_escape_input() {
        let mut events = vec![Event::Key(Key::Esc), Event::Key(Key::Esc)];
        events.extend(chars("[<35;8;4M"));
        assert_eq!(run(events), [Event::Key(Key::Esc)]);
        let mut events = chars("\x1b[<12;\x1b[<35;8;4M");
        events.push(Event::Char('z'));
        let mut expected = chars("\x1b[<12;");
        expected.push(Event::Char('z'));
        assert_eq!(run(events), expected);
    }

    #[test]
    fn legacy_click_drag_release_and_wheel_keep_their_positions() {
        let reports = "\x1b[M ($\x1b[M@)$\x1b[M#)$\x1b[M`)$";
        let mouse = |event, position| Event::Mouse {
            offset: (0, 0).into(),
            position,
            event,
        };
        assert_eq!(
            run(chars(reports)),
            [
                mouse(MouseEvent::Press(MouseButton::Left), (7, 3).into()),
                mouse(MouseEvent::Hold(MouseButton::Left), (8, 3).into()),
                mouse(MouseEvent::Release(MouseButton::Left), (8, 3).into()),
                mouse(MouseEvent::WheelUp, (8, 3).into()),
            ]
        );
    }

    #[test]
    fn alt_bracket_prefix_and_sgr_drag_are_decoded() {
        let mut events = vec![Event::AltChar('[')];
        events.extend(chars("<32;8;4M"));
        assert_eq!(
            run(events),
            [Event::Mouse {
                offset: (0, 0).into(),
                position: (7, 3).into(),
                event: MouseEvent::Hold(MouseButton::Left)
            }]
        );
    }
}
