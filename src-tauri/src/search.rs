//! Turns what the user typed in the search box into an SQL condition.
//!
//! Every word must match (words are combined with AND). A word matches a
//! photo when it appears in its path, camera or place name. Some words mean
//! more than their letters, and then either meaning is enough:
//!
//! | Word                         | Also matches                         |
//! |------------------------------|--------------------------------------|
//! | `2024`                       | photos taken in 2024                 |
//! | `2024-07`, `2024-07-14`      | photos taken that month or day       |
//! | `july`, `jul`                | photos taken in any July             |
//! | `video`, `videos`            | videos                               |
//! | `live`                       | Live Photos                          |
//! | `raw`                        | camera RAW files                     |
//! | `favorite`, `favorites`      | favorites                            |
//! | `red`, `blue`… (label names) | photos with that colour label        |
//! | `rated`, `★★★`               | photos with a rating (at least ★★★)  |
//!
//! The French and German words work too, whatever the app's language:
//! `juillet`, `juli`, `vidéo`, `favoris`, `rouge`, `gelb`, `bewertet`…

use chrono::NaiveDate;

use crate::raw;

const MONTHS: [&str; 12] = [
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];

/// Month names in French and German (and without accents, as people type
/// them), matched in full only: their short forms clash ("jui" is June and
/// July in French).
const OTHER_MONTHS: &[(&str, u32)] = &[
    ("janvier", 1),
    ("février", 2),
    ("fevrier", 2),
    ("mars", 3),
    ("avril", 4),
    ("mai", 5),
    ("juin", 6),
    ("juillet", 7),
    ("août", 8),
    ("aout", 8),
    ("septembre", 9),
    ("octobre", 10),
    ("novembre", 11),
    ("décembre", 12),
    ("decembre", 12),
    ("januar", 1),
    ("jänner", 1),
    ("februar", 2),
    ("märz", 3),
    ("maerz", 3),
    ("marz", 3),
    ("juni", 6),
    ("juli", 7),
    ("oktober", 10),
    ("dezember", 12),
];

/// Colour label names in French and German, for the label they mean.
const OTHER_LABELS: &[(&str, &str)] = &[
    ("rouge", "red"),
    ("jaune", "yellow"),
    ("vert", "green"),
    ("bleu", "blue"),
    ("violet", "purple"),
    ("rot", "red"),
    ("gelb", "yellow"),
    ("grün", "green"),
    ("gruen", "green"),
    ("blau", "blue"),
    ("lila", "purple"),
    ("violett", "purple"),
];

/// The SQL condition for a search, to put after `WHERE`. An empty search
/// matches nothing.
pub fn condition(query: &str) -> String {
    let words: Vec<String> = query
        .split_whitespace()
        .map(|word| word.to_lowercase())
        .take(12) // more than anyone types; keeps the SQL small
        .collect();
    if words.is_empty() {
        return "0".to_owned();
    }
    let conditions: Vec<String> = words.iter().map(|word| word_condition(word)).collect();
    conditions.join(" AND ")
}

fn word_condition(word: &str) -> String {
    let text = text_condition(word);
    match meaning(word) {
        Some(meaning) => format!("({meaning} OR {text})"),
        None => text,
    }
}

/// The word found in the path, the camera or the place.
fn text_condition(word: &str) -> String {
    // Written into the SQL as a quoted LIKE pattern: quotes are doubled, and
    // the characters LIKE treats specially are escaped with a backslash.
    let mut pattern = String::with_capacity(word.len() + 2);
    for c in word.chars().filter(|c| !c.is_control()) {
        match c {
            '\'' => pattern.push_str("''"),
            '\\' | '%' | '_' => {
                pattern.push('\\');
                pattern.push(c);
            }
            _ => pattern.push(c),
        }
    }
    let columns = [
        "rel_path",
        "camera_make",
        "camera_model",
        "place",
        "place_search",
    ];
    let matches: Vec<String> = columns
        .iter()
        .map(|column| format!("{column} LIKE '%{pattern}%' ESCAPE '\\'"))
        .collect();
    format!("({})", matches.join(" OR "))
}

/// What a word means beyond its letters, if anything.
fn meaning(word: &str) -> Option<String> {
    if let Some(range) = date_range(word) {
        return Some(range);
    }
    if word.len() >= 3 {
        if let Some(month) = MONTHS.iter().position(|name| name.starts_with(word)) {
            // Only the three-letter form and the full name: "ma" is not May.
            if word.len() == 3 || word == MONTHS[month] {
                return Some(format!(
                    "CAST(strftime('%m', sort_date / 1000, 'unixepoch') AS INTEGER) = {}",
                    month + 1
                ));
            }
        }
    }
    if let Some(&(_, month)) = OTHER_MONTHS.iter().find(|(name, _)| *name == word) {
        return Some(format!(
            "CAST(strftime('%m', sort_date / 1000, 'unixepoch') AS INTEGER) = {month}"
        ));
    }
    let word = OTHER_LABELS
        .iter()
        .find(|(name, _)| *name == word)
        .map_or(word, |(_, label)| *label);
    match word {
        "video" | "videos" | "vidéo" | "vidéos" => Some("kind = 1".to_owned()),
        "live" => Some("live_video IS NOT NULL".to_owned()),
        "favorite" | "favorites" | "favourite" | "favourites" | "favori" | "favoris"
        | "favorit" | "favoriten" => Some(
            "fingerprint IN (SELECT key FROM org_state WHERE kind = 'fav' AND value = '1')"
                .to_owned(),
        ),
        "rated" | "noté" | "notée" | "notées" | "notés" | "bewertet" => Some(
            "fingerprint IN (SELECT key FROM org_state WHERE kind = 'rating' AND value != '0')"
                .to_owned(),
        ),
        _ if crate::org::LABELS.contains(&word) => Some(format!(
            "fingerprint IN (SELECT key FROM org_state WHERE kind = 'label' AND value = '{word}')"
        )),
        _ if !word.is_empty() && word.chars().all(|c| c == '★') && word.chars().count() <= 5 => {
            Some(format!(
                "fingerprint IN (SELECT key FROM org_state
                     WHERE kind = 'rating' AND CAST(value AS INTEGER) >= {})",
                word.chars().count()
            ))
        }
        // A RAW file by itself, or a JPEG + RAW pair.
        "raw" => Some(format!(
            "(raw_file IS NOT NULL OR {})",
            raw::is_raw_sql("rel_path")
        )),
        _ => None,
    }
}

/// `2024`, `2024-07` or `2024-07-14` as a range of `sort_date`.
fn date_range(word: &str) -> Option<String> {
    let mut parts = word.split('-');
    let year_text = parts.next()?;
    if year_text.len() != 4 {
        return None;
    }
    let year: i32 = year_text
        .parse()
        .ok()
        .filter(|y| (1900..=2100).contains(y))?;
    let month: Option<u32> = parts
        .next()
        .map(|text| text.parse().ok())
        .unwrap_or(Some(0));
    let day: Option<u32> = parts
        .next()
        .map(|text| text.parse().ok())
        .unwrap_or(Some(0));
    if parts.next().is_some() {
        return None;
    }
    let (month, day) = (month?, day?);
    let (first, next) = match (month, day) {
        (0, 0) => (
            NaiveDate::from_ymd_opt(year, 1, 1)?,
            NaiveDate::from_ymd_opt(year + 1, 1, 1)?,
        ),
        (month, 0) => {
            let first = NaiveDate::from_ymd_opt(year, month, 1)?;
            (first, first.checked_add_months(chrono::Months::new(1))?)
        }
        (month, day) => {
            let first = NaiveDate::from_ymd_opt(year, month, day)?;
            (first, first.succ_opt()?)
        }
    };
    let ms = |date: NaiveDate| {
        date.and_hms_opt(0, 0, 0)
            .map(|t| t.and_utc().timestamp_millis())
    };
    Some(format!(
        "(sort_date >= {} AND sort_date < {})",
        ms(first)?,
        ms(next)?
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(
            date_range("2024").unwrap(),
            "(sort_date >= 1704067200000 AND sort_date < 1735689600000)"
        );
        assert_eq!(
            date_range("2024-12").unwrap(),
            "(sort_date >= 1733011200000 AND sort_date < 1735689600000)"
        );
        assert_eq!(
            date_range("2024-07-14").unwrap(),
            "(sort_date >= 1720915200000 AND sort_date < 1721001600000)"
        );
        for not_a_date in [
            "24",
            "20240",
            "2024-13",
            "2024-02-30",
            "2024-x",
            "2024-1-1-1",
            "3024",
        ] {
            assert_eq!(date_range(not_a_date), None, "{not_a_date}");
        }
    }

    #[test]
    fn words_with_a_meaning() {
        assert!(meaning("jul").unwrap().ends_with("= 7"));
        assert!(meaning("december").unwrap().ends_with("= 12"));
        assert_eq!(meaning("ma"), None);
        assert_eq!(meaning("marc"), None);
        assert_eq!(meaning("video").as_deref(), Some("kind = 1"));
        assert!(meaning("raw").unwrap().contains("'%.nef'"));
        assert!(meaning("red").unwrap().contains("value = 'red'"));
        assert!(meaning("★★★").unwrap().contains(">= 3"));
        assert!(meaning("rated").is_some());
        assert_eq!(meaning("redd"), None);
        assert!(meaning("juillet").unwrap().ends_with("= 7"));
        assert!(meaning("märz").unwrap().ends_with("= 3"));
        assert_eq!(meaning("jui"), None);
        assert_eq!(meaning("vidéo").as_deref(), Some("kind = 1"));
        assert!(meaning("gelb").unwrap().contains("value = 'yellow'"));
        assert!(meaning("favoris").is_some());
        assert_eq!(meaning("canon"), None);
    }

    #[test]
    fn special_characters_stay_inside_the_pattern() {
        let sql = condition("o'brien 100%_\\");
        assert!(sql.contains("'%o''brien%'"), "{sql}");
        assert!(sql.contains("'%100\\%\\_\\\\%'"), "{sql}");
        assert_eq!(condition("   "), "0");
    }
}
