//! Sprachen der Oberfläche (Nutzer, 2026-10-27).
//!
//! Ausgangssprache ist Deutsch: Jeder Text steht deutsch im Quelltext und dient zugleich als
//! Schlüssel (`msgid`) in den gettext-Katalogen `po/<sprache>.po`. Die Kataloge sind eingebaut
//! und werden hier ohne libintl gelesen; eine eigene `<sprache>.po` im Ordner `sprachen/` der
//! Konfiguration hat Vorrang. Fehlt eine Übersetzung, erscheint der deutsche Text.
//!
//! - [`tr`] für feste Texte,
//! - [`tf!`](crate::tf) für Texte mit benannten Platzhaltern (`{titel}`),
//! - [`trn`] für Einzahl und Mehrzahl.
//!
//! Die Sprache wird einmal beim Start gewählt ([`init`]); ein Wechsel wirkt nach dem Neustart.

use std::collections::HashMap;
use std::fmt::Display;
use std::path::Path;
use std::sync::OnceLock;

/// Eingebaute Kataloge (Sprachkürzel, Inhalt der `.po`).
const BUILTIN: &[(&str, &str)] = &[("en", include_str!("../../../po/en.po"))];

/// Wählbare Sprachen (Kürzel, Name in der eigenen Sprache); leeres Kürzel = Systemsprache.
pub const LANGUAGES: &[(&str, &str)] = &[("de", "Deutsch"), ("en", "English")];

struct Catalog {
    lang: String,
    map: HashMap<String, Vec<String>>,
}

static CATALOG: OnceLock<Catalog> = OnceLock::new();

/// Sprache festlegen. `choice`: Einstellung (leer = Systemsprache), `system`: bevorzugte
/// Sprachen des Systems (z. B. `de_DE.UTF-8`, `en`), `override_dir`: Ordner mit eigenen
/// Katalogen. Nur der erste Aufruf wirkt; gibt das gewählte Kürzel zurück.
pub fn init(choice: &str, system: &[String], override_dir: Option<&Path>) -> String {
    let lang = pick(choice, system, override_dir);
    let mut map = HashMap::new();
    if lang != "de" {
        let own = override_dir
            .map(|d| d.join(format!("{lang}.po")))
            .and_then(|p| std::fs::read_to_string(p).ok());
        let builtin = BUILTIN.iter().find(|(l, _)| *l == lang).map(|(_, s)| *s);
        // Eigener Katalog ergänzt bzw. überschreibt den eingebauten.
        if let Some(b) = builtin {
            map = parse_po(b);
        }
        if let Some(o) = own {
            map.extend(parse_po(&o));
        }
    }
    CATALOG.get_or_init(|| Catalog { lang, map }).lang.clone()
}

/// Gewählte Sprache (vor [`init`]: Deutsch).
pub fn lang() -> &'static str {
    CATALOG.get().map(|c| c.lang.as_str()).unwrap_or("de")
}

fn pick(choice: &str, system: &[String], override_dir: Option<&Path>) -> String {
    let available = |l: &str| {
        l == "de"
            || BUILTIN.iter().any(|(b, _)| *b == l)
            || override_dir.is_some_and(|d| d.join(format!("{l}.po")).is_file())
    };
    let choice = choice.trim();
    if !choice.is_empty() && available(choice) {
        return choice.to_string();
    }
    for s in system {
        // „de_DE.UTF-8@euro“ → „de“; „C“/„POSIX“ überspringen.
        let l = s
            .split(['_', '.', '@', '-'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if l.len() >= 2 && l != "c" && l != "posix" && available(&l) {
            return l;
        }
    }
    // Ohne passende Systemsprache: Englisch, sofern das System nicht Deutsch ist.
    if system.iter().any(|s| s.to_ascii_lowercase().starts_with("de")) {
        "de".into()
    } else if system.iter().any(|s| {
        let s = s.to_ascii_lowercase();
        s != "c" && s != "posix" && !s.starts_with("c.")
    }) {
        "en".into()
    } else {
        "de".into()
    }
}

/// Bevorzugte Sprachen aus den Umgebungsvariablen (`LANGUAGE`, `LC_ALL`, `LC_MESSAGES`, `LANG`).
pub fn env_languages() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(v) = std::env::var("LANGUAGE") {
        out.extend(v.split(':').filter(|s| !s.is_empty()).map(String::from));
    }
    for k in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(k)
            && !v.is_empty()
        {
            out.push(v);
            break;
        }
    }
    out
}

/// Übersetzung eines festen Textes.
pub fn tr(s: &'static str) -> &'static str {
    match CATALOG.get().and_then(|c| c.map.get(s)).and_then(|v| v.first()) {
        Some(t) if !t.is_empty() => t.as_str(),
        _ => s,
    }
}

/// Einzahl oder Mehrzahl je nach `n`; Platzhalter `{n}` wird ersetzt.
pub fn trn(singular: &'static str, plural: &'static str, n: u64) -> String {
    let form = plural_form(lang(), n);
    let text = CATALOG
        .get()
        .and_then(|c| c.map.get(singular))
        .and_then(|v| v.get(form))
        .filter(|t| !t.is_empty())
        .map(String::as_str)
        .unwrap_or(if n == 1 { singular } else { plural });
    text.replace("{n}", &n.to_string())
}

/// Mehrzahlregel je Sprache (Index in `msgstr[i]`).
fn plural_form(lang: &str, n: u64) -> usize {
    match lang {
        "fr" => usize::from(n > 1),
        _ => usize::from(n != 1),
    }
}

/// Benannte Platzhalter `{name}` ersetzen (für [`tf!`](crate::tf)).
pub fn fill(text: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = text.to_string();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), &value.to_string());
    }
    out
}

/// Übersetzter Text mit benannten Platzhaltern:
/// `tf!("Aufnahme läuft: {titel}", titel = t.title)`.
#[macro_export]
macro_rules! tf {
    ($text:literal $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::i18n::fill(
            $crate::i18n::tr($text),
            &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),*],
        )
    };
}

/// gettext-Katalog lesen: `msgid` → `msgstr` bzw. `msgstr[i]`. Unscharfe (`#, fuzzy`) und
/// leere Übersetzungen sowie der Kopf werden übergangen.
fn parse_po(text: &str) -> HashMap<String, Vec<String>> {
    #[derive(PartialEq)]
    enum Field {
        None,
        Ctxt,
        Id,
        IdPlural,
        Str(usize),
    }
    let mut map = HashMap::new();
    let mut fuzzy = false;
    let mut id = String::new();
    let mut strs: Vec<String> = Vec::new();
    let mut field = Field::None;
    let mut flush = |id: &mut String, strs: &mut Vec<String>, fuzzy: &mut bool| {
        if !id.is_empty() && !*fuzzy && strs.iter().any(|s| !s.is_empty()) {
            map.insert(std::mem::take(id), std::mem::take(strs));
        }
        id.clear();
        strs.clear();
        *fuzzy = false;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(c) = line.strip_prefix("#,") {
            if field != Field::None {
                flush(&mut id, &mut strs, &mut fuzzy);
                field = Field::None;
            }
            fuzzy |= c.split(',').any(|f| f.trim() == "fuzzy");
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let (key, rest) = match line.find(' ') {
            Some(i) if !line.starts_with('"') => (&line[..i], line[i..].trim()),
            _ => ("", line),
        };
        let value = unquote(rest);
        match key {
            "msgctxt" => {
                if field != Field::None {
                    flush(&mut id, &mut strs, &mut fuzzy);
                }
                field = Field::Ctxt;
            }
            "msgid" => {
                if field != Field::Ctxt && field != Field::None {
                    flush(&mut id, &mut strs, &mut fuzzy);
                }
                id = value;
                field = Field::Id;
            }
            "msgid_plural" => field = Field::IdPlural,
            k if k.starts_with("msgstr") => {
                let i = k
                    .strip_prefix("msgstr[")
                    .and_then(|r| r.strip_suffix(']'))
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0);
                if strs.len() <= i {
                    strs.resize(i + 1, String::new());
                }
                strs[i] = value;
                field = Field::Str(i);
            }
            "" => match field {
                Field::Id => id.push_str(&value),
                Field::Str(i) => strs[i].push_str(&value),
                _ => {}
            },
            _ => {}
        }
    }
    flush(&mut id, &mut strs, &mut fuzzy);
    map
}

/// Inhalt eines `"…"`-Abschnitts mit C-Escapes.
fn unquote(s: &str) -> String {
    let s = s.trim();
    let s = s.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(s);
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(o) => out.push(o),
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PO: &str = r#"
msgid ""
msgstr ""
"Content-Type: text/plain; charset=UTF-8\n"

#: app.rs:1
msgid "Aufnahme läuft: {titel}"
msgstr "Recording: {titel}"

msgid ""
"Zwei "
"Zeilen"
msgstr "Two\n"
"lines"

#, fuzzy
msgid "Unscharf"
msgstr "Fuzzy"

msgid "Leer"
msgstr ""

msgid "eine Aufnahme"
msgid_plural "{n} Aufnahmen"
msgstr[0] "one recording"
msgstr[1] "{n} recordings"

msgid "Mit \"Zeichen\" \\ dabei"
msgstr "With \"quotes\" \\ inside"
"#;

    #[test]
    fn reads_po() {
        let m = parse_po(PO);
        assert_eq!(m["Aufnahme läuft: {titel}"], vec!["Recording: {titel}"]);
        assert_eq!(m["Zwei Zeilen"], vec!["Two\nlines"]);
        assert!(!m.contains_key("Unscharf"));
        assert!(!m.contains_key("Leer"));
        assert!(!m.contains_key(""));
        assert_eq!(m["eine Aufnahme"][1], "{n} recordings");
        assert_eq!(m["Mit \"Zeichen\" \\ dabei"], vec!["With \"quotes\" \\ inside"]);
    }

    #[test]
    fn builtin_catalogs_parse() {
        for (_, text) in BUILTIN {
            parse_po(text);
        }
    }

    #[test]
    fn picks_language() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(pick("", &s(&["de_DE.UTF-8"]), None), "de");
        assert_eq!(pick("", &s(&["en_GB.UTF-8"]), None), "en");
        assert_eq!(pick("", &s(&["fr_FR.UTF-8"]), None), "en");
        assert_eq!(pick("", &s(&["C"]), None), "de");
        assert_eq!(pick("en", &s(&["de_DE"]), None), "en");
        assert_eq!(pick("de", &s(&["en_US"]), None), "de");
        assert_eq!(pick("xx", &s(&["de_AT"]), None), "de");
    }

    #[test]
    fn fills_placeholders() {
        assert_eq!(fill("{a} und {b}", &[("a", &1), ("b", &"x")]), "1 und x");
        assert_eq!(plural_form("de", 1), 0);
        assert_eq!(plural_form("fr", 0), 0);
        assert_eq!(plural_form("en", 0), 1);
    }
}
