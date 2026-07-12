//! Clipboard helpers.

use anyhow::{Context, Result};

pub fn copy_text(text: &str) -> Result<()> {
    let mut clipboard = arboard::Clipboard::new().context("open system clipboard")?;
    clipboard
        .set_text(text.to_string())
        .context("write clipboard")?;
    Ok(())
}
