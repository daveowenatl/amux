//! Parser for Gemini CLI dynamic window titles.
//!
//! Gemini emits 80-char-padded terminal titles via OSC 0/2 whenever its
//! StreamingState changes. We parse the prefix to derive a coarse status,
//! which complements the hook-based integration and is the only status
//! signal available for Gemini versions older than v0.26.0 (pre-hooks).
//!
//! Reference: google-gemini/gemini-cli packages/cli/src/utils/windowTitle.ts

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeminiTitleState {
    Ready,
    Working,
    ActionRequired,
}

/// Parse a Gemini dynamic window title and return the state it encodes,
/// or None if the title isn't a Gemini dynamic title (e.g., static mode,
/// a different app, or unrelated output).
pub fn parse_gemini_title(title: &str) -> Option<GeminiTitleState> {
    let trimmed = title.trim();
    if trimmed.starts_with('◇') && trimmed.contains("Ready") {
        Some(GeminiTitleState::Ready)
    } else if trimmed.starts_with('✋') && trimmed.contains("Action Required") {
        Some(GeminiTitleState::ActionRequired)
    } else if trimmed.starts_with('✦') || (trimmed.starts_with('⏲') && trimmed.contains("Working"))
    {
        Some(GeminiTitleState::Working)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ready_state() {
        // Real Gemini output is padded to 80 chars with trailing spaces.
        let title = format!("{:<80}", "◇  Ready (my-project)");
        assert_eq!(parse_gemini_title(&title), Some(GeminiTitleState::Ready));
    }

    #[test]
    fn parses_working_state_with_thought() {
        let title = format!("{:<80}", "✦  Refactoring auth module (my-project)");
        assert_eq!(parse_gemini_title(&title), Some(GeminiTitleState::Working));
    }

    #[test]
    fn parses_silent_working_state() {
        let title = format!("{:<80}", "⏲  Working… (my-project)");
        assert_eq!(parse_gemini_title(&title), Some(GeminiTitleState::Working));
    }

    #[test]
    fn parses_action_required_state() {
        let title = format!("{:<80}", "✋  Action Required (my-project)");
        assert_eq!(
            parse_gemini_title(&title),
            Some(GeminiTitleState::ActionRequired)
        );
    }

    #[test]
    fn returns_none_for_non_gemini_title() {
        assert_eq!(parse_gemini_title("bash - myhost"), None);
        assert_eq!(parse_gemini_title("Gemini CLI (my-project)"), None); // static mode
        assert_eq!(parse_gemini_title(""), None);
        assert_eq!(parse_gemini_title("   "), None);
    }
}
