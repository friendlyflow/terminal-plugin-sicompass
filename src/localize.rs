//! The plugin's strings, in the user's language.
//!
//! Inside the sandbox they come from the host (`host::translate`), which has
//! loaded this plugin's `locales/<lang>.ftl` into the app's bundles. Natively,
//! in the unit tests, the English bundle is read directly, so the tests see the
//! same text a user of the English app does. Every id starts with `terminal-`,
//! which the host requires of a plugin's own strings.

#[cfg(target_arch = "wasm32")]
pub fn t(key: &str) -> String {
    sicompass_pdk::host::translate(key)
}

/// The English bundle, read the simple way: `id = text` lines. Enough for
/// this plugin's strings, which take no arguments. A missing id is the id.
#[cfg(not(target_arch = "wasm32"))]
pub fn t(key: &str) -> String {
    include_str!("../locales/en-US.ftl")
        .lines()
        .find_map(|l| {
            let (id, text) = l.split_once(" = ")?;
            (id.trim() == key).then(|| text.to_owned())
        })
        .unwrap_or_else(|| key.to_owned())
}
