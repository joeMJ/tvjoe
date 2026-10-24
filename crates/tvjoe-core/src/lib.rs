//! Kernbibliothek von tvjoe. Enthält keine Oberflächen- und keine Player-Abhängigkeit,
//! damit sie sich unter Linux und Windows ohne GTK testen lässt.

/// Versionsangabe des Programms (aus `Cargo.toml`).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ist_gesetzt() {
        assert!(!VERSION.is_empty());
    }
}
