//! A shell on a pseudo-terminal: the one program the terminal runs.
//!
//! Inside the sandbox the host starts it, through `process` with a PTY: the
//! plugin names the program (one `plugin.json` lists, `$SHELL` for the user's
//! login shell), and reads and writes never block. Natively, for the unit
//! tests, it is portable-pty directly. Both backends have this one API.

use std::path::PathBuf;

/// Configuration for spawning a [`Shell`].
#[derive(Debug, Clone)]
pub struct ShellConfig {
    /// In the sandbox, a name `plugin.json` lists (`$SHELL`, `bash`, ...).
    /// Natively, a program on `PATH` or a path.
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    /// Extra or overriding variables, on top of the user's environment (which
    /// the program always inherits), so callers do not repopulate `PATH`.
    pub env: Vec<(String, String)>,
    pub rows: u16,
    pub cols: u16,
}

impl Default for ShellConfig {
    fn default() -> Self {
        ShellConfig {
            program: default_program(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            rows: 24,
            cols: 80,
        }
    }
}

/// The arguments and environment every shell gets, whichever backend runs it.
fn command_line(cfg: &ShellConfig) -> (Vec<String>, Vec<(String, String)>) {
    // Interactive mode for known shells. Without the flag bash and zsh skip
    // PS1 and disable the `complete` builtin, so a user's rc file typically
    // prints "complete: command not found" and no prompt shows. PowerShell
    // takes `-NoLogo`; cmd.exe has no analog. Caller-supplied args win.
    let args = if cfg.args.is_empty() {
        interactive_flag(&cfg.program)
            .map(|f| vec![f.to_owned()])
            .unwrap_or_default()
    } else {
        cfg.args.clone()
    };
    let mut env = Vec::new();
    if !cfg.env.iter().any(|(k, _)| k == "TERM") {
        env.push(("TERM".to_owned(), "xterm-256color".to_owned()));
    }
    env.extend(cfg.env.iter().cloned());
    (args, env)
}

pub use backend::Shell;

// ---------------------------------------------------------------------------
// In the sandbox: the host's PTY
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
mod backend {
    use super::{ShellConfig, command_line};
    use sicompass_pdk::process::{Child, PtySize};

    /// A shell the host runs on a PTY. Dropping it drops the host resource,
    /// which kills the program.
    pub struct Shell {
        child: Child,
        /// The end of the last chunk, in case `ESC[6n` straddles two.
        tail: Vec<u8>,
    }

    fn io(e: String) -> std::io::Error {
        std::io::Error::other(e)
    }

    impl Shell {
        pub fn spawn(cfg: ShellConfig) -> std::io::Result<Self> {
            let (args, env) = command_line(&cfg);
            let cwd = cfg.cwd.as_ref().map(|p| p.to_string_lossy().into_owned());
            let size = PtySize {
                rows: cfg.rows,
                cols: cfg.cols,
            };
            let child = Child::spawn(&cfg.program, &args, cwd.as_deref(), &env, &[], Some(size))
                .map_err(io)?;
            Ok(Shell {
                child,
                tail: Vec::new(),
            })
        }

        pub fn write_input(&mut self, bytes: &[u8]) -> std::io::Result<()> {
            self.child.write(bytes).map_err(io)
        }

        pub fn write_line(&mut self, s: &str) -> std::io::Result<()> {
            self.write_input(s.as_bytes())?;
            self.write_input(b"\n")
        }

        /// Whatever the shell wrote since the last call. Never blocks.
        pub fn drain_output(&mut self) -> Vec<u8> {
            let mut out = Vec::new();
            loop {
                let chunk = self.child.read(1 << 20);
                if chunk.is_empty() {
                    break;
                }
                out.extend(chunk);
            }
            if !out.is_empty() {
                let hits = super::count_dsr(&mut self.tail, &out);
                for _ in 0..hits {
                    let _ = self.child.write(super::DSR_REPLY);
                }
            }
            out
        }

        pub fn foreground_busy(&self) -> bool {
            self.child.foreground_busy()
        }

        pub fn resize(&mut self, rows: u16, cols: u16) -> std::io::Result<()> {
            self.child.resize(PtySize { rows, cols });
            Ok(())
        }

        /// Where the shell is now (`cd` moves it), where the host can say.
        pub fn cwd(&self) -> Option<String> {
            self.child.cwd()
        }
    }
}

// ---------------------------------------------------------------------------
// Natively, for the tests: portable-pty
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
mod backend {
    use super::{ShellConfig, command_line};
    use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
    use std::io::{Read, Write};
    use std::sync::{Arc, Mutex};

    /// A spawned, PTY-backed shell process.
    ///
    /// `drain_output()` is non-blocking and returns whatever bytes the
    /// background reader thread has buffered since the previous call.
    pub struct Shell {
        master: Box<dyn MasterPty + Send>,
        /// Shared with the reader thread, which answers DSR queries itself.
        writer: Arc<Mutex<Box<dyn Write + Send>>>,
        child: Box<dyn portable_pty::Child + Send + Sync>,
        output: Arc<Mutex<Vec<u8>>>,
    }

    fn io_err<E: std::fmt::Display>(e: E) -> std::io::Error {
        std::io::Error::other(e.to_string())
    }

    impl Shell {
        pub fn spawn(cfg: ShellConfig) -> std::io::Result<Self> {
            let pair = native_pty_system()
                .openpty(PtySize {
                    rows: cfg.rows,
                    cols: cfg.cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(io_err)?;
            let (args, env) = command_line(&cfg);
            let program = if cfg.program == "$SHELL" {
                super::default_program()
            } else {
                cfg.program.clone()
            };
            let mut cmd = CommandBuilder::new(&program);
            cmd.args(&args);
            if let Some(cwd) = &cfg.cwd {
                cmd.cwd(cwd);
            }
            for (k, v) in std::env::vars() {
                cmd.env(k, v);
            }
            for (k, v) in &env {
                cmd.env(k, v);
            }

            let child = pair.slave.spawn_command(cmd).map_err(io_err)?;
            drop(pair.slave);

            let mut reader = pair.master.try_clone_reader().map_err(io_err)?;
            let writer: Arc<Mutex<Box<dyn Write + Send>>> =
                Arc::new(Mutex::new(pair.master.take_writer().map_err(io_err)?));
            let output = Arc::new(Mutex::new(Vec::<u8>::new()));
            {
                let output = Arc::clone(&output);
                let writer = Arc::clone(&writer);
                std::thread::spawn(move || {
                    let mut buf = [0u8; 4096];
                    let mut tail: Vec<u8> = Vec::with_capacity(4);
                    loop {
                        match reader.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                let chunk = &buf[..n];
                                if let Ok(mut o) = output.lock() {
                                    o.extend_from_slice(chunk);
                                }
                                let hits = super::count_dsr(&mut tail, chunk);
                                if hits > 0
                                    && let Ok(mut w) = writer.lock()
                                {
                                    for _ in 0..hits {
                                        let _ = w.write_all(super::DSR_REPLY);
                                    }
                                    let _ = w.flush();
                                }
                            }
                        }
                    }
                });
            }

            Ok(Shell {
                master: pair.master,
                writer,
                child,
                output,
            })
        }

        pub fn write_input(&mut self, bytes: &[u8]) -> std::io::Result<()> {
            let mut w = self.writer.lock().expect("shell writer mutex poisoned");
            w.write_all(bytes)?;
            w.flush()
        }

        /// Send `s` followed by an Enter keystroke (`\r\n` on Windows, `\n` on
        /// Unix). On Windows, cmd.exe under ConPTY treats a bare `\n` as a
        /// continuation character, not a line submission.
        pub fn write_line(&mut self, s: &str) -> std::io::Result<()> {
            self.write_input(s.as_bytes())?;
            if cfg!(windows) {
                self.write_input(b"\r\n")
            } else {
                self.write_input(b"\n")
            }
        }

        pub fn drain_output(&mut self) -> Vec<u8> {
            let mut guard = self.output.lock().expect("shell output mutex poisoned");
            std::mem::take(&mut *guard)
        }

        #[cfg(test)]
        pub fn is_alive(&mut self) -> bool {
            matches!(self.child.try_wait(), Ok(None))
        }

        pub fn pid(&self) -> Option<u32> {
            self.child.process_id()
        }

        #[cfg(test)]
        pub fn kill(&mut self) -> std::io::Result<()> {
            self.child.kill()
        }

        /// Whether a foreground command (something other than the shell
        /// itself) is running on the PTY. Linux only, from `/proc/<pid>/stat`:
        /// the shell's process group against its terminal's foreground group.
        pub fn foreground_busy(&self) -> bool {
            #[cfg(target_os = "linux")]
            {
                let Some(pid) = self.pid() else {
                    return false;
                };
                let Ok(content) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
                    return false;
                };
                let Some(rparen) = content.rfind(')') else {
                    return false;
                };
                let fields: Vec<&str> = content[rparen + 1..].split_whitespace().collect();
                let pgrp = fields.get(2).and_then(|s| s.parse::<i32>().ok());
                let tpgid = fields.get(5).and_then(|s| s.parse::<i32>().ok());
                matches!((pgrp, tpgid), (Some(pg), Some(tp)) if tp >= 0 && tp != pg)
            }
            #[cfg(not(target_os = "linux"))]
            {
                false
            }
        }

        pub fn resize(&mut self, rows: u16, cols: u16) -> std::io::Result<()> {
            self.master
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(io_err)
        }

        /// Where the shell is now. Linux: `/proc/<pid>/cwd`.
        pub fn cwd(&self) -> Option<String> {
            #[cfg(target_os = "linux")]
            {
                let pid = self.pid()?;
                std::fs::read_link(format!("/proc/{pid}/cwd"))
                    .ok()
                    .map(|p| p.to_string_lossy().into_owned())
            }
            #[cfg(not(target_os = "linux"))]
            {
                None
            }
        }
    }

    impl Drop for Shell {
        fn drop(&mut self) {
            let _ = self.child.kill();
        }
    }
}

/// A device status report query: where is the cursor?
const DSR_QUERY: &[u8] = b"\x1b[6n";

/// The answer to one. Real emulators report the actual cursor position, but
/// cmd.exe under ConPTY blocks its prompt until it gets *some* valid reply.
const DSR_REPLY: &[u8] = b"\x1b[1;1R";

/// How many DSR queries `tail` + `chunk` hold, keeping the end of the chunk in
/// `tail` in case the next query straddles two reads.
fn count_dsr(tail: &mut Vec<u8>, chunk: &[u8]) -> usize {
    let mut scan = std::mem::take(tail);
    scan.extend_from_slice(chunk);
    let mut hits = 0;
    let mut i = 0;
    while i + DSR_QUERY.len() <= scan.len() {
        if &scan[i..i + DSR_QUERY.len()] == DSR_QUERY {
            hits += 1;
            i += DSR_QUERY.len();
        } else {
            i += 1;
        }
    }
    let keep = scan.len().min(DSR_QUERY.len() - 1);
    // A query already counted must not be counted again with the next chunk.
    let from = (scan.len() - keep).max(i.min(scan.len()));
    tail.extend_from_slice(&scan[from..]);
    hits
}

/// The shell to run when the user has not picked one.
///
/// In the sandbox: `$SHELL`, which the host resolves to the user's login
/// shell. Natively: `$SHELL` from the environment, then `/bin/sh` (and
/// `%ComSpec%`, then `cmd.exe`, on Windows).
pub fn default_program() -> String {
    if cfg!(target_arch = "wasm32") {
        "$SHELL".to_owned()
    } else if cfg!(windows) {
        std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_owned())
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned())
    }
}

/// For known interactive shells, the flag that makes PS1 and programmable
/// completion work under a PTY. `None` for a shell with no such flag (cmd.exe)
/// or one this does not recognise.
///
/// `$SHELL` is the user's login shell, which on the systems the sandbox runs
/// shells on is almost always one of the Unix shells here, all of which take
/// `-i`.
fn interactive_flag(program: &str) -> Option<&'static str> {
    if program == "$SHELL" {
        return Some("-i");
    }
    let basename = std::path::Path::new(program)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(program)
        .to_ascii_lowercase();
    let basename = basename.strip_suffix(".exe").unwrap_or(&basename);
    match basename {
        "bash" | "zsh" | "sh" | "dash" | "ksh" | "fish" => Some("-i"),
        "pwsh" | "powershell" => Some("-NoLogo"),
        _ => None,
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[cfg(unix)]
    #[test]
    fn spawn_sh_echo_observes_output() {
        let cfg = ShellConfig {
            program: "/bin/sh".to_owned(),
            ..Default::default()
        };
        let mut shell = Shell::spawn(cfg).expect("spawn /bin/sh");
        shell
            .write_line("echo sicompass-shell-test")
            .expect("write");

        let mut acc: Vec<u8> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            acc.extend(shell.drain_output());
            if String::from_utf8_lossy(&acc).contains("sicompass-shell-test") {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!(
            "did not observe echoed marker; got: {:?}",
            String::from_utf8_lossy(&acc)
        );
    }

    #[cfg(unix)]
    #[test]
    fn kill_terminates_child() {
        let cfg = ShellConfig {
            program: "/bin/sh".to_owned(),
            ..Default::default()
        };
        let mut shell = Shell::spawn(cfg).expect("spawn /bin/sh");
        assert!(shell.is_alive(), "child should be alive after spawn");
        shell.kill().expect("kill");

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if !shell.is_alive() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("child still alive after kill");
    }

    #[test]
    fn default_program_returns_non_empty() {
        assert!(!default_program().is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn foreground_busy_false_at_prompt_true_while_running() {
        // A freshly spawned interactive shell sits at its prompt, so the
        // foreground process group is the shell itself → not busy.
        let cfg = ShellConfig {
            program: "/bin/sh".to_owned(),
            ..Default::default()
        };
        let mut shell = Shell::spawn(cfg).expect("spawn /bin/sh");
        // Give the shell a moment to reach its prompt.
        let settle = Instant::now() + Duration::from_millis(300);
        while Instant::now() < settle {
            let _ = shell.drain_output();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!shell.foreground_busy(), "idle shell at prompt is not busy");

        // Launch a foreground command that blocks; the shell hands the terminal
        // to it, so `foreground_busy` must report true while it runs.
        shell.write_line("sleep 5").expect("write");
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut saw_busy = false;
        while Instant::now() < deadline {
            let _ = shell.drain_output();
            if shell.foreground_busy() {
                saw_busy = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(saw_busy, "shell should report busy while `sleep` runs");
    }

    #[cfg(windows)]
    #[test]
    fn spawn_cmd_echo_observes_output() {
        // The submitted command must not contain the marker string itself,
        // or cmd's typing-echo alone would satisfy the assertion even when
        // Enter was never registered. We use `echo MARKER` so the marker
        // appears in output only if the command actually executed.
        let cfg = ShellConfig {
            program: "cmd.exe".to_owned(),
            ..Default::default()
        };
        let mut shell = Shell::spawn(cfg).expect("spawn cmd.exe");
        shell.write_line("echo SICOMPASS_OK").expect("write");

        let mut acc: Vec<u8> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            acc.extend(shell.drain_output());
            // The marker must appear at least twice: once as cmd's
            // typing-echo of the submitted line, and once as the actual
            // output of `echo`. A single occurrence means Enter was never
            // registered and the command never ran.
            let text = String::from_utf8_lossy(&acc);
            if text.matches("SICOMPASS_OK").count() >= 2 {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!(
            "did not observe executed marker; got: {:?}",
            String::from_utf8_lossy(&acc)
        );
    }

    #[cfg(windows)]
    #[test]
    fn kill_terminates_cmd_child() {
        let cfg = ShellConfig {
            program: "cmd.exe".to_owned(),
            ..Default::default()
        };
        let mut shell = Shell::spawn(cfg).expect("spawn cmd.exe");
        assert!(shell.is_alive());
        shell.kill().expect("kill");

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if !shell.is_alive() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("child still alive after kill");
    }

    #[test]
    fn interactive_flag_recognises_unix_shells() {
        assert_eq!(interactive_flag("/bin/bash"), Some("-i"));
        assert_eq!(interactive_flag("/usr/bin/zsh"), Some("-i"));
        assert_eq!(interactive_flag("fish"), Some("-i"));
    }

    #[test]
    fn interactive_flag_recognises_windows_shells() {
        assert_eq!(interactive_flag("powershell.exe"), Some("-NoLogo"));
        assert_eq!(interactive_flag("PowerShell.EXE"), Some("-NoLogo"));
        assert_eq!(interactive_flag("pwsh"), Some("-NoLogo"));
        // cmd.exe and unknown shells get no flag.
        assert_eq!(interactive_flag("cmd.exe"), None);
        assert_eq!(interactive_flag("CMD.exe"), None);
        assert_eq!(interactive_flag("nu"), None);
    }

    #[test]
    fn the_login_shell_is_interactive_too() {
        assert_eq!(interactive_flag("$SHELL"), Some("-i"));
    }

    #[test]
    fn a_dsr_query_is_answered_once_even_across_two_reads() {
        let mut tail = Vec::new();
        assert_eq!(count_dsr(&mut tail, b"abc\x1b[6ndef"), 1);
        assert_eq!(count_dsr(&mut tail, b"ghi"), 0, "not counted again");
        assert_eq!(count_dsr(&mut tail, b"x\x1b["), 0);
        assert_eq!(count_dsr(&mut tail, b"6ny"), 1, "straddling two reads");
        assert_eq!(count_dsr(&mut tail, b"\x1b[6n\x1b[6n"), 2);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cwd_follows_a_cd() {
        let tmp = tempfile::TempDir::new().unwrap();
        let there = tmp.path().canonicalize().unwrap();
        let cfg = ShellConfig {
            program: "/bin/sh".to_owned(),
            ..Default::default()
        };
        let mut shell = Shell::spawn(cfg).expect("spawn /bin/sh");
        assert!(shell.cwd().is_some());
        shell
            .write_line(&format!("cd '{}'", there.display()))
            .expect("write");
        let want = there.to_string_lossy().into_owned();
        let deadline = Instant::now() + Duration::from_secs(5);
        while shell.cwd().as_deref() != Some(want.as_str()) {
            let _ = shell.drain_output();
            assert!(Instant::now() < deadline, "cwd never followed the cd");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
