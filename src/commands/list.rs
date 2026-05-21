use anyhow::Result;
use berth::config::Config;
use berth::lifecycle_state;
use colored::Colorize;
use std::time::{SystemTime, UNIX_EPOCH};

pub async fn run(long: bool, absolute_time: bool) -> Result<()> {
    let config = Config::load()?;

    if config.workspaces.is_empty() {
        println!("(no workspaces configured)");
        return Ok(());
    }

    if long {
        return long_form(&config).await;
    }

    let state = lifecycle_state::load();
    let now = current_epoch_seconds();

    let mut rows: Vec<Row> = config
        .workspaces
        .iter()
        .map(|(name, ws)| {
            let host = config.resolved_remote(name, ws);
            let last_used = last_used_seconds(&state, name, host.as_deref());
            Row {
                name: name.clone(),
                ws_type: if host.is_some() { "remote" } else { "local" },
                last_used: format_last_used(last_used, now, absolute_time),
                last_used_epoch: last_used,
                path: ws.path.clone(),
            }
        })
        .collect();

    // Most recently used first; never-used at the bottom; ties broken by name.
    rows.sort_by(|a, b| {
        b.last_used_epoch
            .cmp(&a.last_used_epoch)
            .then_with(|| a.name.cmp(&b.name))
    });

    let name_w = col_width("NAME", rows.iter().map(|r| r.name.as_str()));
    let type_w = col_width("TYPE", rows.iter().map(|r| r.ws_type));
    let used_w = col_width("LAST USED", rows.iter().map(|r| r.last_used.as_str()));

    println!(
        "{:<nw$}  {:<tw$}  {:<uw$}  {}",
        "NAME".bold(),
        "TYPE".bold(),
        "LAST USED".bold(),
        "PATH".bold(),
        nw = name_w,
        tw = type_w,
        uw = used_w
    );
    for row in &rows {
        let type_colored = match row.ws_type {
            "remote" => row.ws_type.cyan(),
            _ => row.ws_type.normal(),
        };
        let used_colored = if row.last_used_epoch.is_some() {
            row.last_used.as_str().normal()
        } else {
            row.last_used.as_str().dimmed()
        };
        // Pad against the uncolored values so ANSI escapes don't
        // shove the columns out of line.
        let name_pad = " ".repeat(name_w.saturating_sub(row.name.chars().count()));
        let type_pad = " ".repeat(type_w.saturating_sub(row.ws_type.chars().count()));
        let used_pad = " ".repeat(used_w.saturating_sub(row.last_used.chars().count()));
        println!(
            "{}{}  {}{}  {}{}  {}",
            row.name.bold(),
            name_pad,
            type_colored,
            type_pad,
            used_colored,
            used_pad,
            row.path.dimmed(),
        );
    }

    Ok(())
}

struct Row {
    name: String,
    ws_type: &'static str,
    last_used: String,
    last_used_epoch: Option<u64>,
    path: String,
}

fn col_width<'a>(header: &str, values: impl Iterator<Item = &'a str>) -> usize {
    values
        .map(|s| s.chars().count())
        .max()
        .unwrap_or(0)
        .max(header.chars().count())
}

fn current_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Look up the last_active timestamp for a workspace. The lifecycle state
/// is keyed by `<workspace>` for local entries and `<workspace>@<host>` for
/// remote; we check the remote key first when a host is known, then fall
/// back to the bare key (covers the case where a workspace was re-pointed
/// at a remote after its last local touch).
fn last_used_seconds(
    state: &lifecycle_state::State,
    workspace: &str,
    host: Option<&str>,
) -> Option<u64> {
    if let Some(host) = host {
        let keyed = format!("{}@{}", workspace, host);
        if let Some(env) = state.environments.get(&keyed) {
            return Some(env.last_active_epoch_seconds);
        }
    }
    state
        .environments
        .get(workspace)
        .map(|env| env.last_active_epoch_seconds)
}

fn format_last_used(epoch: Option<u64>, now: u64, absolute: bool) -> String {
    match epoch {
        None => "never".to_string(),
        Some(t) if absolute => format_utc(t),
        Some(t) => format_relative(now.saturating_sub(t)),
    }
}

fn format_relative(elapsed_seconds: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    const WEEK: u64 = 7 * DAY;
    const MONTH: u64 = 30 * DAY;
    const YEAR: u64 = 365 * DAY;

    let (n, unit) = if elapsed_seconds < MINUTE {
        return "just now".to_string();
    } else if elapsed_seconds < HOUR {
        (elapsed_seconds / MINUTE, "m")
    } else if elapsed_seconds < DAY {
        (elapsed_seconds / HOUR, "h")
    } else if elapsed_seconds < WEEK {
        (elapsed_seconds / DAY, "d")
    } else if elapsed_seconds < MONTH {
        (elapsed_seconds / WEEK, "w")
    } else if elapsed_seconds < YEAR {
        (elapsed_seconds / MONTH, "mo")
    } else {
        (elapsed_seconds / YEAR, "y")
    };

    format!("{n}{unit} ago")
}

/// Cheap UTC formatter (YYYY-MM-DD HH:MM:SS UTC) without pulling in chrono.
/// Good enough for human eyeballing; not a replacement for a real time lib.
fn format_utc(epoch_seconds: u64) -> String {
    let secs_per_day = 86_400u64;
    let mut days = epoch_seconds / secs_per_day;
    let rem = epoch_seconds % secs_per_day;
    let hour = rem / 3600;
    let minute = (rem % 3600) / 60;
    let second = rem % 60;

    // Days since 1970-01-01 → Y/M/D via the civil-from-days routine
    // (Howard Hinnant). Cheap and correct for the Gregorian calendar.
    days += 719_468;
    let era = days / 146_097;
    let doe = days - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{y:04}-{m:02}-{d:02} {hour:02}:{minute:02}:{second:02} UTC")
}

async fn long_form(config: &Config) -> Result<()> {
    let mut names: Vec<&String> = config.workspaces.keys().collect();
    names.sort();
    for name in names {
        crate::commands::project::show(name.clone()).await?;
        println!();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_under_a_minute_is_just_now() {
        assert_eq!(format_relative(0), "just now");
        assert_eq!(format_relative(59), "just now");
    }

    #[test]
    fn relative_minutes_hours_days() {
        assert_eq!(format_relative(60), "1m ago");
        assert_eq!(format_relative(60 * 59), "59m ago");
        assert_eq!(format_relative(3600), "1h ago");
        assert_eq!(format_relative(86_400), "1d ago");
        assert_eq!(format_relative(86_400 * 3), "3d ago");
    }

    #[test]
    fn relative_weeks_months_years() {
        assert_eq!(format_relative(86_400 * 7), "1w ago");
        assert_eq!(format_relative(86_400 * 14), "2w ago");
        assert_eq!(format_relative(86_400 * 30), "1mo ago");
        assert_eq!(format_relative(86_400 * 365), "1y ago");
    }

    #[test]
    fn absolute_formats_epoch_zero_as_unix_epoch() {
        assert_eq!(format_utc(0), "1970-01-01 00:00:00 UTC");
    }

    #[test]
    fn absolute_formats_known_date() {
        // 2024-01-01 00:00:00 UTC = 1_704_067_200
        assert_eq!(format_utc(1_704_067_200), "2024-01-01 00:00:00 UTC");
    }

    #[test]
    fn col_width_respects_header_and_data() {
        let widths = ["a", "bb", "ccc"];
        assert_eq!(col_width("HEADER", widths.iter().copied()), 6);
        let widths = ["aaa", "bbbb", "ccccc"];
        assert_eq!(col_width("X", widths.iter().copied()), 5);
    }
}
