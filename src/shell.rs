//! A shell on a pseudo-terminal: the one program the terminal runs.
//!
//! portable-pty (a Unix PTY, or ConPTY on Windows), with a reader thread, so
//! nothing the app calls ever waits on the shell. The program is a name
//! (`$SHELL` for the user's login shell, see [`resolve_program`]) or a path.

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Configuration for spawning a [`Shell`].
#[derive(Debug, Clone)]
pub struct ShellConfig {
    /// [`LOGIN_SHELL`], a program name (`bash`, found on `PATH` or in
    /// `~/.local/bin`), or a path.
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

/// The arguments and environment every shell gets.
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

// ---------------------------------------------------------------------------
// The shell
// ---------------------------------------------------------------------------

/// A spawned, PTY-backed shell process.
///
/// `drain_output()` is non-blocking and returns whatever bytes the background
/// reader thread has buffered since the previous call.
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
        // Resolved first, so the interactive flag is chosen for the shell that
        // actually runs (the login shell need not take `-i`).
        let cfg = ShellConfig {
            program: resolve_program(&cfg.program),
            ..cfg
        };
        let (args, env) = command_line(&cfg);
        let mut cmd = CommandBuilder::new(&cfg.program);
        cmd.args(&args);
        if let Some(cwd) = &cfg.cwd {
            cmd.cwd(cwd);
        }
        for (k, v) in std::env::vars_os() {
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
            std::thread::Builder::new()
                .name("terminal-pty".into())
                .spawn(move || {
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
                                let hits = count_dsr(&mut tail, chunk);
                                if hits > 0
                                    && let Ok(mut w) = writer.lock()
                                {
                                    for _ in 0..hits {
                                        let _ = w.write_all(DSR_REPLY);
                                    }
                                    let _ = w.flush();
                                }
                            }
                        }
                    }
                })?;
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

    /// The shell's process id, which the app's tab switcher names the tab
    /// after.
    pub fn pid(&self) -> Option<u32> {
        self.child.process_id()
    }

    #[cfg(test)]
    pub fn kill(&mut self) -> std::io::Result<()> {
        self.child.kill()
    }

    /// Whether a foreground command (something other than the shell itself)
    /// is running on the PTY: the terminal's foreground process group
    /// (`tcgetpgrp` on the master) is not the shell's own. The shell leads its
    /// own session and group, and job control hands the terminal to each
    /// command it runs. Never busy on Windows, where ConPTY has no such thing.
    pub fn foreground_busy(&self) -> bool {
        #[cfg(unix)]
        {
            let (Some(pid), Some(leader)) = (self.pid(), self.master.process_group_leader()) else {
                return false;
            };
            i64::from(leader) != i64::from(pid)
        }
        #[cfg(not(unix))]
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

    /// Where the shell is now (`cd` moves it): `/proc/<pid>/cwd` on Linux,
    /// `proc_pidinfo` on macOS. `None` on Windows, where the prompt follows
    /// the `cd`s it parses instead.
    pub fn cwd(&self) -> Option<String> {
        process_cwd(self.pid()?)
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        // SIGHUP, then SIGKILL if the shell ignores it; reaped, so no zombie
        // stays behind for as long as the plugin runs.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(target_os = "linux")]
fn process_cwd(pid: u32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/cwd"))
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

#[cfg(target_os = "macos")]
fn process_cwd(pid: u32) -> Option<String> {
    let pid = libc::c_int::try_from(pid).ok()?;
    // SAFETY: an all-zero `proc_vnodepathinfo` is a valid value (plain
    // integers and byte arrays), and `proc_pidinfo` writes at most `size`
    // bytes into it.
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_vnodepathinfo>()).ok()?;
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            (&raw mut info).cast(),
            size,
        )
    };
    if n != size {
        return None;
    }
    // `vip_path` is a NUL-terminated MAXPATHLEN buffer, declared as 32 rows
    // of 32 for the sake of old compilers.
    let bytes: Vec<u8> = info
        .pvi_cdir
        .vip_path
        .iter()
        .flatten()
        .map(|c| *c as u8)
        .take_while(|b| *b != 0)
        .collect();
    (!bytes.is_empty()).then(|| String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn process_cwd(_pid: u32) -> Option<String> {
    None
}

// ---------------------------------------------------------------------------
// Which program
// ---------------------------------------------------------------------------

/// The `shellProgram` value that means the user's login shell.
pub const LOGIN_SHELL: &str = "$SHELL";

/// The program to start for `program`: the login shell for [`LOGIN_SHELL`], a
/// path as it is, and a name as it is when it is on `PATH` (the PTY finds it
/// there), else from `~/.local/bin` when it is there. A desktop session's
/// `PATH` often lacks `~/.local/bin`, where per-user installers put programs.
pub fn resolve_program(program: &str) -> String {
    let program = program.trim();
    if program.is_empty() || program == LOGIN_SHELL {
        return login_shell();
    }
    if program.contains(['/', '\\']) {
        return program.to_owned();
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    if std::env::split_paths(&path).any(|dir| is_program(&dir, program)) {
        return program.to_owned();
    }
    crate::sys::home_dir()
        .map(|h| h.join(".local").join("bin"))
        .filter(|dir| is_program(dir, program))
        .map(|dir| dir.join(program).to_string_lossy().into_owned())
        .unwrap_or_else(|| program.to_owned())
}

/// Whether `dir` holds an executable named `program` (on Windows, with any
/// `PATHEXT` extension too).
fn is_program(dir: &std::path::Path, program: &str) -> bool {
    #[cfg(unix)]
    {
        is_executable(&dir.join(program))
    }
    #[cfg(not(unix))]
    {
        let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned());
        std::iter::once(String::new())
            .chain(exts.split(';').filter(|e| !e.is_empty()).map(str::to_owned))
            .any(|ext| dir.join(format!("{program}{ext}")).is_file())
    }
}

#[cfg(unix)]
fn is_executable(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The user's login shell: `$SHELL`, which the login session sets from the
/// account (and a user who starts sicompass from a shell of their choice
/// means), then the account's own entry, then `/bin/sh`. The same order as
/// most terminal emulators.
#[cfg(unix)]
pub fn login_shell() -> String {
    let runs = |s: &String| is_executable(std::path::Path::new(s.trim()));
    std::env::var("SHELL")
        .ok()
        .filter(runs)
        .or_else(|| passwd_shell().filter(runs))
        .unwrap_or_else(|| "/bin/sh".to_owned())
}

/// The user's shell on Windows: `%ComSpec%` (cmd.exe), else PowerShell.
#[cfg(not(unix))]
pub fn login_shell() -> String {
    std::env::var("ComSpec")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "powershell.exe".to_owned())
}

/// The shell in this user's account entry, from the system's user database.
#[cfg(unix)]
fn passwd_shell() -> Option<String> {
    let mut len = 1024usize;
    loop {
        let mut buf = vec![0 as libc::c_char; len];
        // SAFETY: an all-zero `passwd` is valid (null pointers and integers);
        // `getpwuid_r` fills it with pointers into `buf`, which outlives every
        // read of them below.
        let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let rc = unsafe {
            libc::getpwuid_r(
                libc::getuid(),
                &mut pwd,
                buf.as_mut_ptr(),
                buf.len(),
                &mut result,
            )
        };
        if rc == libc::ERANGE && len < 1 << 16 {
            len *= 4;
            continue;
        }
        if rc != 0 || result.is_null() || pwd.pw_shell.is_null() {
            return None;
        }
        // SAFETY: a NUL-terminated string inside `buf`.
        let shell = unsafe { std::ffi::CStr::from_ptr(pwd.pw_shell) };
        let shell = shell.to_str().ok()?.trim();
        return (!shell.is_empty()).then(|| shell.to_owned());
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

/// The shell to run when the user has not picked one: [`LOGIN_SHELL`],
/// which [`resolve_program`] turns into the user's login shell when it starts.
pub fn default_program() -> String {
    LOGIN_SHELL.to_owned()
}

/// For known interactive shells, the flag that makes PS1 and programmable
/// completion work under a PTY. `None` for a shell with no such flag (cmd.exe)
/// or one this does not recognise.
///
/// [`Shell::spawn`] asks with the resolved program. [`LOGIN_SHELL`] itself
/// is taken to be one of the Unix shells here, all of which take `-i`.
fn interactive_flag(program: &str) -> Option<&'static str> {
    if program == LOGIN_SHELL {
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

#[cfg(test)]
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
    fn the_login_shell_and_empty_resolve_to_the_users_shell() {
        let login = login_shell();
        assert!(!login.trim().is_empty());
        assert_eq!(resolve_program(LOGIN_SHELL), login);
        assert_eq!(resolve_program(""), login);
    }

    #[cfg(unix)]
    #[test]
    fn a_path_is_kept_and_a_name_on_path_stays_a_name() {
        assert_eq!(resolve_program("/bin/dash"), "/bin/dash");
        assert_eq!(resolve_program("sh"), "sh", "sh is on PATH");
        assert_eq!(
            resolve_program("no-such-shell-xyz"),
            "no-such-shell-xyz",
            "nowhere: the name, and starting it says why"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_account_shell_is_a_program() {
        // Where the user database has an entry, it names something that runs.
        if let Some(shell) = passwd_shell() {
            assert!(shell.starts_with('/'), "{shell}");
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_shell_knows_its_starting_directory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let there = tmp.path().canonicalize().unwrap();
        let cfg = ShellConfig {
            program: "/bin/sh".to_owned(),
            cwd: Some(there.clone()),
            ..Default::default()
        };
        let shell = Shell::spawn(cfg).expect("spawn /bin/sh");
        assert_eq!(shell.cwd(), Some(there.to_string_lossy().into_owned()));
        assert!(shell.pid().is_some());
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
