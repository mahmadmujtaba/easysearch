//! Emits `BUILD_DATE` (a `YYYY-MM-DD` string) for the window title and the
//! About dialog, so every build is identifiable.
//!
//! The date is computed here rather than read from the environment, and without
//! a date crate: days since the Unix epoch are converted to a civil date with
//! Howard Hinnant's algorithm. It is the build machine's UTC date.

fn main() {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
    println!("cargo:rustc-env=BUILD_DATE={year:04}-{month:02}-{day:02}");
    // Only rebuild when the script itself changes; a date is not worth a rebuild.
    println!("cargo:rerun-if-changed=build.rs");
}

/// Days since 1970-01-01 → `(year, month, day)`, proleptic Gregorian.
/// <https://howardhinnant.github.io/date_algorithms.html#civil_from_days>
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}
