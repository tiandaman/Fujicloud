//! When a photo was taken, from where it's kept, for photos that don't say
//! themselves (scans, screenshots, files that lost their EXIF on the way
//! through a messaging app): a date in the file's name, or in the names of
//! the folders it's in, nearest first: "2019-03-15 Birthday", "2019/03/15",
//! "2019/03 - Paris", "March 15, 2019", "July 2019", "Christmas 2019",
//! "2018/Thanksgiving", "Summer '19", "2019".

use std::path::Path;

use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime, Weekday};

/// How much of a date a name gave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precision {
    /// To the second ("IMG_20230715_143012").
    Exact,
    Day,
    Month,
    /// Three months: spring, summer, autumn or winter.
    Season,
    Year,
}

/// A date read from a name: the start of the period it names (naive local
/// time), how long that is, and for a holiday ("Christmas 2019", the month
/// of December) the day itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dated {
    pub start: NaiveDateTime,
    pub precision: Precision,
    pub on: Option<NaiveDate>,
}

fn first_of(year: i32, month: u32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(year, month, 1)
}

fn months_later(d: NaiveDate, n: u32) -> Option<NaiveDate> {
    let m = d.month0() + n;
    first_of(d.year() + (m / 12) as i32, m % 12 + 1)
}

impl Dated {
    fn day(date: NaiveDate) -> Self {
        Self::period(date, Precision::Day)
    }

    fn period(start: NaiveDate, precision: Precision) -> Self {
        Dated {
            start: start.and_time(NaiveTime::MIN),
            precision,
            on: None,
        }
    }

    fn month(year: i32, month: u32) -> Option<Self> {
        first_of(year, month).map(|d| Self::period(d, Precision::Month))
    }

    /// The first moment after the period it names.
    fn end(&self) -> NaiveDateTime {
        let d = self.start.date();
        let next = match self.precision {
            Precision::Exact => return self.start + chrono::Duration::seconds(1),
            Precision::Day => d.succ_opt(),
            Precision::Month => months_later(d, 1),
            Precision::Season => months_later(d, 3),
            Precision::Year => first_of(d.year() + 1, 1),
        };
        next.map_or(self.start, |n| n.and_time(NaiveTime::MIN))
    }
}

/// When a photo at `path` was taken, going by its name and then its
/// folders' (up to `root`, the source, whose own name counts too), for
/// photos with no date of their own. `file_time` (the file's modified time,
/// seconds since 1970) is kept when it falls in the period a folder names,
/// as it's more precise; otherwise the period's start (a holiday's own day)
/// is used, at the file's time of day so photos keep an order within it.
/// `None` if no name says.
pub fn taken_at(path: &Path, root: Option<&Path>, file_time: i64) -> Option<i64> {
    let dated = from_path(path, root)?;
    let file = chrono::DateTime::from_timestamp(file_time, 0).map(|t| t.naive_utc());
    if dated.precision == Precision::Exact {
        return Some(dated.start.and_utc().timestamp());
    }
    // The file's time is UTC and the name's local: allow a day either way.
    let slack = chrono::Duration::hours(14);
    if let Some(f) = file
        && f >= dated.start - slack
        && f < dated.end() + slack
    {
        return Some(file_time);
    }
    let time = file.map_or(NaiveTime::from_hms_opt(12, 0, 0).unwrap(), |f| f.time());
    let day = dated.on.unwrap_or(dated.start.date());
    Some(day.and_time(time).and_utc().timestamp())
}

/// The date a path's names give: the file's own name first, then its
/// folders from the nearest up. Folders that name no year ("03", "03
/// March", "10-21", "15", "Christmas") take it from a folder above them:
/// "2019/03/15", "2019/Christmas", "Summer 2019/July".
pub fn from_path(path: &Path, root: Option<&Path>) -> Option<Dated> {
    if let Some(d) = path
        .file_stem()
        .and_then(|s| s.to_str())
        .and_then(full_date)
    {
        return Some(d);
    }
    let mut below: Vec<Part> = Vec::new();
    let mut dir = path.parent();
    while let Some(d) = dir {
        let Some(name) = d.file_name().and_then(|s| s.to_str()) else {
            break;
        };
        match folder_name(name) {
            Some(Found::Date(found)) => return Some(narrow(found, &below)),
            Some(Found::Part(part)) => below.push(part),
            None => {}
        }
        if root.is_some_and(|r| d == r) {
            break;
        }
        dir = d.parent();
    }
    None
}

/// What a folder's name says without a year.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Part {
    /// Just a number, "03" or "15": a month, or a day under one.
    Number(u32),
    Month(u32),
    MonthDay(u32, u32),
    Holiday(Holiday),
    Season(u32),
}

enum Found {
    Date(Dated),
    Part(Part),
}

/// A dated folder made more precise by the folders below it (nearest
/// first in `below`): "2019" with "03" and "15" below is 15 March 2019.
fn narrow(found: Dated, below: &[Part]) -> Dated {
    if !matches!(
        found.precision,
        Precision::Year | Precision::Season | Precision::Month
    ) {
        return found;
    }
    let year = found.start.year();
    let (mut month, mut day) = match found.precision {
        Precision::Month => (Some(found.start.month()), None),
        _ => (None, None),
    };
    let mut special = None;
    // From the folder just below the dated one down to the photo.
    for part in below.iter().rev() {
        match *part {
            Part::Month(m) if month.is_none() => month = Some(m),
            Part::MonthDay(m, d) if month.is_none() => (month, day) = (Some(m), Some(d)),
            Part::Number(n) if month.is_none() && (1..=12).contains(&n) => month = Some(n),
            Part::Number(n) if month.is_some() && day.is_none() => day = Some(n),
            Part::Holiday(_) | Part::Season(_) if month.is_none() && special.is_none() => {
                special = Some(*part)
            }
            _ => {}
        }
    }
    // A month in winter's second half is in the next year.
    let year_of = |m: u32| {
        if found.precision == Precision::Season && m < found.start.month() {
            year + 1
        } else {
            year
        }
    };
    let narrowed = match (month, day, special) {
        (Some(m), Some(d), _) => NaiveDate::from_ymd_opt(year_of(m), m, d).map(Dated::day),
        (Some(m), None, _) if found.precision != Precision::Month => Dated::month(year_of(m), m),
        (None, _, Some(Part::Holiday(h))) => h.in_year(year),
        (None, _, Some(Part::Season(m))) if found.precision == Precision::Year => season(year, m),
        _ => None,
    };
    narrowed.unwrap_or(found)
}

/// Runs of digits in a name, with where they start.
fn numbers(s: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in s.char_indices() {
        match (c.is_ascii_digit(), start) {
            (true, None) => start = Some(i),
            (false, Some(b)) => {
                out.push((b, &s[b..i]));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(b) = start {
        out.push((b, &s[b..]));
    }
    out
}

fn this_year() -> i32 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    chrono::DateTime::from_timestamp(now, 0).map_or(2100, |t| t.year())
}

fn plausible_year(y: i32) -> bool {
    // Photography's past, up to next year (a camera clock a little ahead).
    (1826..=this_year() + 1).contains(&y)
}

fn ymd(y: &str, m: &str, d: &str) -> Option<NaiveDate> {
    let y: i32 = y.parse().ok()?;
    if !plausible_year(y) || m.len() > 2 || d.len() > 2 {
        return None;
    }
    NaiveDate::from_ymd_opt(y, m.parse().ok()?, d.parse().ok()?)
}

fn hms(h: &str, m: &str, s: &str) -> Option<NaiveTime> {
    if h.len() > 2 || m.len() != 2 || s.len() != 2 {
        return None;
    }
    NaiveTime::from_hms_opt(h.parse().ok()?, m.parse().ok()?, s.parse().ok()?)
}

/// A full date in a run of numbers: "20230715", "2023-07-15", "2023_07_15",
/// "2023.07.15", and the time after it when there is one ("_143012",
/// " 14.30.12", "-143012345"). The only kind a file's own name is read for:
/// "DSCF0001" and "IMG_2023" are numbers, not dates.
fn full_date(s: &str) -> Option<Dated> {
    let nums = numbers(s);
    for (i, &(_, n)) in nums.iter().enumerate() {
        let (date, rest) = if n.len() >= 8 && n.len() != 9 {
            // 20230715, or 20230715143012 run together.
            match ymd(&n[..4], &n[4..6], &n[6..8]) {
                Some(d) => (
                    d,
                    if n.len() >= 14 {
                        vec![&n[8..10], &n[10..12], &n[12..14]]
                    } else {
                        vec![]
                    },
                ),
                None => continue,
            }
        } else if n.len() == 4 && i + 2 < nums.len() {
            match ymd(n, nums[i + 1].1, nums[i + 2].1) {
                Some(d) => (d, vec![]),
                None => continue,
            }
        } else {
            continue;
        };
        let used = if rest.is_empty() && n.len() == 4 {
            i + 3
        } else {
            i + 1
        };
        let time = if rest.len() == 3 {
            hms(rest[0], rest[1], rest[2])
        } else {
            // The time in the next run(s): "143012", "143012345" or "14.30.12".
            match nums.get(used..) {
                Some([(_, t), ..]) if t.len() >= 6 && t.len() <= 9 => {
                    hms(&t[..2], &t[2..4], &t[4..6])
                }
                Some([(_, h), (_, m), (_, sec), ..]) => hms(h, m, sec),
                _ => None,
            }
        };
        return Some(match time {
            Some(t) => Dated {
                start: date.and_time(t),
                precision: Precision::Exact,
                on: None,
            },
            None => Dated::day(date),
        });
    }
    None
}

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

/// A name lowercased, words only, one space apart, with a space at each
/// end so phrases can be found whole: "New Year's Eve" → " new years eve ".
fn words(name: &str) -> String {
    let mut out = String::from(" ");
    for c in name.chars() {
        if c == '\'' || c == '\u{2019}' {
            continue;
        }
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with(' ') {
            out.push(' ');
        }
    }
    if !out.ends_with(' ') {
        out.push(' ');
    }
    out
}

/// A month named in words: "July", "Jul", "Sept". Two months in one name
/// ("June-July") say no single one.
fn month_word(w: &str) -> Option<u32> {
    let mut found = None;
    let tokens: Vec<&str> = w.split(' ').filter(|t| !t.is_empty()).collect();
    for (i, &word) in tokens.iter().enumerate() {
        if word.len() < 3 {
            continue;
        }
        // "May" is a month at either end of a name or beside a number ("May
        // 2019", "2019 May", "Paris May"), not in "Things we may keep".
        let edge = i == 0 || i + 1 == tokens.len();
        let numeric = |j: Option<usize>| {
            j.and_then(|j| tokens.get(j))
                .is_some_and(|t| t.starts_with(|c: char| c.is_ascii_digit()))
        };
        if word == "may" && !edge && !numeric(i.checked_sub(1)) && !numeric(Some(i + 1)) {
            continue;
        }
        let m = if word == "sept" {
            Some(9)
        } else {
            MONTHS
                .iter()
                .position(|m| *m == word || (word.len() == 3 && m.starts_with(word)))
                .map(|i| i as u32 + 1)
        };
        if let Some(m) = m {
            if found.is_some_and(|f| f != m) {
                return None;
            }
            found = Some(m);
        }
    }
    found
}

/// A day number beside a month word: "March 15", "15 March", "march 15th".
/// A month's own number before it labels it ("03 March"), as folders of
/// months are often named.
fn day_beside_month(w: &str) -> Option<u32> {
    let tokens: Vec<&str> = w.split(' ').filter(|t| !t.is_empty()).collect();
    for (i, t) in tokens.iter().enumerate() {
        let Some(month) = month_word(&format!(" {t} ")) else {
            continue;
        };
        if i > 0 && tokens[i - 1].parse::<u32>() == Ok(month) {
            continue;
        }
        for n in [i.checked_sub(1), Some(i + 1)].into_iter().flatten() {
            let Some(tok) = tokens.get(n) else { continue };
            let digits = tok.trim_end_matches(|c: char| c.is_alphabetic());
            let suffix = &tok[digits.len()..];
            if (1..=2).contains(&digits.len())
                && digits.chars().all(|c| c.is_ascii_digit())
                && ["", "st", "nd", "rd", "th"].contains(&suffix)
                && let Ok(d) = digits.parse::<u32>()
                && (1..=31).contains(&d)
            {
                return Some(d);
            }
        }
    }
    None
}

/// Holidays that name a month: on a fixed day, or found each year.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Holiday {
    Fixed(u32, u32),
    /// The nth weekday of a month (thanksgiving: the 4th Thursday of November).
    Nth(u32, Weekday, u8),
    /// The last such weekday of a month (Memorial Day).
    Last(u32, Weekday),
    Easter,
    /// The day after the 4th Thursday of November.
    BlackFriday,
    /// A month, with no one day ("Spring Break").
    Month(u32),
}

/// Longer phrases first: "christmas eve" before "christmas".
const HOLIDAYS: &[(&str, Holiday)] = &[
    ("christmas eve", Holiday::Fixed(12, 24)),
    ("xmas eve", Holiday::Fixed(12, 24)),
    ("christmas", Holiday::Fixed(12, 25)),
    ("xmas", Holiday::Fixed(12, 25)),
    ("boxing day", Holiday::Fixed(12, 26)),
    ("new years eve", Holiday::Fixed(12, 31)),
    ("new year eve", Holiday::Fixed(12, 31)),
    ("nye", Holiday::Fixed(12, 31)),
    ("new years", Holiday::Fixed(1, 1)),
    ("new year", Holiday::Fixed(1, 1)),
    ("valentines", Holiday::Fixed(2, 14)),
    ("valentine", Holiday::Fixed(2, 14)),
    ("st patricks", Holiday::Fixed(3, 17)),
    ("saint patricks", Holiday::Fixed(3, 17)),
    ("st paddys", Holiday::Fixed(3, 17)),
    ("st pattys", Holiday::Fixed(3, 17)),
    ("spring break", Holiday::Month(3)),
    ("easter", Holiday::Easter),
    ("cinco de mayo", Holiday::Fixed(5, 5)),
    ("mothers day", Holiday::Nth(5, Weekday::Sun, 2)),
    ("memorial day", Holiday::Last(5, Weekday::Mon)),
    ("fathers day", Holiday::Nth(6, Weekday::Sun, 3)),
    ("4th of july", Holiday::Fixed(7, 4)),
    ("fourth of july", Holiday::Fixed(7, 4)),
    ("july 4th", Holiday::Fixed(7, 4)),
    ("july 4", Holiday::Fixed(7, 4)),
    ("independence day", Holiday::Fixed(7, 4)),
    ("labor day", Holiday::Nth(9, Weekday::Mon, 1)),
    ("labour day", Holiday::Nth(9, Weekday::Mon, 1)),
    ("halloween", Holiday::Fixed(10, 31)),
    ("thanksgiving", Holiday::Nth(11, Weekday::Thu, 4)),
    ("black friday", Holiday::BlackFriday),
];

impl Holiday {
    fn month(self) -> u32 {
        match self {
            Holiday::Fixed(m, _) | Holiday::Nth(m, _, _) | Holiday::Last(m, _) => m,
            Holiday::Month(m) => m,
            Holiday::Easter => 4,
            Holiday::BlackFriday => 11,
        }
    }

    fn day(self, year: i32) -> Option<NaiveDate> {
        match self {
            Holiday::Fixed(m, d) => NaiveDate::from_ymd_opt(year, m, d),
            Holiday::Nth(m, w, n) => NaiveDate::from_weekday_of_month_opt(year, m, w, n),
            Holiday::Last(m, w) => (1..=5)
                .rev()
                .find_map(|n| NaiveDate::from_weekday_of_month_opt(year, m, w, n)),
            Holiday::Easter => easter(year),
            Holiday::BlackFriday => {
                NaiveDate::from_weekday_of_month_opt(year, 11, Weekday::Thu, 4)?.succ_opt()
            }
            Holiday::Month(_) => None,
        }
    }

    /// The holiday's month, and its day in that year. Easter's month is
    /// its own (March or April).
    fn in_year(self, year: i32) -> Option<Dated> {
        let on = self.day(year);
        let month = on.map_or(self.month(), |d| d.month());
        let mut dated = Dated::month(year, month)?;
        dated.on = on;
        Some(dated)
    }
}

/// Easter Sunday (Gregorian; the anonymous algorithm).
fn easter(year: i32) -> Option<NaiveDate> {
    let (a, b, c) = (year % 19, year / 100, year % 100);
    let (d, e) = (b / 4, b % 4);
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15) % 30;
    let (i, k) = (c / 4, c % 4);
    let l = (32 + 2 * e + 2 * i - h - k) % 7;
    let m = (a + 11 * h + 22 * l) / 451;
    let month = (h + l - 7 * m + 114) / 31;
    let day = (h + l - 7 * m + 114) % 31 + 1;
    NaiveDate::from_ymd_opt(year, month as u32, day as u32)
}

fn holiday(w: &str) -> Option<Holiday> {
    HOLIDAYS
        .iter()
        .find(|(phrase, _)| w.contains(&format!(" {phrase} ")))
        .map(|(_, h)| *h)
}

/// A season's first month: spring March, summer June, autumn September,
/// winter December (into the next year).
fn season_word(w: &str) -> Option<u32> {
    [
        ("spring", 3),
        ("summer", 6),
        ("fall", 9),
        ("autumn", 9),
        ("winter", 12),
    ]
    .into_iter()
    .find(|(word, _)| w.contains(&format!(" {word} ")))
    .map(|(_, m)| m)
}

fn season(year: i32, first_month: u32) -> Option<Dated> {
    first_of(year, first_month).map(|d| Dated::period(d, Precision::Season))
}

/// The one year a name gives: "2019", or "'19" ("Summer '19"). A year
/// stands alone ("1990s" is a decade), and two years are a span, not a date.
fn year_in(name: &str) -> Option<(Option<usize>, i32)> {
    let mut years: Vec<(Option<usize>, i32)> = numbers(name)
        .into_iter()
        .filter(|(at, n)| n.len() == 4 && !name[at + 4..].starts_with(|c: char| c.is_alphabetic()))
        .filter_map(|(at, n)| Some((Some(at), n.parse().ok()?)))
        .filter(|(_, y)| plausible_year(*y))
        .collect();
    for (at, n) in numbers(name) {
        let before = name[..at].chars().next_back();
        if n.len() == 2 && matches!(before, Some('\'' | '\u{2019}')) {
            let yy: i32 = n.parse().ok()?;
            let century = if 2000 + yy <= this_year() + 1 {
                2000
            } else {
                1900
            };
            years.push((None, century + yy));
        }
    }
    match years[..] {
        [one] => Some(one),
        _ => None,
    }
}

/// What a folder's name says: a date (a full one, a month, a season, a
/// holiday or a year), or a part of one waiting for a year from above.
fn folder_name(name: &str) -> Option<Found> {
    if let Some(d) = full_date(name) {
        return Some(Found::Date(Dated::day(d.start.date())));
    }
    let nums = numbers(name);
    // "03-15-2019" (American), "15.03.2019": a day above 12 tells which;
    // otherwise month first.
    if let [(0, a), (_, b), (_, y)] = nums[..]
        && a.len() <= 2
        && b.len() <= 2
        && y.len() == 4
    {
        let (a, b): (u32, u32) = (a.parse().ok()?, b.parse().ok()?);
        let (m, d) = if a > 12 { (b, a) } else { (a, b) };
        if let Some(date) = ymd(y, &m.to_string(), &d.to_string()) {
            return Some(Found::Date(Dated::day(date)));
        }
    }
    let w = words(name);
    let special_holiday = holiday(&w);
    let special_season = season_word(&w);
    let month = month_word(&w).filter(|_| special_holiday.is_none());
    let Some((at, year)) = year_in(name) else {
        if numbers(name).iter().any(|(_, n)| n.len() >= 4) {
            return None;
        }
        // No year: a part of a date for the folders above to complete.
        return Some(Found::Part(if let Some(h) = special_holiday {
            Part::Holiday(h)
        } else if let Some(m) = month {
            match day_beside_month(&w) {
                Some(d) if NaiveDate::from_ymd_opt(2000, m, d).is_some() => Part::MonthDay(m, d),
                _ => Part::Month(m),
            }
        } else if let Some(s) = special_season {
            Part::Season(s)
        } else {
            match nums[..] {
                [(0, n)] if n.len() <= 2 => Part::Number(n.parse().ok()?),
                // "10-21", "10_21 Party".
                [(0, m), (at, d)]
                    if m.len() <= 2
                        && d.len() <= 2
                        && at == m.len() + 1
                        && name[m.len()..].starts_with(['-', '_', '.']) =>
                {
                    let (m, d): (u32, u32) = (m.parse().ok()?, d.parse().ok()?);
                    NaiveDate::from_ymd_opt(2000, m, d)?;
                    Part::MonthDay(m, d)
                }
                _ => return None,
            }
        }));
    };
    if let Some(h) = special_holiday {
        return h.in_year(year).map(Found::Date);
    }
    if let Some(m) = month {
        let date = match day_beside_month(&w) {
            Some(d) => NaiveDate::from_ymd_opt(year, m, d).map(Dated::day),
            None => None,
        };
        return date.or_else(|| Dated::month(year, m)).map(Found::Date);
    }
    if let Some(s) = special_season {
        return season(year, s).map(Found::Date);
    }
    // A month number beside the year: "2023-07", "2023_07 Paris", "07-2023".
    if let Some(at) = at {
        let after = &name[at + 4..];
        let next = after.strip_prefix(['-', '_', '.', ' ']).map(|r| {
            r.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
        });
        let before = name[..at].strip_suffix(['-', '_', '.', ' ']).and_then(|r| {
            let digits: String = r.chars().rev().take_while(char::is_ascii_digit).collect();
            (digits.len() == r.len()).then(|| digits.chars().rev().collect::<String>())
        });
        for m in [next, before].into_iter().flatten() {
            if (1..=2).contains(&m.len())
                && let Some(d) = Dated::month(year, m.parse().ok()?)
            {
                return Some(Found::Date(d));
            }
        }
    }
    first_of(year, 1).map(|d| Found::Date(Dated::period(d, Precision::Year)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap()
    }

    fn dated(path: &str) -> Option<(NaiveDateTime, Precision)> {
        from_path(Path::new(path), Some(Path::new("/photos"))).map(|d| (d.start, d.precision))
    }

    #[test]
    fn dates_in_file_names() {
        use Precision::*;
        for (path, when, precision) in [
            (
                "/photos/IMG_20230715_143012.jpg",
                "2023-07-15 14:30:12",
                Exact,
            ),
            (
                "/photos/PXL_20230715_143012345.jpg",
                "2023-07-15 14:30:12",
                Exact,
            ),
            (
                "/photos/Screenshot_20230715-143012.png",
                "2023-07-15 14:30:12",
                Exact,
            ),
            (
                "/photos/Screenshot 2023-07-15 at 14.30.12.png",
                "2023-07-15 14:30:12",
                Exact,
            ),
            (
                "/photos/2023-07-15 14.30.12.jpg",
                "2023-07-15 14:30:12",
                Exact,
            ),
            (
                "/photos/IMG-20230715-WA0003.jpg",
                "2023-07-15 00:00:00",
                Day,
            ),
            (
                "/photos/signal-2023-07-15-143012.jpg",
                "2023-07-15 14:30:12",
                Exact,
            ),
            ("/photos/20230715143012.jpg", "2023-07-15 14:30:12", Exact),
            ("/photos/scan 2001_12_25.tif", "2001-12-25 00:00:00", Day),
        ] {
            assert_eq!(dated(path), Some((at(when), precision)), "{path}");
        }
    }

    #[test]
    fn numbers_that_arent_dates() {
        for path in [
            "/photos/DSCF0001.JPG",
            "/photos/IMG_2023.HEIC",
            "/photos/IMG_1234_5678.jpg",
            "/photos/20231345_000000.jpg",
            "/photos/Holidays/DSCF0001.JPG",
            "/photos/1990s/DSCF0001.JPG",
            "/photos/2019-2020/DSCF0001.JPG",
            "/photos/123456789/DSCF0001.JPG",
        ] {
            assert_eq!(dated(path), None, "{path}");
        }
    }

    #[test]
    fn dates_in_folder_names() {
        use Precision::*;
        for (path, when, precision) in [
            ("/photos/2019/DSCF0001.JPG", "2019-01-01 00:00:00", Year),
            (
                "/photos/2019 Holidays/DSCF0001.JPG",
                "2019-01-01 00:00:00",
                Year,
            ),
            ("/photos/2019-03/DSCF0001.JPG", "2019-03-01 00:00:00", Month),
            (
                "/photos/2019_03 Paris/DSCF0001.JPG",
                "2019-03-01 00:00:00",
                Month,
            ),
            (
                "/photos/July 2019/DSCF0001.JPG",
                "2019-07-01 00:00:00",
                Month,
            ),
            (
                "/photos/2019 Sept/DSCF0001.JPG",
                "2019-09-01 00:00:00",
                Month,
            ),
            (
                "/photos/2019-03-15 Birthday/DSCF0001.JPG",
                "2019-03-15 00:00:00",
                Day,
            ),
            ("/photos/20190315/DSCF0001.JPG", "2019-03-15 00:00:00", Day),
            // A month below its year.
            ("/photos/2019/03/DSCF0001.JPG", "2019-03-01 00:00:00", Month),
            (
                "/photos/2019/03 March/DSCF0001.JPG",
                "2019-03-01 00:00:00",
                Month,
            ),
            (
                "/photos/2019/March/Paris/DSCF0001.JPG",
                "2019-03-01 00:00:00",
                Month,
            ),
            (
                "/photos/2026/10-21/DSCF0001.JPG",
                "2026-10-21 00:00:00",
                Day,
            ),
            (
                "/photos/2024/02_29 Leap/DSCF0001.JPG",
                "2024-02-29 00:00:00",
                Day,
            ),
            // The nearest folder with a date wins; the file's name first.
            (
                "/photos/2019/2020-05-01/DSCF0001.JPG",
                "2020-05-01 00:00:00",
                Day,
            ),
            (
                "/photos/2019/IMG_20230715_143012.jpg",
                "2023-07-15 14:30:12",
                Exact,
            ),
        ] {
            assert_eq!(dated(path), Some((at(when), precision)), "{path}");
        }
        // Folders above the source don't count, but its own name does.
        assert_eq!(
            from_path(
                Path::new("/2015/photos/x/DSCF0001.JPG"),
                Some(Path::new("/2015/photos"))
            ),
            None
        );
        assert!(from_path(Path::new("/2015/x/DSCF0001.JPG"), Some(Path::new("/2015"))).is_some());
    }

    /// (start of the period, precision, the holiday's own day).
    fn full(path: &str) -> Option<(String, Precision, Option<String>)> {
        from_path(Path::new(path), Some(Path::new("/photos"))).map(|d| {
            (
                d.start.date().to_string(),
                d.precision,
                d.on.map(|o| o.to_string()),
            )
        })
    }

    #[test]
    fn holidays_seasons_and_dates_in_words() {
        use Precision::*;
        let on = |s: &str| Some(s.to_owned());
        for (path, start, precision, day) in [
            (
                "/photos/Christmas 2019/a.jpg",
                "2019-12-01",
                Month,
                on("2019-12-25"),
            ),
            (
                "/photos/2019/Christmas/a.jpg",
                "2019-12-01",
                Month,
                on("2019-12-25"),
            ),
            (
                "/photos/2019/Xmas Eve at Mum's/a.jpg",
                "2019-12-01",
                Month,
                on("2019-12-24"),
            ),
            (
                "/photos/New Year's Eve 2015/a.jpg",
                "2015-12-01",
                Month,
                on("2015-12-31"),
            ),
            (
                "/photos/Thanksgiving 2018/a.jpg",
                "2018-11-01",
                Month,
                on("2018-11-22"),
            ),
            (
                "/photos/Black Friday 2019/a.jpg",
                "2019-11-01",
                Month,
                on("2019-11-29"),
            ),
            (
                "/photos/Easter 2024/a.jpg",
                "2024-03-01",
                Month,
                on("2024-03-31"),
            ),
            (
                "/photos/Easter 2025/a.jpg",
                "2025-04-01",
                Month,
                on("2025-04-20"),
            ),
            (
                "/photos/Mother's Day 2021/a.jpg",
                "2021-05-01",
                Month,
                on("2021-05-09"),
            ),
            (
                "/photos/Memorial Day 2020/a.jpg",
                "2020-05-01",
                Month,
                on("2020-05-25"),
            ),
            (
                "/photos/Father’s Day 2022/a.jpg",
                "2022-06-01",
                Month,
                on("2022-06-19"),
            ),
            (
                "/photos/4th of July 2019/a.jpg",
                "2019-07-01",
                Month,
                on("2019-07-04"),
            ),
            (
                "/photos/Labor Day 2023/a.jpg",
                "2023-09-01",
                Month,
                on("2023-09-04"),
            ),
            (
                "/photos/Halloween '17/a.jpg",
                "2017-10-01",
                Month,
                on("2017-10-31"),
            ),
            (
                "/photos/Valentines 2020/a.jpg",
                "2020-02-01",
                Month,
                on("2020-02-14"),
            ),
            ("/photos/Spring Break 2018/a.jpg", "2018-03-01", Month, None),
            // Seasons, and months inside them.
            ("/photos/Summer 2019/a.jpg", "2019-06-01", Season, None),
            ("/photos/Summer '19/a.jpg", "2019-06-01", Season, None),
            ("/photos/2019/Fall/a.jpg", "2019-09-01", Season, None),
            ("/photos/Summer 2019/July/a.jpg", "2019-07-01", Month, None),
            (
                "/photos/Winter 2019/January/a.jpg",
                "2020-01-01",
                Month,
                None,
            ),
            // Dates in words and other orders.
            ("/photos/March 15, 2019/a.jpg", "2019-03-15", Day, None),
            (
                "/photos/15th March 2019 - Party/a.jpg",
                "2019-03-15",
                Day,
                None,
            ),
            ("/photos/Dec 2019/a.jpg", "2019-12-01", Month, None),
            ("/photos/03-15-2019/a.jpg", "2019-03-15", Day, None),
            ("/photos/15.03.2019/a.jpg", "2019-03-15", Day, None),
            ("/photos/07-2019/a.jpg", "2019-07-01", Month, None),
            // Years, months and days as nested folders.
            ("/photos/2019/03/15/a.jpg", "2019-03-15", Day, None),
            ("/photos/2019/03 March/15/a.jpg", "2019-03-15", Day, None),
            ("/photos/2019/March 15/a.jpg", "2019-03-15", Day, None),
        ] {
            assert_eq!(
                full(path),
                Some((start.to_owned(), precision, day)),
                "{path}"
            );
        }
        // No year anywhere: no date.
        assert_eq!(full("/photos/Christmas/a.jpg"), None);
        assert_eq!(full("/photos/03/15/a.jpg"), None);
        assert_eq!(
            full("/photos/2019/Things we may keep/a.jpg"),
            Some(("2019-01-01".to_owned(), Year, None))
        );
        assert_eq!(
            full("/photos/May 2019/a.jpg"),
            Some(("2019-05-01".to_owned(), Month, None))
        );
    }

    #[test]
    fn the_file_time_is_kept_when_it_fits() {
        let ts = |s: &str| at(s).and_utc().timestamp();
        let path = Path::new("/photos/2019-03/DSCF0001.JPG");
        let root = Some(Path::new("/photos"));
        // Copied in 2024: the folder's month, at the file's time of day.
        assert_eq!(
            taken_at(path, root, ts("2024-06-02 09:15:00")),
            Some(ts("2019-03-01 09:15:00"))
        );
        // Taken in that month: the file's own time is better.
        assert_eq!(
            taken_at(path, root, ts("2019-03-20 18:00:00")),
            Some(ts("2019-03-20 18:00:00"))
        );
        // A name to the second wins over the file's time.
        assert_eq!(
            taken_at(
                Path::new("/photos/IMG_20230715_143012.jpg"),
                root,
                ts("2024-01-01 00:00:00")
            ),
            Some(ts("2023-07-15 14:30:12"))
        );
        assert_eq!(taken_at(Path::new("/photos/DSCF0001.JPG"), root, 0), None);
        // A holiday: its own day, unless the file's time is in its month.
        let christmas = Path::new("/photos/Christmas 2019/DSCF0001.JPG");
        assert_eq!(
            taken_at(christmas, root, ts("2024-06-02 09:15:00")),
            Some(ts("2019-12-25 09:15:00"))
        );
        assert_eq!(
            taken_at(christmas, root, ts("2019-12-28 18:00:00")),
            Some(ts("2019-12-28 18:00:00"))
        );
    }
}
