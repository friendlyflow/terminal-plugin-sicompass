//! Who and where the user is, for the synthesized prompt: their user name,
//! the machine's short name, and their home folder.
//!
//! The plugin runs with the user's environment, so they come from there, and
//! the machine's name from the system (`gethostname`).

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
pub fn is_windows() -> bool {
    cfg!(windows)
}

fn short(host: &str) -> String {
    let host = host.trim();
    let short = host.split('.').next().unwrap_or("");
    if short.is_empty() { "host" } else { short }.to_owned()
}

fn probe() -> Identity {
    let var = |names: &[&str]| {
        names
            .iter()
            .find_map(|n| std::env::var(n).ok().filter(|v| !v.trim().is_empty()))
    };
    Identity {
        user: var(&["USER", "LOGNAME", "USERNAME"]).unwrap_or_else(|| "user".to_owned()),
        host: short(
            &hostname()
                .or_else(|| var(&["HOSTNAME", "COMPUTERNAME"]))
                .unwrap_or_default(),
        ),
        home: var(&["HOME", "USERPROFILE"]).map(PathBuf::from),
    }
}

/// The machine's name, from the system.
#[cfg(unix)]
fn hostname() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: `gethostname` writes at most `len` bytes into the buffer.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc != 0 {
        return None;
    }
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    let name = String::from_utf8_lossy(&buf[..end]).into_owned();
    (!name.trim().is_empty()).then_some(name)
}

/// On Windows the name is `%COMPUTERNAME%`, which the caller reads.
#[cfg(not(unix))]
fn hostname() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_name_is_cut_at_its_first_dot() {
        assert_eq!(short("verysilly.lan\n"), "verysilly");
        assert_eq!(short("box"), "box");
        assert_eq!(short(""), "host");
    }

    #[cfg(unix)]
    #[test]
    fn the_host_name_comes_from_the_system() {
        let name = hostname().expect("gethostname");
        assert!(!name.contains('\0'));
        assert_eq!(host(), short(&name));
    }

    #[test]
    fn the_prompt_always_has_a_user_and_a_host() {
        assert!(!user().is_empty());
        assert!(!host().is_empty());
    }
}
