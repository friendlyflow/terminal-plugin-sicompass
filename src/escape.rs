//! Keeping text this plugin did not write from being read as markup, and getting
//! it back.
//!
//! `plugin.json` says `"markup": true`, so the app parses every row for HTML-named
//! tags as well as the older `<input>` and `<button>`. A line of program output, a
//! folder's name or a recalled command holding `<b>` is two letters and a bracket,
//! so each goes through [`escape`] on its way into a row. The `entries`, the recall
//! history and `pending_input` hold plain text, and only `fetch` escapes it.
//!
//! The value of a row can come back two ways, and a command must never gain a
//! backslash on the way to the shell:
//!
//! * A value read back out of a row (`extract_input`, or a history button's
//!   function name) is raw, with its escapes. [`unescape`] undoes them.
//! * What the user typed is plain. A shell line can really hold `\<`, so it is not
//!   unescaped. [`reads_back_as`] is how `commit_edit` and `set_input_value` tell
//!   the two apart.

pub use sicompass_sdk::tags::escape;

/// Undo [`escape`]. Text with no `\<` or `\>` passes through untouched, so a lone
/// backslash survives.
pub fn unescape(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\'
            && let Some(bracket) = chars.next_if(|next| matches!(next, '<' | '>'))
        {
            out.push(bracket);
            continue;
        }
        out.push(c);
    }
    out
}

/// Whether `value`, handed over by the app for the live input slot, is only that
/// slot's own text `plain` as the row holds it, rather than something new.
///
/// The app answers Enter on a filled slot with the row's raw `<input>`, not with
/// typed text. That is `plain` escaped once when the slot came from `fetch`, and
/// twice when the app has just filled it from a history button (`fill_live_input`
/// escapes the button's function name, which is already escaped). `plain` itself
/// counts too, so that hearing the same text again changes nothing.
pub fn reads_back_as(value: &str, plain: &str) -> bool {
    value == plain || value == escape(plain) || value == escape(&escape(plain))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescape_undoes_escape() {
        for raw in [
            "<b>x</b>",
            "a < b > c",
            r"\<already\>",
            r"ends in a backslash\",
            r"C:\path",
            "",
        ] {
            assert_eq!(unescape(&escape(raw)), raw);
        }
    }

    #[test]
    fn unescape_leaves_a_lone_backslash_alone() {
        assert_eq!(unescape(r"C:\path\"), r"C:\path\");
    }

    #[test]
    fn a_slot_reads_back_as_its_text_once_or_twice_escaped() {
        let plain = "echo <b>";
        assert!(reads_back_as(plain, plain));
        assert!(reads_back_as(r"echo \<b\>", plain));
        assert!(reads_back_as(r"echo \\<b\\>", plain));
        assert!(!reads_back_as(r"echo \\\<b\\\>", plain));
        assert!(!reads_back_as("echo <i>", plain));
    }
}
