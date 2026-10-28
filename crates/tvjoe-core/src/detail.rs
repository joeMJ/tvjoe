//! Zusätzliche Angaben zu einem Eintrag (Nutzer, 2026-10-28): Die Beschreibung einer Bibliothek
//! (`[detail]` im Adapter) sagt, welche Anfrage an den Server geht und wo im Ergebnis welche
//! Angabe steht. tvjoe liest die Werte allgemein aus; neue Felder oder geänderte Server-Formate
//! werden in der TOML-Datei angepasst, nicht im Programm.
//!
//! Pfadsprache: Ebenen mit Punkt (`Media.Part`), `[3]` ein Eintrag, `[]` alle Einträge,
//! `[Feld=Wert]` Filter, `[Feld^=Anfang]` Filter nach Textanfang. Ist eine Ebene eine Liste, gilt
//! der nächste Name für jeden Eintrag; mehrere Treffer ergeben eine Liste.
//! Platzhalter in Vorlagen: `{Pfad}` oder `{Pfad|strip:Anfang}`, `{Pfad|url}` (für Adressen
//! kodiert); dazu `{server}`, `{token}`, `{id}`, `{user_id}`.

use serde::Deserialize;
use serde_json::Value;

/// Abschnitt `[detail]` einer Bibliotheksbeschreibung.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Detail {
    /// Anfrage an den Server (Pfad mit Platzhaltern, z. B. `/library/metadata/{id}`).
    pub request: String,
    /// Wurzel im Ergebnis (z. B. `MediaContainer.Metadata[0]`); leer = ganzes Ergebnis.
    #[serde(default)]
    pub root: String,
    #[serde(default)]
    pub field: Vec<Field>,
    #[serde(default)]
    pub people: Vec<People>,
    #[serde(default)]
    pub link: Vec<Link>,
}

/// Eine Angabe als Zeile „Beschriftung – Wert“.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub label: String,
    #[serde(default)]
    pub label_en: Option<String>,
    /// Fundstelle (Pfad); mehrere Treffer werden mit `join` verbunden.
    pub value: String,
    #[serde(default)]
    pub join: Option<String>,
    /// Zahlen auf so viele Nachkommastellen runden.
    #[serde(default)]
    pub digits: Option<usize>,
    /// Text hinter dem Wert (z. B. „ %“).
    #[serde(default)]
    pub suffix: Option<String>,
    /// Anfang, der vom Wert entfernt wird.
    #[serde(default)]
    pub strip: Option<String>,
}

/// Personen (Besetzung) mit Rolle und Foto.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct People {
    pub label: String,
    #[serde(default)]
    pub label_en: Option<String>,
    /// Liste der Personen (Pfad).
    pub list: String,
    /// Name (Pfad innerhalb eines Eintrags).
    pub name: String,
    #[serde(default)]
    pub role: Option<String>,
    /// Adresse des Fotos (Vorlage mit Pfaden innerhalb eines Eintrags).
    #[serde(default)]
    pub photo: Option<String>,
    /// Höchstens so viele Personen.
    #[serde(default)]
    pub max: Option<usize>,
}

/// Knopf, der eine Seite im Browser öffnet.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Link {
    pub label: String,
    /// Adresse (Vorlage); fehlt ein Wert darin, entfällt der Knopf.
    pub url: String,
    /// Nur wenn diese Bedingung zutrifft (`Pfad=Wert`, z. B. `type=movie`).
    #[serde(default)]
    pub when: Option<String>,
}

/// Ausgelesene Angaben.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Info {
    pub fields: Vec<(String, String)>,
    pub people: Vec<(String, Vec<Person>)>,
    pub links: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    pub name: String,
    pub role: String,
    pub photo: Option<String>,
}

/// Ein Schritt der Pfadsprache.
enum Sel<'a> {
    Index(usize),
    All,
    Eq(&'a str, &'a str),
    Prefix(&'a str, &'a str),
}

/// Werte an einer Fundstelle (leer, wenn es sie nicht gibt).
pub fn select<'a>(v: &'a Value, path: &str) -> Vec<&'a Value> {
    let mut cur: Vec<&Value> = vec![v];
    for seg in path.split('.').filter(|s| !s.is_empty()) {
        let (name, sels) = match seg.find('[') {
            Some(i) => (&seg[..i], &seg[i..]),
            None => (seg, ""),
        };
        if !name.is_empty() {
            let mut next = Vec::new();
            for c in cur {
                match c {
                    Value::Array(a) => next.extend(a.iter().filter_map(|x| x.get(name))),
                    _ => next.extend(c.get(name)),
                }
            }
            cur = next;
        }
        for sel in parse_sels(sels) {
            let mut next = Vec::new();
            for c in cur {
                let items: Vec<&Value> = match c {
                    Value::Array(a) => a.iter().collect(),
                    other => vec![other],
                };
                match sel {
                    Sel::Index(i) => next.extend(items.get(i).copied()),
                    Sel::All => next.extend(items),
                    Sel::Eq(k, want) => {
                        next.extend(items.into_iter().filter(|x| text(&x[k]).as_deref() == Some(want)))
                    }
                    Sel::Prefix(k, p) => next.extend(
                        items
                            .into_iter()
                            .filter(|x| text(&x[k]).is_some_and(|t| t.starts_with(p))),
                    ),
                }
            }
            cur = next;
        }
    }
    // Liegt am Ende eine Liste, zählen ihre Einträge einzeln (Genres, Besetzung).
    cur.into_iter()
        .flat_map(|v| match v {
            Value::Array(a) => a.iter().collect::<Vec<_>>(),
            other => vec![other],
        })
        .filter(|v| !v.is_null())
        .collect()
}

fn parse_sels(s: &str) -> Vec<Sel<'_>> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(start) = rest.find('[') {
        let Some(end) = rest[start..].find(']') else { break };
        let inner = &rest[start + 1..start + end];
        out.push(if inner.is_empty() {
            Sel::All
        } else if let Ok(i) = inner.parse() {
            Sel::Index(i)
        } else if let Some((k, p)) = inner.split_once("^=") {
            Sel::Prefix(k, p)
        } else if let Some((k, v)) = inner.split_once('=') {
            Sel::Eq(k, v)
        } else {
            Sel::All
        });
        rest = &rest[start + end + 1..];
    }
    out
}

/// Wert als Text (Zahlen und Wahrheitswerte ausgeschrieben; Listen und Objekte nicht).
fn text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Vorlage füllen; `None`, wenn ein Platzhalter keinen Wert hat.
pub fn fill(template: &str, v: &Value, extra: &[(&str, &str)]) -> Option<String> {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let end = rest[start..].find('}')? + start;
        let mut parts = rest[start + 1..end].split('|');
        let key = parts.next().unwrap_or("");
        let mut value = match extra.iter().find(|(k, _)| *k == key) {
            Some((_, x)) => x.to_string(),
            None => select(v, key).first().and_then(|x| text(x))?,
        };
        for f in parts {
            if let Some(p) = f.strip_prefix("strip:") {
                value = value.strip_prefix(p).unwrap_or(&value).to_string();
            } else if f == "url" {
                value = urlencode(&value);
            }
        }
        if value.is_empty() {
            return None;
        }
        out.push_str(&value);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

pub fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Angaben aus dem Ergebnis der Anfrage lesen. `english`: englische Beschriftungen, wo vorhanden.
pub fn extract(d: &Detail, result: &Value, extra: &[(&str, &str)], english: bool) -> Info {
    let roots = select(result, &d.root);
    let Some(root) = roots.first() else {
        return Info::default();
    };
    let label = |de: &str, en: &Option<String>| -> String {
        match (english, en) {
            (true, Some(e)) => e.clone(),
            _ => de.to_string(),
        }
    };
    let mut info = Info::default();
    for f in &d.field {
        let values: Vec<String> = select(root, &f.value)
            .into_iter()
            .filter_map(text)
            .map(|t| {
                let t = match &f.strip {
                    Some(p) => t.strip_prefix(p.as_str()).unwrap_or(&t).to_string(),
                    None => t,
                };
                let t = match (f.digits, t.parse::<f64>()) {
                    // Deutsch mit Komma („5,5“).
                    (Some(n), Ok(x)) if english => format!("{x:.n$}"),
                    (Some(n), Ok(x)) => format!("{x:.n$}").replace('.', ","),
                    _ => t,
                };
                format!("{t}{}", f.suffix.as_deref().unwrap_or(""))
            })
            .filter(|t| !t.trim().is_empty())
            .collect();
        if !values.is_empty() {
            info.fields.push((
                label(&f.label, &f.label_en),
                values.join(f.join.as_deref().unwrap_or(" · ")),
            ));
        }
    }
    for p in &d.people {
        let list: Vec<Person> = select(root, &p.list)
            .into_iter()
            .filter_map(|e| {
                let name = select(e, &p.name).first().and_then(|x| text(x))?;
                let role = p
                    .role
                    .as_deref()
                    .and_then(|r| select(e, r).first().and_then(|x| text(x)))
                    .unwrap_or_default();
                let photo = p.photo.as_deref().and_then(|t| fill(t, e, extra));
                Some(Person { name, role, photo })
            })
            .take(p.max.unwrap_or(usize::MAX))
            .collect();
        if !list.is_empty() {
            info.people.push((label(&p.label, &p.label_en), list));
        }
    }
    for l in &d.link {
        if let Some((k, want)) = l.when.as_deref().and_then(|w| w.split_once('=')) {
            if select(root, k).first().and_then(|x| text(x)).as_deref() != Some(want) {
                continue;
            }
        }
        if let Some(url) = fill(&l.url, root, extra) {
            info.links.push((l.label.clone(), url));
        }
    }
    info
}

/// Anfrage an den Server stellen und die Angaben auslesen (`id`: Eintrag).
pub fn load(d: &Detail, acc: &crate::jellyfin::Account, id: &str, english: bool) -> Result<Info, String> {
    let extra = [
        ("server", acc.server.as_str()),
        ("token", acc.token.as_str()),
        ("id", id),
        ("user_id", acc.user_id.as_str()),
    ];
    let path = fill(&d.request, &Value::Null, &extra)
        .ok_or_else(|| crate::tr("Anfrage der Zusatzangaben unvollständig").to_string())?;
    let v = acc.get_path(&path)?;
    Ok(extract(d, &v, &extra, english))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gegen echte Server: `TVJOE_TEST_DETAIL=<Zugangsdatei>:<Beschreibung>:<Eintrag>` (mehrere mit `;`).
    #[test]
    #[ignore]
    fn echte_server() {
        for spec in std::env::var("TVJOE_TEST_DETAIL").unwrap().split(';') {
            let mut it = spec.split(':');
            let (acc, kind, id) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap());
            let acc = crate::jellyfin::Account::load(std::path::Path::new(acc)).unwrap();
            let d = crate::adapter::libraries(None).0.into_iter().find(|d| d.id == kind).unwrap().detail.unwrap();
            let info = load(&d, &acc, id, false).unwrap();
            eprintln!("== {kind}");
            for (k, v) in &info.fields {
                eprintln!("  {k}: {v}");
            }
            for (t, p) in &info.people {
                eprintln!("  {t}: {} Personen, erste: {} ({}), Foto {}", p.len(), p[0].name, p[0].role, p[0].photo.is_some());
            }
            for (t, u) in &info.links {
                eprintln!("  Link {t}: {u}");
            }
            assert!(!info.fields.is_empty());
        }
    }

    #[test]
    fn pfade_und_vorlagen() {
        let v = serde_json::json!({
            "MediaContainer": {"Metadata": [{
                "type": "movie", "rating": 3.66,
                "Director": [{"tag": "John Singleton"}],
                "Guid": [{"id": "imdb://tt0322259"}, {"id": "tmdb://584"}],
                "Media": [{"Part": [{"Stream": [
                    {"streamType": 1, "displayTitle": "720p"},
                    {"streamType": 2, "displayTitle": "Deutsch (DTS 5.1)"},
                    {"streamType": 2, "displayTitle": "English (DTS 5.1)"}
                ]}]}],
                "Role": [{"tag": "Paul Walker", "role": "Brian", "thumb": "https://x/y.jpg"}]
            }]}
        });
        let root = select(&v, "MediaContainer.Metadata[0]")[0];
        assert_eq!(select(root, "Director.tag").len(), 1);
        assert_eq!(select(root, "Media[0].Part[0].Stream[streamType=2].displayTitle").len(), 2);
        assert_eq!(
            fill("https://www.imdb.com/title/{Guid[id^=imdb://].id|strip:imdb://}/", root, &[]).as_deref(),
            Some("https://www.imdb.com/title/tt0322259/")
        );
        assert_eq!(fill("{thumb|url}", &root["Role"][0], &[]).as_deref(), Some("https%3A%2F%2Fx%2Fy.jpg"));
        assert!(fill("{fehlt}", root, &[]).is_none());
        let d: Detail = toml::from_str(
            r#"
            request = "/library/metadata/{id}"
            root = "MediaContainer.Metadata[0]"
            [[field]]
            label = "Ton"
            label_en = "Audio"
            value = "Media[0].Part[0].Stream[streamType=2].displayTitle"
            [[field]]
            label = "Bewertung"
            value = "rating"
            digits = 1
            [[people]]
            label = "Besetzung"
            list = "Role"
            name = "tag"
            role = "role"
            photo = "{server}/photo?url={thumb|url}"
            [[link]]
            label = "TMDB"
            url = "https://www.themoviedb.org/movie/{Guid[id^=tmdb://].id|strip:tmdb://}"
            when = "type=movie"
            [[link]]
            label = "TMDB Serie"
            url = "https://www.themoviedb.org/tv/{Guid[id^=tmdb://].id|strip:tmdb://}"
            when = "type=show"
            "#,
        )
        .unwrap();
        let info = extract(&d, &v, &[("server", "http://s")], true);
        assert_eq!(info.fields[0], ("Audio".into(), "Deutsch (DTS 5.1) · English (DTS 5.1)".into()));
        assert_eq!(info.fields[1].1, "3.7");
        assert_eq!(extract(&d, &v, &[], false).fields[1].1, "3,7");
        assert_eq!(info.people[0].1[0].photo.as_deref(), Some("http://s/photo?url=https%3A%2F%2Fx%2Fy.jpg"));
        assert_eq!(info.links, vec![("TMDB".into(), "https://www.themoviedb.org/movie/584".into())]);
    }
}
