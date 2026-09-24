//! Who and where the user is, for the synthesized prompt: their user name,
//! the machine's short name, and their home folder.
//!
//! Natively (the unit tests) these come from the environment. In the sandbox
//! the plugin's own environment is empty, so they come once from `sh`, which
//! the host starts with the user's environment like any program it runs.

use std::path::PathBuf;
use std::sync::OnceLock;

struct Identity {
    user: String,
    host: String,
    home: Option<PathBuf>,
}

fn identity() -> &'static Identity {
    static ID: OnceLock<Identity> = OnceLock::new();
    ID.get_or_init(probe)
}

pub fn user() -> String {
    identity().user.clone()
}

/// The machine's name up to its first `.`.
pub fn host() -> String {
    identity().host.clone()
}

pub fn home_dir() -> Option<PathBuf> {
    identity().home.clone()
}

/// Whether the shell runs on Windows, which changes the prompt and quoting.
/// The sandbox starts shells on Unix-like systems only.
pub fn is_windows() -> bool {
    cfg!(windows)
}

fn short(host: &str) -> String {
    let host = host.trim();
    let short = host.split('.').next().unwrap_or("");
    if short.is_empty() { "host" } else { short }.to_owned()
}

#[cfg(not(target_arch = "wasm32"))]
fn probe() -> Identity {
    let host = std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| std::env::var("HOSTNAME").unwrap_or_default());
    Identity {
        user: std::env::var("USER").unwrap_or_else(|_| "user".to_owned()),
        host: short(&host),
        home: std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .filter(|h| !h.is_empty())
            .map(PathBuf::from),
    }
}

#[cfg(target_arch = "wasm32")]
fn probe() -> Identity {
    use sicompass_pdk::process::Child;
    use std::time::{Duration, Instant};

    const SCRIPT: &str =
        r#"printf '%s\n%s\n' "${USER:-$(id -un 2>/dev/null)}" "$HOME"; uname -n 2>/dev/null"#;
    let mut out = Vec::new();
    if let Ok(child) = Child::spawn(
        "sh",
        &["-c".to_owned(), SCRIPT.to_owned()],
        None,
        &[],
        &[],
        None,
    ) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let exited = child.try_wait().is_some();
            loop {
                let chunk = child.read(1 << 16);
                if chunk.is_empty() {
                    break;
                }
                out.extend(chunk);
            }
            if exited || Instant::now() > deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    let text = String::from_utf8_lossy(&out);
    let mut lines = text.lines();
    let user = lines.next().unwrap_or("").trim();
    let home = lines.next().unwrap_or("").trim();
    Identity {
        user: if user.is_empty() { "user" } else { user }.to_owned(),
        host: short(lines.next().unwrap_or("")),
        home: (!home.is_empty()).then(|| PathBuf::from(home)),
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn a_host_name_is_cut_at_its_first_dot() {
        assert_eq!(short("verysilly.lan\n"), "verysilly");
        assert_eq!(short("box"), "box");
        assert_eq!(short(""), "host");
    }

    #[test]
    fn the_prompt_always_has_a_user_and_a_host() {
        assert!(!user().is_empty());
        assert!(!host().is_empty());
    }
}
