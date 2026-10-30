//! When a photo was taken, from where it's kept, for photos that don't say
//! themselves (scans, screenshots, files that lost their EXIF on the way
//! through a messaging app): a date in the file's name, or in the names of
//! the folders it's in, nearest first ("2019/03 - Paris", "2019-03-15
//! Birthday", "July 2019", "2019").

use std::path::Path;

use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime};

/// How much of a date a name gave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precision {
    /// To the second ("IMG_20230715_143012").
    Exact,
    Day,
    Month,
    Year,
}

/// A date read from a name: its start (naive local time), and how precise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dated {
    pub start: NaiveDateTime,
    pub precision: Precision,
}

impl Dated {
    fn day(date: NaiveDate) -> Self {
        Dated {
            start: date.and_time(NaiveTime::MIN),
            precision: Precision::Day,
        }
    }

    /// The first moment after the period it names.
    fn end(&self) -> NaiveDateTime {
        let d = self.start.date();
        let next = match self.precision {
            Precision::Exact => return self.start + chrono::Duration::seconds(1),
            Precision::Day => d.succ_opt(),
            Precision::Month => {
                let (y, m) = if d.month() == 12 {
                    (d.year() + 1, 1)
                } else {
                    (d.year(), d.month() + 1)
                };
                NaiveDate::from_ymd_opt(y, m, 1)
            }
            Precision::Year => NaiveDate::from_ymd_opt(d.year() + 1, 1, 1),
        };
        next.map_or(self.start, |n| n.and_time(NaiveTime::MIN))
    }
}

/// When a photo at `path` was taken, going by its name and then its
/// folders' (up to `root`, the source, whose own name counts too), for
/// photos with no date of their own. `file_time` (the file's modified time,
/// seconds since 1970) is kept when it falls in the period a folder names,
/// as it's more precise; otherwise the period's start is used, at the file's
/// time of day so photos keep an order within it. `None` if no name says.
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
    Some(dated.start.date().and_time(time).and_utc().timestamp())
}

/// The date a path's names give: the file's own name first, then its
/// folders from the nearest up. A folder naming only a month ("03",
/// "03 March", "March"), or a month and day ("10-21"), takes its year from
/// a folder above it.
pub fn from_path(path: &Path, root: Option<&Path>) -> Option<Dated> {
    if let Some(d) = path
        .file_stem()
        .and_then(|s| s.to_str())
        .and_then(file_name)
    {
        return Some(d);
    }
    let mut month_only: Option<(u32, Option<u32>)> = None;
    let mut dir = path.parent();
    while let Some(d) = dir {
        let Some(name) = d.file_name().and_then(|s| s.to_str()) else {
            break;
        };
        if let Some(found) = folder_name(name) {
            return Some(match (month_only, found.precision) {
                // "2019/03": the year above, the month below.
                (Some((m, None)), Precision::Year) => {
                    NaiveDate::from_ymd_opt(found.start.year(), m, 1).map_or(found, |date| Dated {
                        start: date.and_time(NaiveTime::MIN),
                        precision: Precision::Month,
                    })
                }
                // "2026/10-21".
                (Some((m, Some(d))), Precision::Year) => {
                    NaiveDate::from_ymd_opt(found.start.year(), m, d).map_or(found, Dated::day)
                }
                _ => found,
            });
        }
        if month_only.is_none() {
            month_only = month_name(name);
        }
        if root.is_some_and(|r| d == r) {
            break;
        }
        dir = d.parent();
    }
    None
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

fn plausible_year(y: i32) -> bool {
    // Photography's past, up to next year (a camera clock a little ahead).
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let this_year = chrono::DateTime::from_timestamp(now, 0).map_or(2100, |t| t.year());
    (1826..=this_year + 1).contains(&y)
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
/// " 14.30.12", "-143012345").
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
            },
            None => Dated::day(date),
        });
    }
    None
}

/// A file's name: only a full date counts ("DSCF0001" and "IMG_2023" are
/// numbers, not dates).
fn file_name(stem: &str) -> Option<Dated> {
    full_date(stem)
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

/// A month named in words: "July", "Jul", "jul.".
fn month_word(s: &str) -> Option<u32> {
    let mut found = None;
    for word in s
        .split(|c: char| !c.is_alphabetic())
        .filter(|w| w.len() >= 3)
    {
        let w = word.to_lowercase();
        if let Some(i) = MONTHS
            .iter()
            .position(|m| *m == w || (w.len() == 3 && m.starts_with(&w)) || (w == "sept"))
        {
            // Two months in one name ("June-July") say no single one.
            if found.is_some() {
                return None;
            }
            found = Some(if w == "sept" { 9 } else { i as u32 + 1 });
        }
    }
    found
}

/// A folder's name: a full date, a year and month ("2023-07", "2023_07",
/// "July 2023", "2023 July"), or a year alone ("2023", "2023 Holidays").
fn folder_name(name: &str) -> Option<Dated> {
    if let Some(d) = full_date(name) {
        return Some(match d.precision {
            Precision::Exact => Dated::day(d.start.date()),
            _ => d,
        });
    }
    let nums = numbers(name);
    let years: Vec<(usize, i32)> = nums
        .iter()
        // A year stands alone: "1990s" is a decade.
        .filter(|(at, n)| n.len() == 4 && !name[at + 4..].starts_with(|c: char| c.is_alphabetic()))
        .filter_map(|(at, n)| Some((*at, n.parse().ok()?)))
        .filter(|(_, y)| plausible_year(*y))
        .collect();
    // "2019-2020" or "1990s 2000s": a span, not a date.
    let [(at, year)] = years[..] else {
        return None;
    };
    let month = |m: u32| {
        NaiveDate::from_ymd_opt(year, m, 1).map(|d| Dated {
            start: d.and_time(NaiveTime::MIN),
            precision: Precision::Month,
        })
    };
    // A month number right after the year: "2023-07", "2023_07 Paris".
    let after = &name[at + 4..];
    if let Some(m) = after
        .strip_prefix(['-', '_', '.', ' '])
        .and_then(|r| {
            let digits: String = r.chars().take_while(char::is_ascii_digit).collect();
            (digits.len() <= 2 && !digits.is_empty()).then_some(digits)
        })
        .and_then(|d| d.parse::<u32>().ok())
        && let Some(d) = month(m)
    {
        return Some(d);
    }
    if let Some(m) = month_word(name) {
        return month(m);
    }
    // Other numbers beside the year ("2023 12 photos") make it unclear only
    // when they could be a month; a year on its own stands.
    NaiveDate::from_ymd_opt(year, 1, 1).map(|d| Dated {
        start: d.and_time(NaiveTime::MIN),
        precision: Precision::Year,
    })
}

/// A folder naming a month and no year: "03", "03 March", "3 - March",
/// "March"; or a month and day: "10-21", "10_21 Party".
fn month_name(name: &str) -> Option<(u32, Option<u32>)> {
    let nums = numbers(name);
    match nums[..] {
        [] => month_word(name).map(|m| (m, None)),
        [(0, n)] if n.len() <= 2 => {
            let m: u32 = n.parse().ok()?;
            let word = month_word(name);
            ((1..=12).contains(&m) && word.is_none_or(|w| w == m)).then_some((m, None))
        }
        [(0, m), (at, d)]
            if m.len() <= 2
                && d.len() <= 2
                && at == m.len() + 1
                && name[m.len()..].starts_with(['-', '_', '.']) =>
        {
            let (m, d): (u32, u32) = (m.parse().ok()?, d.parse().ok()?);
            NaiveDate::from_ymd_opt(2000, m, d).map(|_| (m, Some(d)))
        }
        _ => None,
    }
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
    }
}
