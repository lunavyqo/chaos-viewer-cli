//! Dark agent-CLI inspired palette.

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
            bg: Color::Rgb(18, 18, 22),
            panel: Color::Rgb(28, 28, 36),
            border: Color::Rgb(55, 55, 70),
            text: Color::Rgb(230, 230, 235),
            muted: Color::Rgb(140, 140, 155),
            accent: Color::Rgb(120, 200, 255), // soft cyan
            key: Color::Rgb(255, 200, 120),    // keycaps stand out
            error: Color::Rgb(255, 120, 120),
            matched: Color::Rgb(80, 200, 140),
            unmatched: Color::Rgb(150, 150, 160),
            claim: Color::Rgb(230, 190, 80),  // gold locks
            batch: Color::Rgb(190, 150, 255), // violet = in prompt batch
        }
    }
}
