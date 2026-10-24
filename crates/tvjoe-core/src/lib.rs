//! Kernbibliothek von tvjoe. Enthält keine Oberflächen- und keine Player-Abhängigkeit,
//! damit sie sich unter Linux und Windows ohne GTK testen lässt.

pub mod check;
pub mod ids;
pub mod m3u;

pub use check::{Finding, FindingKind, Severity};
pub use ids::normalize_id;
pub use m3u::{Entry, ParseError, Playlist, Quality, SourceKind};

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
