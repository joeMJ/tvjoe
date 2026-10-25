//! Kernbibliothek von tvjoe. Enthält keine Oberflächen- und keine Player-Abhängigkeit,
//! damit sie sich unter Linux und Windows ohne GTK testen lässt.

pub mod backup;
pub mod check;
pub mod epg;
pub mod ids;
pub mod jellyfin;
pub mod jellyfin_lib;
pub mod library;
pub mod logomatch;
pub mod m3u;
pub mod mediathek;
pub mod net;
pub mod paths;
pub mod satip;
pub mod settings;
pub mod store;
pub mod syscheck;
pub mod ts;
pub mod update;

pub use check::{Finding, FindingKind, Severity};
pub use ids::normalize_id;
pub use library::{Channel, ImportOutcome, Library, NumberTaken, ScannedChannel, Source, Stream};
pub use m3u::{Entry, ParseError, Playlist, Quality, SourceKind};
pub use settings::{Fallback, Settings, Strategy};
pub use store::StoreError;

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
