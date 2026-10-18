use std::cmp::Ordering;
use std::iter::Peekable;
use std::str::Chars;

pub fn fold(s: &str) -> String {
    s.chars()
        .filter(|c| !('\u{0300}'..='\u{036f}').contains(c))
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' | 'ã' | 'å' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' | 'õ' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            other => other,
        })
        .collect()
}

fn take_number(chars: &mut Peekable<Chars<'_>>) -> String {
    let mut number = String::new();
    while let Some(c) = chars.peek().copied().filter(char::is_ascii_digit) {
        number.push(c);
        chars.next();
    }
    number
}

pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let (na, nb) = (take_number(&mut a), take_number(&mut b));
                let (na, nb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let order = na.len().cmp(&nb.len()).then_with(|| na.cmp(nb));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(&y);
                }
                a.next();
                b.next();
            }
        }
    }
}

pub fn matches_all_words(folded_text: &str, folded_query: &str) -> bool {
    folded_query.split_whitespace().all(|word| folded_text.contains(word))
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b' ') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |range: std::ops::Range<usize>| s.get(range)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, se) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let mut idx = 19;
    if b.get(idx) == Some(&b'.') {
        idx += 1;
        while b.get(idx).is_some_and(u8::is_ascii_digit) {
            idx += 1;
        }
    }
    let offset = match b.get(idx) {
        None | Some(b'Z') => 0,
        Some(&sign @ (b'+' | b'-')) => {
            let hours = num(idx + 1..idx + 3)?;
            let minutes = num(idx + 4..idx + 6)?;
            (hours * 3600 + minutes * 60) * if sign == b'+' { 1 } else { -1 }
        }
        _ => return None,
    };
    Some(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + se - offset)
}

pub fn format_date(epoch: i64) -> String {
    let (y, m, d) = civil_from_days(epoch.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_ignores_case_and_accents_in_both_unicode_forms() {
        assert_eq!(fold("Presentación Pablo"), "presentacion pablo");
        assert_eq!(fold("Presentacio\u{301}n"), "presentacion");
        assert_eq!(fold("PEQUEÑAJA"), "pequenaja");
    }

    #[test]
    fn natural_order_compares_numbers_by_value() {
        let mut names = vec!["foto 10", "foto 2", "foto 1", "foto 02a"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, vec!["foto 1", "foto 2", "foto 02a", "foto 10"]);
        assert_eq!(natural_cmp("2015-02", "2015-10"), Ordering::Less);
        assert_eq!(natural_cmp("a", "a"), Ordering::Equal);
        assert_eq!(natural_cmp("a", "ab"), Ordering::Less);
    }

    #[test]
    fn query_words_can_match_in_any_order() {
        assert!(matches_all_words("2015 toledo raw", "raw toledo"));
        assert!(!matches_all_words("2015 toledo", "toledo raw"));
        assert!(matches_all_words("cualquiera", "   "));
    }

    #[test]
    fn rfc3339_parses_known_instants() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(parse_rfc3339("2000-03-01T02:00:00.250+02:00"), Some(951_868_800));
        assert_eq!(parse_rfc3339("2000-03-01T00:00:00-01:30"), Some(951_868_800 + 5400));
        assert_eq!(parse_rfc3339("basura"), None);
        assert_eq!(parse_rfc3339(""), None);
    }

    #[test]
    fn dates_round_trip() {
        let epoch = parse_rfc3339("2026-07-16T10:10:45Z").unwrap();
        assert_eq!(format_date(epoch), "2026-07-16");
        assert_eq!(format_date(0), "1970-01-01");
        assert_eq!(format_date(parse_rfc3339("2024-02-29T23:59:59Z").unwrap()), "2024-02-29");
    }
}
