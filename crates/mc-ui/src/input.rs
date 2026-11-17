//! Two-button gesture recognizer (docs/spec/ui.md, "Interaction model").
//!
//! Fed with raw press/release edges and a millisecond clock, it emits gestures:
//! - `Click`: press shorter than 600 ms, not followed by a second press within 300 ms;
//! - `Double`: two short presses within 300 ms (A: reported at the second release; B: once the
//!   window after it has passed, since B also counts further presses);
//! - `Triple`, `Quad`: three, four or more short presses of B, each within 300 ms of the last
//!   (Home's shortcuts, `docs/spec/ui.md` §3);
//! - while typing ([`Gestures::set_immediate_b`]), B neither waits nor counts: every short press
//!   is a `Click` at its release (B deletes a character there, §7);
//! - `Long`: released after 600 ms (and, for A, before the held ticks start at 1 s);
//! - `Held`: while a button stays down past 1 s, one tick every 250 ms (the app takes the first
//!   as a long press, hold-to-confirm dialogs count them down), then `HeldEnd` on release (A
//!   only).
//!
//! Pure and `no_std`: the same code runs on the device, in the emulator and in tests.

/// One of the two buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    /// Bottom button (GPIO21): next, previous, open.
    A,
    /// Top button (GPIO14): back, home, light.
    B,
}

/// A recognized gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    Click(Button),
    Double(Button),
    /// Three short presses (B only).
    Triple(Button),
    /// Four or more short presses (B only).
    Quad(Button),
    Long(Button),
    /// Still held; `ms` since the press.
    Held(Button, u32),
    /// A released after `Held` ticks (B reports `Long` instead): ends a hold-to-confirm.
    HeldEnd(Button),
}

pub const DEBOUNCE_MS: u64 = 30;
pub const DOUBLE_MS: u64 = 300;
pub const LONG_MS: u64 = 600;
pub const HOLD_MS: u64 = 1000;
pub const HOLD_TICK_MS: u64 = 250;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    /// Down since `at`, after `count` short presses in a row; `ticks` held ticks sent.
    Down {
        at: u64,
        count: u8,
        ticks: u32,
    },
    /// `count` short presses done; waiting for another one until `until`.
    WaitDouble {
        until: u64,
        count: u8,
    },
}

#[derive(Debug, Clone, Copy)]
struct Track {
    state: State,
    last_edge: u64,
    pressed: bool,
}

impl Track {
    const fn new() -> Self {
        Self {
            state: State::Idle,
            last_edge: 0,
            pressed: false,
        }
    }
}

/// Recognizer for both buttons.
#[derive(Debug, Clone)]
pub struct Gestures {
    a: Track,
    b: Track,
    /// B's short presses are clicks at once (text entry).
    immediate_b: bool,
    out: heapless::Deque<Gesture, 8>,
}

impl Default for Gestures {
    fn default() -> Self {
        Self::new()
    }
}

impl Gestures {
    pub const fn new() -> Self {
        Self {
            a: Track::new(),
            b: Track::new(),
            immediate_b: false,
            out: heapless::Deque::new(),
        }
    }

    fn track(&mut self, b: Button) -> &mut Track {
        match b {
            Button::A => &mut self.a,
            Button::B => &mut self.b,
        }
    }

    fn emit(&mut self, g: Gesture) {
        if self.out.is_full() {
            let _ = self.out.pop_front();
        }
        let _ = self.out.push_back(g);
    }

    /// While on, a short press of B is a `Click` at its release: no double-click window to
    /// wait for, no counting. The runner sets it from `App::typing`, where B deletes one
    /// character per press and several quick presses must delete several.
    pub fn set_immediate_b(&mut self, on: bool) {
        self.immediate_b = on;
    }

    /// A raw edge: `pressed` is the new level of `button` at `now_ms`. Bounces closer than
    /// 30 ms to the previous edge are ignored.
    pub fn edge(&mut self, button: Button, pressed: bool, now_ms: u64) {
        let immediate = button == Button::B && self.immediate_b;
        let t = self.track(button);
        if pressed == t.pressed
            || now_ms.saturating_sub(t.last_edge) < DEBOUNCE_MS && t.last_edge != 0
        {
            return;
        }
        t.pressed = pressed;
        t.last_edge = now_ms;
        let state = t.state;
        let mut emit = None;
        t.state = match (pressed, state) {
            (true, State::WaitDouble { until, count }) if now_ms <= until => State::Down {
                at: now_ms,
                count,
                ticks: 0,
            },
            (true, _) => State::Down {
                at: now_ms,
                count: 0,
                ticks: 0,
            },
            (false, State::Down { at, count, ticks }) => {
                let held = now_ms.saturating_sub(at);
                if held < LONG_MS {
                    if button == Button::A && count >= 1 {
                        // A's double click at once: it is "previous" everywhere.
                        emit = Some(Gesture::Double(button));
                        State::Idle
                    } else if immediate {
                        emit = Some(Gesture::Click(button));
                        State::Idle
                    } else {
                        State::WaitDouble {
                            until: now_ms + DOUBLE_MS,
                            count: count.saturating_add(1),
                        }
                    }
                } else {
                    // A ticks once held past 1 s: its release then only ends the hold.
                    emit = Some(if button == Button::B || ticks == 0 {
                        Gesture::Long(button)
                    } else {
                        Gesture::HeldEnd(button)
                    });
                    State::Idle
                }
            }
            (false, s) => s,
        };
        if let Some(g) = emit {
            self.emit(g);
        }
    }

    /// Whether `button` counts as pressed (its last edge kept). A level that differs means an
    /// edge was taken for a bounce: the runner samples again after the debounce time.
    pub fn down(&self, button: Button) -> bool {
        match button {
            Button::A => self.a.pressed,
            Button::B => self.b.pressed,
        }
    }

    /// Timed gestures (click after the double window, held ticks). Call often.
    pub fn poll(&mut self, now_ms: u64) -> Option<Gesture> {
        for button in [Button::A, Button::B] {
            let t = self.track(button);
            match t.state {
                State::WaitDouble { until, count } if now_ms > until => {
                    t.state = State::Idle;
                    self.emit(match count {
                        0 | 1 => Gesture::Click(button),
                        2 => Gesture::Double(button),
                        3 => Gesture::Triple(button),
                        _ => Gesture::Quad(button),
                    });
                }
                State::Down { at, count, ticks } => {
                    let held = now_ms.saturating_sub(at);
                    if held >= HOLD_MS {
                        let due = ((held - HOLD_MS) / HOLD_TICK_MS) as u32 + 1;
                        if due > ticks {
                            t.state = State::Down {
                                at,
                                count,
                                ticks: due,
                            };
                            self.emit(Gesture::Held(button, held as u32));
                        }
                    }
                }
                _ => {}
            }
        }
        self.out.pop_front()
    }

    /// When the next timed gesture may fire, to sleep until then.
    pub fn next_deadline(&self) -> Option<u64> {
        let d = |t: &Track| match t.state {
            State::WaitDouble { until, .. } => Some(until + 1),
            State::Down { at, ticks, .. } => Some(at + HOLD_MS + u64::from(ticks) * HOLD_TICK_MS),
            State::Idle => None,
        };
        match (d(&self.a), d(&self.b)) {
            (Some(x), Some(y)) => Some(x.min(y)),
            (x, y) => x.or(y),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;
    use std::vec::Vec;

    fn run(events: &[(Button, bool, u64)], until: u64) -> Vec<Gesture> {
        let mut g = Gestures::new();
        let mut out = Vec::new();
        let mut i = 0;
        for now in 0..=until {
            while i < events.len() && events[i].2 == now {
                g.edge(events[i].0, events[i].1, now);
                i += 1;
            }
            while let Some(x) = g.poll(now) {
                out.push(x);
            }
        }
        out
    }

    use Button::*;
    use Gesture::*;

    #[test]
    fn click_after_double_window() {
        assert_eq!(run(&[(A, true, 100), (A, false, 200)], 600), vec![Click(A)]);
    }

    #[test]
    fn double_click() {
        assert_eq!(
            run(
                &[
                    (A, true, 100),
                    (A, false, 180),
                    (A, true, 300),
                    (A, false, 380)
                ],
                900
            ),
            vec![Double(A)]
        );
    }

    /// B counts its clicks: one gesture once the presses stop.
    #[test]
    fn b_counts_up_to_four_clicks() {
        let presses = |n: u64| -> Vec<(Button, bool, u64)> {
            (0..n)
                .flat_map(|i| [(B, true, 100 + i * 200), (B, false, 160 + i * 200)])
                .collect()
        };
        assert_eq!(run(&presses(1), 1500), vec![Click(B)]);
        assert_eq!(run(&presses(2), 1500), vec![Double(B)]);
        assert_eq!(run(&presses(3), 1500), vec![Triple(B)]);
        assert_eq!(run(&presses(4), 1800), vec![Quad(B)]);
        assert_eq!(run(&presses(6), 2200), vec![Quad(B)]);
        // B's double click comes once the window after the second press has passed.
        let mut g = Gestures::new();
        for (b, p, t) in presses(2) {
            g.edge(b, p, t);
        }
        assert_eq!(g.poll(400), None);
        assert_eq!(g.next_deadline(), Some(661));
        assert_eq!(g.poll(661), Some(Double(B)));
    }

    /// Presses further apart than the window stay separate clicks; A's double click is
    /// reported at once and A never counts further.
    #[test]
    fn slow_presses_and_a_stay_as_before() {
        assert_eq!(
            run(
                &[
                    (B, true, 100),
                    (B, false, 160),
                    (B, true, 600),
                    (B, false, 660)
                ],
                1200
            ),
            vec![Click(B), Click(B)]
        );
        let mut g = Gestures::new();
        for (b, p, t) in [
            (A, true, 100),
            (A, false, 160),
            (A, true, 300),
            (A, false, 360),
        ] {
            g.edge(b, p, t);
        }
        assert_eq!(g.poll(361), Some(Double(A)));
        assert_eq!(
            run(
                &[
                    (A, true, 100),
                    (A, false, 160),
                    (A, true, 300),
                    (A, false, 360),
                    (A, true, 500),
                    (A, false, 560)
                ],
                1200
            ),
            vec![Double(A), Click(A)]
        );
    }

    /// While typing, each short press of B is a click at once, however fast they come; a long
    /// press still is one, and A is untouched.
    #[test]
    fn immediate_b_clicks_at_each_release() {
        let mut g = Gestures::new();
        g.set_immediate_b(true);
        let mut out = Vec::new();
        for i in 0..5u64 {
            g.edge(B, true, 100 + i * 120);
            g.edge(B, false, 160 + i * 120);
            // Reported at the release itself, with nothing left to wait for.
            assert_eq!(g.poll(160 + i * 120), Some(Click(B)));
            assert_eq!(g.next_deadline(), None);
            out.push(Click(B));
        }
        assert_eq!(out.len(), 5);
        g.edge(B, true, 2000);
        g.edge(B, false, 2700);
        assert_eq!(g.poll(2700), Some(Long(B)));
        g.edge(A, true, 3000);
        g.edge(A, false, 3060);
        assert_eq!(g.poll(3061), None, "A still waits for a double click");
        assert_eq!(g.poll(3361), Some(Click(A)));
        // Back to counting once typing ends.
        g.set_immediate_b(false);
        g.edge(B, true, 4000);
        g.edge(B, false, 4060);
        g.edge(B, true, 4200);
        g.edge(B, false, 4260);
        assert_eq!(g.poll(4261), None);
        assert_eq!(g.poll(4561), Some(Double(B)));
    }

    #[test]
    fn long_press_on_release() {
        assert_eq!(run(&[(B, true, 100), (B, false, 800)], 1200), vec![Long(B)]);
        assert_eq!(run(&[(A, true, 100), (A, false, 900)], 1200), vec![Long(A)]);
    }

    #[test]
    fn a_held_repeats_and_does_not_open() {
        let g = run(&[(A, true, 100), (A, false, 1700)], 2000);
        assert_eq!(
            g,
            vec![Held(A, 1000), Held(A, 1250), Held(A, 1500), HeldEnd(A)]
        );
    }

    #[test]
    fn b_held_still_toggles_light() {
        let g = run(&[(B, true, 100), (B, false, 1600)], 2000);
        assert_eq!(g.last(), Some(&Long(B)));
    }

    #[test]
    fn bounces_are_ignored() {
        let g = run(
            &[
                (A, true, 100),
                (A, false, 110),
                (A, true, 115),
                (A, false, 220),
            ],
            700,
        );
        assert_eq!(g, vec![Click(A)]);
    }

    #[test]
    fn buttons_are_independent() {
        let g = run(
            &[
                (A, true, 100),
                (B, true, 120),
                (A, false, 200),
                (B, false, 230),
            ],
            800,
        );
        assert_eq!(g, vec![Click(A), Click(B)]);
    }
}
