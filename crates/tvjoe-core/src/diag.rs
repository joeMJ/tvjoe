//! Diagnoseprotokoll (Nutzer, 2026-10-28): mit `TVJOE_DIAGNOSE=1` schreibt tvjoe alles, was zur
//! Fehlersuche nötig ist, in eine Datei – eigene Schritte, das vollständige Protokoll von libmpv
//! und ffmpeg, Weitergaben von SAT>IP und DVB, Zustandsbilder. Die Oberfläche richtet die Datei
//! ein ([`set_sink`]); der Kern meldet über [`log`]. Zugangsschlüssel werden vorher entfernt.

use std::sync::OnceLock;

static SINK: OnceLock<fn(&str, &str)> = OnceLock::new();

/// Ziel der Meldungen setzen (einmal beim Start, nur bei eingeschaltetem Protokoll).
pub fn set_sink(f: fn(&str, &str)) {
    let _ = SINK.set(f);
}

/// Diagnoseprotokoll eingeschaltet?
pub fn enabled() -> bool {
    SINK.get().is_some()
}

/// Eine Meldung (`cat`: Bereich, z. B. `satip`, `mpv/ffmpeg`).
pub fn log(cat: &str, msg: &str) {
    if let Some(f) = SINK.get() {
        f(cat, &redact(msg));
    }
}

/// Einstellungen als TOML-Text (Zugangsschlüssel liegen in eigenen Dateien, nicht hier).
pub fn settings_text(s: &crate::Settings) -> String {
    toml::to_string(&s.for_saving()).unwrap_or_else(|e| format!("nicht lesbar: {e}"))
}

/// Zugangsschlüssel aus einem Text entfernen (Plex `X-Plex-Token`, Jellyfin `Token="…"`,
/// Kopfzeilen `Authorization`).
pub fn redact(s: &str) -> String {
    let mut out = s.to_string();
    for key in ["X-Plex-Token=", "X-Plex-Token: ", "Token=\"", "api_key=", "ApiKey="] {
        let mut from = 0;
        while let Some(i) = out[from..].find(key) {
            let start = from + i + key.len();
            let end = out[start..]
                .find(|c: char| c == '&' || c == '"' || c == ' ' || c == ',' || c == '\n' || c == '\'')
                .map_or(out.len(), |e| start + e);
            // Weiter hinter der Ersetzung (immer an einer Zeichengrenze).
            from = if end > start {
                out.replace_range(start..end, "…");
                start + "…".len()
            } else {
                start
            };
            if from >= out.len() {
                break;
            }
        }
    }
    if let Some(i) = out.find("Authorization: ") {
        let start = i + "Authorization: ".len();
        let end = out[start..].find('\n').map_or(out.len(), |e| start + e);
        out.replace_range(start..end, "…");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schluessel_entfernt() {
        assert_eq!(redact("http://s/x?a=1&X-Plex-Token=abc123&b=2"), "http://s/x?a=1&X-Plex-Token=…&b=2");
        assert_eq!(redact("MediaBrowser Client=\"tvjoe\", Token=\"geheim\""), "MediaBrowser Client=\"tvjoe\", Token=\"…\"");
        assert_eq!(redact("Authorization: MediaBrowser x"), "Authorization: …");
        assert_eq!(redact("nichts"), "nichts");
    }
}
