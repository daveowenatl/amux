//! Clean up ugly PTY titles before they reach user-visible chrome.
//!
//! ConPTY on Windows sets the initial window title of a spawned
//! shell to the full absolute path of the shell executable (e.g.
//! `C:\Program Files\WindowsApps\Microsoft.PowerShell_7.6.0.0_arm64__8wekyb3d8bbwe\pwsh.exe`).
//! amux picks that up via `pane.title()` and echoes it to every
//! user-visible title surface: the main window title, the tab bar
//! tab label, any notification-panel entries, and so on.
//!
//! On macOS and Linux this is less of a problem because bash / zsh
//! default `PROMPT_COMMAND` emits an `OSC 0` sequence on every
//! prompt that replaces the initial title with something more
//! useful. pwsh on Windows does set `$Host.UI.RawUI.WindowTitle`
//! from its default prompt function, but that depends on the
//! user's profile actually running the default prompt AND on pwsh
//! translating the title-set into an OSC sequence ConPTY can
//! forward — which doesn't reliably happen on every Windows build
//! we've tested.
//!
//! [`sanitize_pane_title`] is a best-effort cleanup applied at
//! every title consumer (in practice, applied once in
//! `ManagedPane::title()` so every consumer inherits the cleanup
//! for free). Rules:
//!
//! 1. Empty input → `"?"` (matches the raw pane fallback today).
//! 2. Input that is an **absolute** path (starts with `/`, `\\`,
//!    or a drive letter followed by `\`/`/`) AND whose last
//!    segment is a known shell exe → basename minus extension.
//!    Relative paths like `tools/pwsh.exe` are left untouched.
//! 3. Input that looks like a bare shell exe (no path separators
//!    but has a known shell extension) → basename minus extension.
//! 4. Otherwise → passthrough (assume the shell or user set it
//!    deliberately via `OSC 0`, a prompt command, or our own
//!    `user_title` override).
//!
//! See amux #199 for the user-facing context.

use std::borrow::Cow;

/// Known shell executable extensions. We strip any of these (plus
/// their surrounding path) to collapse an ugly absolute shell path
/// into just the shell name.
const SHELL_EXTENSIONS: &[&str] = &[
    ".exe", // Windows-native shells: pwsh.exe, cmd.exe, bash.exe, fish.exe, ...
    ".cmd", // Batch shims
    ".bat", // Batch scripts
    ".ps1", // PowerShell scripts
    ".sh",  // Shell scripts
    ".fish", ".zsh",
];

/// Known shell basenames (lowercased, extension included) that we
/// recognize as "definitely the raw shell executable path, not a
/// user-set title" even when they're bare filenames.
///
/// We could try to do this case-insensitively just with extension
/// matching, but being explicit here avoids accidentally stripping
/// the name of a user-set title that happens to include a file
/// extension (e.g. a workspace named `"README.md"` would not be in
/// this list).
const KNOWN_SHELL_BASENAMES: &[&str] = &[
    "pwsh.exe",
    "powershell.exe",
    "cmd.exe",
    "bash.exe",
    "fish.exe",
    "zsh.exe",
    "sh.exe",
    // Unix-y — rarely appear as a bare title, but cheap to include
    "bash",
    "zsh",
    "fish",
    "sh",
    "pwsh",
];

/// Sanitize a raw pane title for display. Returns `Cow::Borrowed`
/// when the input is already clean (zero allocation) and
/// `Cow::Owned` when it had to be transformed.
pub(crate) fn sanitize_pane_title(raw: &str) -> Cow<'_, str> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Cow::Borrowed("?");
    }

    // Only treat the input as a path-to-collapse if it starts with
    // a real absolute-path prefix. Relative paths like
    // `tools/pwsh.exe` stay as-is — we can't tell whether the user
    // typed that deliberately as an OSC title.
    let looks_like_abs_path = trimmed.starts_with('/')      // Unix absolute
        || trimmed.starts_with("\\\\")                       // Windows UNC
        || has_drive_letter_prefix(trimmed); //               Windows C:\ / C:/

    // Case 1: absolute-path-to-shell, where the final path segment
    // either has a known shell extension (`pwsh.exe`, `bash.sh`) OR
    // is a bare known shell basename (`bash`, `fish`, `zsh`).
    if looks_like_abs_path {
        if let Some(basename) = last_path_segment(trimmed) {
            if let Some(ext) = shell_extension(basename) {
                return Cow::Owned(strip_extension(basename, ext).to_string());
            }
            if is_known_shell_basename(basename) {
                return Cow::Owned(basename.to_string());
            }
        }
    }

    // Case 2: bare shell basename (no separators) — matches any
    // entry in `KNOWN_SHELL_BASENAMES`, case-insensitively. Strip
    // the extension if present so `pwsh.exe` → `pwsh`.
    if is_known_shell_basename(trimmed) {
        if let Some(ext) = shell_extension(trimmed) {
            return Cow::Owned(strip_extension(trimmed, ext).to_string());
        }
        return Cow::Borrowed(trimmed);
    }

    // Case 3: passthrough — shell or user set it deliberately.
    Cow::Borrowed(trimmed)
}

/// Case-insensitive match of `s` against [`KNOWN_SHELL_BASENAMES`].
/// Uses `eq_ignore_ascii_case` so the common passthrough case stays
/// allocation-free (important because this runs from per-frame
/// tab-bar / window-title rendering paths).
fn is_known_shell_basename(s: &str) -> bool {
    KNOWN_SHELL_BASENAMES
        .iter()
        .any(|b| b.eq_ignore_ascii_case(s))
}

/// True if `s` starts with a real absolute Windows drive path
/// (`C:\`, `C:/`, `d:\`, ...). A bare drive-relative prefix like
/// `C:pwsh.exe` is NOT considered absolute — Windows keeps a
/// per-drive "current directory", and those inputs are almost
/// certainly user-typed text rather than raw PTY titles.
fn has_drive_letter_prefix(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

/// Returns the shell extension suffix (e.g. `".exe"`) if `s` ends
/// with one from `SHELL_EXTENSIONS`. Matches case-insensitively
/// without allocating.
fn shell_extension(s: &str) -> Option<&'static str> {
    SHELL_EXTENSIONS
        .iter()
        .find(|ext| ends_with_ignore_ascii_case(s, ext))
        .copied()
}

/// Allocation-free case-insensitive suffix test. `ext` is assumed
/// to already be ASCII lowercase (all entries in
/// `SHELL_EXTENSIONS` are).
fn ends_with_ignore_ascii_case(s: &str, ext: &str) -> bool {
    let s = s.as_bytes();
    let ext = ext.as_bytes();
    if s.len() < ext.len() {
        return false;
    }
    let tail = &s[s.len() - ext.len()..];
    tail.iter()
        .zip(ext)
        .all(|(a, b)| a.to_ascii_lowercase() == *b)
}

/// Return the final path segment of `s`, splitting on either
/// forward or backward slashes.
fn last_path_segment(s: &str) -> Option<&str> {
    s.rsplit(['/', '\\']).next().filter(|seg| !seg.is_empty())
}

/// Strip `ext` from the end of `s` if it ends with `ext` (case-
/// insensitive). Returns `s` unchanged otherwise. Allocation-free.
fn strip_extension<'a>(s: &'a str, ext: &str) -> &'a str {
    if ends_with_ignore_ascii_case(s, ext) {
        &s[..s.len() - ext.len()]
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_empty_becomes_question_mark() {
        assert_eq!(sanitize_pane_title(""), "?");
        assert_eq!(sanitize_pane_title("   "), "?");
    }

    #[test]
    fn passthrough_normal_titles() {
        assert_eq!(sanitize_pane_title("My Workspace"), "My Workspace");
        assert_eq!(sanitize_pane_title("~/src/amux"), "~/src/amux");
        assert_eq!(
            sanitize_pane_title("agent: Claude Code"),
            "agent: Claude Code"
        );
    }

    #[test]
    fn collapses_windows_store_pwsh_path() {
        // The motivating case from #199.
        let raw = r"C:\Program Files\WindowsApps\Microsoft.PowerShell_7.6.0.0_arm64__8wekyb3d8bbwe\pwsh.exe";
        assert_eq!(sanitize_pane_title(raw), "pwsh");
    }

    #[test]
    fn collapses_classic_program_files_pwsh_path() {
        let raw = r"C:\Program Files\PowerShell\7\pwsh.exe";
        assert_eq!(sanitize_pane_title(raw), "pwsh");
    }

    #[test]
    fn collapses_cmd_exe_path() {
        let raw = r"C:\Windows\System32\cmd.exe";
        assert_eq!(sanitize_pane_title(raw), "cmd");
    }

    #[test]
    fn collapses_unix_absolute_bash_path() {
        assert_eq!(sanitize_pane_title("/bin/bash"), "bash");
        assert_eq!(sanitize_pane_title("/opt/homebrew/bin/fish"), "fish");
    }

    #[test]
    fn collapses_bare_shell_basenames() {
        assert_eq!(sanitize_pane_title("pwsh.exe"), "pwsh");
        assert_eq!(sanitize_pane_title("cmd.exe"), "cmd");
        assert_eq!(sanitize_pane_title("bash"), "bash");
        assert_eq!(sanitize_pane_title("zsh"), "zsh");
    }

    #[test]
    fn case_insensitive_extension_match() {
        assert_eq!(sanitize_pane_title(r"C:\WINDOWS\SYSTEM32\CMD.EXE"), "CMD");
        assert_eq!(sanitize_pane_title(r"D:\Apps\Pwsh.Exe"), "Pwsh");
    }

    #[test]
    fn passthrough_forward_slash_non_shell_paths() {
        // User-set OSC titles that happen to contain slashes should
        // pass through untouched.
        assert_eq!(sanitize_pane_title("ls: no such file"), "ls: no such file");
        assert_eq!(sanitize_pane_title("feat/my-branch"), "feat/my-branch");
    }

    #[test]
    fn preserves_non_shell_exe_paths() {
        // An absolute path that doesn't end in a known shell
        // extension shouldn't be collapsed — we can't tell whether
        // it's something the user set deliberately.
        assert_eq!(
            sanitize_pane_title(r"C:\Users\dave\notes.md"),
            r"C:\Users\dave\notes.md"
        );
    }

    #[test]
    fn relative_paths_with_shell_names_pass_through() {
        // A relative path that happens to end in a shell exe name
        // should NOT be collapsed — the user may have typed it as a
        // deliberate OSC title and "absolute path to shell" is the
        // only case we want to rewrite.
        assert_eq!(sanitize_pane_title("tools/pwsh.exe"), "tools/pwsh.exe");
        assert_eq!(
            sanitize_pane_title(r"scripts\build\bash.sh"),
            r"scripts\build\bash.sh"
        );
    }

    #[test]
    fn drive_relative_prefix_is_not_absolute() {
        // `C:pwsh.exe` is a Windows *drive-relative* path, not an
        // absolute one. It should pass through unchanged rather
        // than being collapsed to `C:pwsh`.
        assert_eq!(sanitize_pane_title("C:pwsh.exe"), "C:pwsh.exe");
    }

    #[test]
    fn unc_path_collapses() {
        // Windows UNC paths (`\\server\share\...`) are absolute and
        // should collapse like any other absolute shell path.
        assert_eq!(sanitize_pane_title(r"\\server\share\pwsh.exe"), "pwsh");
    }
}
