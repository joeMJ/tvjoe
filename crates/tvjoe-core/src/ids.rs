//! Normalisierung von Kennungen für den Vergleich zwischen M3U (`tvg-id`) und EPG (Kanal-`id`).

/// Kleinschreibung, nur Buchstaben und Ziffern: `Das.Erste.de` und `DasErste.de` ergeben beide `daserstede`.
pub fn normalize_id(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn punkte_und_grossschreibung_egal() {
        assert_eq!(normalize_id("DasErste.de"), normalize_id("Das.Erste.de"));
        assert_eq!(normalize_id("ARD-alpha.de"), normalize_id("ARD.alpha.de"));
        assert_eq!(normalize_id("One.de"), normalize_id("ONE.de"));
        assert_eq!(normalize_id("phoenix.de"), normalize_id("PHOENIX.de"));
    }

    #[test]
    fn verschiedene_sender_bleiben_verschieden() {
        assert_ne!(normalize_id("ZDF.de"), normalize_id("ZDFneo.de"));
    }

    #[test]
    fn umlaute_bleiben_erhalten() {
        assert_eq!(normalize_id("Österreich 1"), "österreich1");
    }
}
