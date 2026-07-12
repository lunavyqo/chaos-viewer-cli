//! Terminal-safe palette for macOS 12+ Terminal.app.
//!
//! Prefer **256-colour indexed** values over `Color::Rgb` (truecolour). RGB was
//! the source of the “cursor and everything below stays tinted” bug on older
//! Terminal.app. Indexed greys give a charcoal look without that breakage.

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
            // 232–255 = grayscale ramp in the xterm 256 palette
            bg: Color::Indexed(234),     // charcoal (not pure black)
            panel: Color::Indexed(236),  // selection / raised surface
            border: Color::Indexed(240), // subtle frame
            text: Color::Indexed(252),   // near-white body text
            muted: Color::Indexed(245),  // secondary text
            accent: Color::Cyan,         // classic ANSI — reliable
            key: Color::Yellow,
            error: Color::LightRed,
            matched: Color::Green,
            unmatched: Color::Indexed(244),
            claim: Color::Yellow,
            batch: Color::Magenta,
        }
    }
}
