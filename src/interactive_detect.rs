//! Stream parser that flags entry into / exit from a "full-terminal"
//! interactive program by sniffing the DEC private-mode set/reset sequences
//! `CSI ? p1;p2;… {h|l}` in a PTY output stream.
//!
//! Used by `TerminalProvider` to auto-switch into the interactive dashboard
//! when the user runs a TUI from the scrollback view, and back out when the
//! program exits.
//!
//! Two families of program are covered:
//!
//! * **Alternate-screen** TUIs — `vim`, `less`, `htop`, `man` — emit
//!   `CSI ? {1049|47|1047} h` on entry and `… l` on exit.
//! * **Main-screen** TUIs — `claude`, `aider`, `gemini` — never touch the
//!   alternate screen (they keep the scrollback intact). They still announce
//!   themselves by enabling mouse-tracking (`1000/1002/1003/1006`) or
//!   focus-tracking (`1004`) reporting, which a bare shell never does.
//!   (`claude` specifically enables `?1004h`.)
//!
//! **Windows exception:** main-screen detection is disabled on Windows.
//! ConPTY enables focus/mouse-tracking modes itself as part of its handshake,
//! for *any* attached program — so honouring them would auto-enter the
//! interactive dashboard for a bare `cmd.exe` and trap the user there (once in
//! the interactive dashboard every key, including Esc and Ctrl+C, is forwarded
//! to the program). On Windows only the alternate-screen modes trigger.
//!
//! Bracketed paste (`2004`) is deliberately *not* a trigger: interactive
//! shells (bash/zsh ≥ 5.1) enable it at every prompt, so it carries no signal
//! about a child program. Cursor visibility (`25`) is excluded for the same
//! reason.
//!
//! **The shell itself is never a trigger,** whatever modes it sets. fish 4
//! probes the terminal at startup inside `CSI ?1049h … CSI ?1049l`, and any
//! shell or prompt plugin may switch focus or mouse reporting on for its own
//! line editor. Excluding modes one by one cannot keep up with every shell, so
//! the caller says whether a program the shell started holds the terminal
//! ([`InteractiveDetector::feed_while`]), and a mode *set* while it does not is
//! not recorded. Resets always apply.
//!
//! The detector tracks the *set* of interactive modes currently enabled and
//! emits `Enter` when that set becomes non-empty and `Leave` when it drains
//! back to empty — so a TUI that enables several modes at once (e.g. alt
//! screen + mouse) still yields a single Enter/Leave pair.
//!
//! Resumes across PTY chunk boundaries — partial sequences are remembered
//! between `feed` calls so the caller may pass arbitrary chunks. Bytes that
//! aren't part of a private-mode sequence are discarded; we only emit
//! transitions, never buffer.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractiveEvent {
    /// The set of enabled interactive modes went empty → non-empty: a
    /// full-terminal program started.
    Enter,
    /// The set went non-empty → empty: the program exited.
    Leave,
}

/// Alternate-screen DEC private modes — `vim`, `less`, `htop`, `man`.
/// Honoured on every platform.
const ALT_SCREEN_MODES: [u32; 3] = [47, 1047, 1049];

/// Main-screen-TUI DEC private modes — mouse tracking (`1000/1002/1003/1006`)
/// and focus tracking (`1004`).
///
/// Empty on Windows: ConPTY enables these for any attached program, so they
/// carry no signal there (see the module docs). Non-Windows only.
///
/// Bracketed paste (`2004`) and cursor visibility (`25`) are intentionally
/// absent everywhere — both are emitted by ordinary shell prompts and would
/// false-trigger.
#[cfg(not(windows))]
const MAIN_SCREEN_MODES: [u32; 5] = [1000, 1002, 1003, 1006, 1004];
#[cfg(windows)]
const MAIN_SCREEN_MODES: [u32; 0] = [];

/// Whether `mode` marks a "full-terminal" interactive program on this platform.
fn is_interactive_mode(mode: u32) -> bool {
    ALT_SCREEN_MODES.contains(&mode) || MAIN_SCREEN_MODES.contains(&mode)
}

#[derive(Debug)]
pub struct InteractiveDetector {
    state: State,
    /// Decimal parameters accumulated since `CSI ?`, split on `;`.
    params: Vec<u32>,
    /// The parameter currently being accumulated.
    cur: u32,
    /// Whether `cur` has seen any digit yet.
    has_digit: bool,
    /// Interactive modes currently enabled (a subset of `INTERACTIVE_MODES`).
    active: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Plain text or unrelated escape — looking for `ESC`.
    Ground,
    /// Saw `ESC` — looking for `[`.
    Esc,
    /// Saw `ESC [` — looking for `?` (or a final byte that ends a non-private
    /// CSI we don't care about).
    Csi,
    /// Saw `ESC [ ?` — accumulating `;`-separated digits, waiting for `h`/`l`.
    Param,
}

impl InteractiveDetector {
    pub fn new() -> Self {
        InteractiveDetector {
            state: State::Ground,
            params: Vec::new(),
            cur: 0,
            has_digit: false,
            active: Vec::new(),
        }
    }

    /// [`feed_while`](Self::feed_while) with a program assumed to be running:
    /// every interactive mode the bytes set counts.
    #[cfg(test)]
    pub fn feed(&mut self, bytes: &[u8], emit: impl FnMut(InteractiveEvent)) {
        self.feed_while(bytes, true, emit);
    }

    /// Feed bytes; invoke `emit` for every Enter/Leave transition seen.
    ///
    /// `program_running` is whether a program the shell started holds the
    /// terminal. When it is false the bytes are the shell's own, and a mode
    /// they set is ignored: it is the shell's prompt or line editor, not a
    /// full-terminal program. It must not enter the set either, or a later real
    /// program would find it non-empty and never yield an Enter.
    pub fn feed_while(
        &mut self,
        bytes: &[u8],
        program_running: bool,
        mut emit: impl FnMut(InteractiveEvent),
    ) {
        for &b in bytes {
            match self.state {
                State::Ground => {
                    if b == 0x1b {
                        self.state = State::Esc;
                    }
                }
                State::Esc => {
                    self.state = if b == b'[' { State::Csi } else { State::Ground };
                }
                State::Csi => {
                    if b == b'?' {
                        self.state = State::Param;
                        self.params.clear();
                        self.cur = 0;
                        self.has_digit = false;
                    } else if matches!(b, 0x40..=0x7E) {
                        // Some other CSI final byte; not a private mode.
                        self.state = State::Ground;
                    }
                    // else: stay in Csi (param / intermediate byte we ignore)
                }
                State::Param => match b {
                    b'0'..=b'9' => {
                        self.cur = self
                            .cur
                            .saturating_mul(10)
                            .saturating_add((b - b'0') as u32);
                        self.has_digit = true;
                    }
                    b';' => {
                        if self.has_digit {
                            self.params.push(self.cur);
                        }
                        self.cur = 0;
                        self.has_digit = false;
                    }
                    b'h' | b'l' => {
                        if self.has_digit {
                            self.params.push(self.cur);
                        }
                        let on = b == b'h';
                        let params = std::mem::take(&mut self.params);
                        // A set with no program running is the shell's own;
                        // see `feed_while`.
                        if !on || program_running {
                            for p in params {
                                self.apply_mode(p, on, &mut emit);
                            }
                        }
                        self.state = State::Ground;
                        self.cur = 0;
                        self.has_digit = false;
                    }
                    0x40..=0x7E => {
                        // Any other final byte — done with this sequence.
                        self.state = State::Ground;
                    }
                    _ => {}
                },
            }
        }
    }

    /// Apply a single `mode` set/reset, emitting a transition when the active
    /// set crosses the empty boundary.
    fn apply_mode(&mut self, mode: u32, on: bool, emit: &mut impl FnMut(InteractiveEvent)) {
        if !is_interactive_mode(mode) {
            return;
        }
        let was_empty = self.active.is_empty();
        if on {
            if !self.active.contains(&mode) {
                self.active.push(mode);
            }
        } else {
            self.active.retain(|&m| m != mode);
        }
        match (was_empty, self.active.is_empty()) {
            (true, false) => emit(InteractiveEvent::Enter),
            (false, true) => emit(InteractiveEvent::Leave),
            _ => {}
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn collect(input: &[u8]) -> Vec<InteractiveEvent> {
        let mut d = InteractiveDetector::new();
        let mut out = Vec::new();
        d.feed(input, |e| out.push(e));
        out
    }

    #[test]
    fn detects_alt_screen_enter_leave() {
        assert_eq!(collect(b"\x1b[?1049h"), vec![InteractiveEvent::Enter]);
        assert_eq!(
            collect(b"\x1b[?1049h\x1b[?1049l"),
            vec![InteractiveEvent::Enter, InteractiveEvent::Leave],
        );
        assert_eq!(collect(b"\x1b[?47h"), vec![InteractiveEvent::Enter]);
        assert_eq!(collect(b"\x1b[?1047h"), vec![InteractiveEvent::Enter]);
    }

    // Mouse/focus tracking is a trigger only off Windows — ConPTY emits those
    // modes itself, so they are excluded from detection on Windows.
    #[test]
    #[cfg(not(windows))]
    fn detects_mouse_tracking() {
        assert_eq!(collect(b"\x1b[?1000h"), vec![InteractiveEvent::Enter]);
        assert_eq!(
            collect(b"\x1b[?1002h\x1b[?1002l"),
            vec![InteractiveEvent::Enter, InteractiveEvent::Leave],
        );
        assert_eq!(collect(b"\x1b[?1006h"), vec![InteractiveEvent::Enter]);
    }

    #[test]
    #[cfg(not(windows))]
    fn detects_focus_tracking() {
        // This is the signal `claude` emits.
        assert_eq!(collect(b"\x1b[?1004h"), vec![InteractiveEvent::Enter]);
        assert_eq!(
            collect(b"\x1b[?1004h\x1b[?1004l"),
            vec![InteractiveEvent::Enter, InteractiveEvent::Leave],
        );
    }

    #[test]
    fn bracketed_paste_alone_does_not_trigger() {
        // Shells enable ?2004 at every prompt — must not be a trigger.
        assert_eq!(collect(b"\x1b[?2004h"), vec![]);
        assert_eq!(collect(b"\x1b[?2004h\x1b[?2004l"), vec![]);
    }

    #[test]
    fn cursor_visibility_does_not_trigger() {
        assert_eq!(collect(b"\x1b[?25h"), vec![]);
        assert_eq!(collect(b"\x1b[?25l"), vec![]);
    }

    #[test]
    #[cfg(not(windows))]
    fn claude_style_init_triggers_once() {
        // Sequence captured from a real `claude` startup: sync output, cursor
        // hide, bracketed paste, focus tracking. Only ?1004h is a trigger.
        assert_eq!(
            collect(b"\x1b[?2026h\x1b[?25l\x1b[?2004h\x1b[?1004h\x1b[?2026l"),
            vec![InteractiveEvent::Enter],
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn multi_param_yields_single_enter() {
        // `CSI ? 1000;1002;1006 h` — three modes, one empty→non-empty cross.
        assert_eq!(
            collect(b"\x1b[?1000;1002;1006h"),
            vec![InteractiveEvent::Enter]
        );
    }

    #[test]
    fn multi_param_alt_screen_with_extra() {
        // The old detector bailed on `;`; now `1049` is still honoured.
        assert_eq!(collect(b"\x1b[?1049;6h"), vec![InteractiveEvent::Enter]);
    }

    #[test]
    fn combined_modes_one_enter_one_leave() {
        // A TUI that enables alt screen + mouse, then disables both on exit.
        assert_eq!(
            collect(b"\x1b[?1049h\x1b[?1002h\x1b[?1002l\x1b[?1049l"),
            vec![InteractiveEvent::Enter, InteractiveEvent::Leave],
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn idempotent_enable_no_duplicate_enter() {
        assert_eq!(
            collect(b"\x1b[?1004h\x1b[?1004h"),
            vec![InteractiveEvent::Enter],
        );
    }

    /// On Windows the mouse/focus modes are excluded (ConPTY emits them for
    /// any program); only the alternate-screen modes survive as triggers.
    #[test]
    #[cfg(windows)]
    fn windows_ignores_mouse_and_focus_modes() {
        // Focus + mouse tracking carry no signal on Windows.
        assert_eq!(collect(b"\x1b[?1004h"), vec![]);
        assert_eq!(collect(b"\x1b[?1000h"), vec![]);
        assert_eq!(collect(b"\x1b[?1002h"), vec![]);
        assert_eq!(collect(b"\x1b[?1006h"), vec![]);
        assert_eq!(collect(b"\x1b[?1000;1002;1006h"), vec![]);
        // A claude-style init (only ?1004h is interactive) must NOT trigger.
        assert_eq!(
            collect(b"\x1b[?2026h\x1b[?25l\x1b[?2004h\x1b[?1004h\x1b[?2026l"),
            vec![],
        );
        // Alternate-screen modes still trigger normally.
        assert_eq!(collect(b"\x1b[?1049h"), vec![InteractiveEvent::Enter]);
    }

    #[test]
    fn ignores_plain_csi_and_text() {
        assert_eq!(collect(b"\x1b[31m"), vec![]);
        assert_eq!(collect(b"\x1b[2J"), vec![]);
        assert_eq!(collect(b"\x1b[H"), vec![]);
        assert_eq!(collect(b"hello world\n"), vec![]);
    }

    #[test]
    fn partial_chunks_resume() {
        let mut d = InteractiveDetector::new();
        let mut events = Vec::new();
        for chunk in [&b"\x1b["[..], &b"?10"[..], &b"49h"[..]] {
            d.feed(chunk, |e| events.push(e));
        }
        assert_eq!(events, vec![InteractiveEvent::Enter]);
    }

    #[test]
    fn garbage_recovers_for_next_sequence() {
        assert_eq!(
            collect(b"\x1b[?1049x\x1b[?1049h"),
            vec![InteractiveEvent::Enter],
        );
    }

    /// What fish 4.7 writes before its first prompt, captured from a PTY with
    /// `fish --no-config -i`: kitty keyboard, XTVERSION and background-colour
    /// queries, then XTGETTCAP inside an alternate-screen bracket, then DA1.
    pub(crate) const FISH_STARTUP: &[u8] = b"\x1b[?u\x1b[>0q\x1b]11;?\x1b\\\
        \x1b[?1049h\x1bP+q696e646e\x1b\\\x1bP+q71756572792d6f732d6e616d65\x1b\\\
        \x1b[?1049l\x1b[0c";

    fn collect_while(program_running: bool, input: &[u8]) -> Vec<InteractiveEvent> {
        let mut d = InteractiveDetector::new();
        let mut out = Vec::new();
        d.feed_while(input, program_running, |e| out.push(e));
        out
    }

    #[test]
    fn a_mode_the_shell_sets_for_itself_is_not_a_program() {
        assert_eq!(collect_while(false, b"\x1b[?1049h"), vec![]);
        assert_eq!(collect_while(false, b"\x1b[?1000;1006h"), vec![]);
        assert_eq!(collect_while(false, FISH_STARTUP), vec![]);
    }

    #[test]
    fn fish_startup_counts_when_a_program_is_said_to_be_running() {
        // The bytes alone cannot tell fish from vim; only the caller's
        // `program_running` does.
        assert_eq!(
            collect_while(true, FISH_STARTUP),
            vec![InteractiveEvent::Enter, InteractiveEvent::Leave],
        );
    }

    #[test]
    fn a_reset_applies_whoever_is_in_the_foreground() {
        // A program set the alt screen, and the shell was back in the
        // foreground by the time its exit was read.
        let mut d = InteractiveDetector::new();
        let mut events = Vec::new();
        d.feed_while(b"\x1b[?1049h", true, |e| events.push(e));
        d.feed_while(b"\x1b[?1049l", false, |e| events.push(e));
        assert_eq!(
            events,
            vec![InteractiveEvent::Enter, InteractiveEvent::Leave]
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn a_mode_left_on_by_the_shell_does_not_hide_a_later_program() {
        // A prompt that turns focus reporting on and leaves it on, then vim.
        let mut d = InteractiveDetector::new();
        let mut events = Vec::new();
        d.feed_while(b"\x1b[?1004h", false, |e| events.push(e));
        d.feed_while(b"\x1b[?1049h", true, |e| events.push(e));
        assert_eq!(events, vec![InteractiveEvent::Enter]);
    }
}
