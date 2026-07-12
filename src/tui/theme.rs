//! Terminal-safe palette.
//!
//! Use classic ANSI named colours (not `Color::Rgb`). macOS 12 Terminal.app
//! mishandles 24-bit truecolour SGR sequences, which showed up as “everything
//! from the cursor downward stays tinted”.

use ratatui::style::Color;

#[derive(Debug, Clone)]
pub struct Theme {
    pub bg: Color,
    pub panel: Color,
    pub border: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub key: Color,
    pub error: Color,
    pub matched: Color,
    pub unmatched: Color,
    pub claim: Color,
    pub batch: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            bg: Color::Black,
            panel: Color::DarkGray,
            border: Color::DarkGray,
            text: Color::Gray,
            muted: Color::DarkGray,
            accent: Color::Cyan,
            key: Color::Yellow,
            error: Color::LightRed,
            matched: Color::Green,
            unmatched: Color::DarkGray,
            claim: Color::Yellow,
            batch: Color::Magenta,
        }
    }
}
