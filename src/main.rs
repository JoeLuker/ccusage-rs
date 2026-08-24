//! ccusage-rs — Claude Code usage & cost reports from local JSONL
//! transcripts. A from-scratch Rust sibling of the TypeScript `ccusage`
//! (github.com/ccusage/ccusage): same data source and report shapes,
//! static offline pricing, plus exact 5m/1h cache-write tiering.

mod parse;
mod pricing;

use chrono::{DateTime, Datelike, Local, NaiveDate, Utc};
use clap::{Parser, Subcommand};
use std::collections::BTreeMap;

#[derive(Parser)]
#[command(name = "ccusage-rs", version, about = "Claude Code usage & cost reports from local transcripts")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Report only days on/after this date (YYYYMMDD or YYYY-MM-DD)
    #[arg(long, global = true)]
    since: Option<String>,
    /// Report only days on/before this date (YYYYMMDD or YYYY-MM-DD)
    #[arg(long, global = true)]
    until: Option<String>,
    /// Emit JSON instead of a table
    #[arg(long, global = true)]
    json: bool,
    /// Add per-model rows under each period
    #[arg(long, global = true)]
    breakdown: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Usage aggregated per local calendar day (default)
    Daily,
    /// Usage aggregated per calendar month
    Monthly,
    /// Usage aggregated per session file
    Session,
}

#[derive(Default, Clone)]
struct Agg {
    input: u64,
    output: u64,
    cache_write: u64,
    cache_read: u64,
    cost: f64,
    unknown_model_tokens: u64,
    models: BTreeMap<String, Box<Agg>>,
    last_ts: Option<DateTime<Utc>>,
}

impl Agg {
    fn add(&mut self, r: &parse::Record, cost: Option<f64>, into_models: bool) {
        self.input += r.input;
        self.output += r.output;
        self.cache_write += r.cache_write;
        self.cache_read += r.cache_read;
        match cost {
            Some(c) => self.cost += c,
            None => {
                self.unknown_model_tokens += r.input + r.output + r.cache_write + r.cache_read;
            }
        }
        if self.last_ts.is_none_or(|t| r.ts > t) {
            self.last_ts = Some(r.ts);
        }
        if into_models {
            self.models
                .entry(short_model(&r.model))
                .or_default()
                .add(r, cost, false);
        }
    }
    fn total_tokens(&self) -> u64 {
        self.input + self.output + self.cache_write + self.cache_read
    }
}

fn short_model(m: &str) -> String {
    m.strip_prefix("claude-").unwrap_or(m).to_string()
}

fn parse_date(s: &str) -> Option<NaiveDate> {
    let clean: String = s.chars().filter(char::is_ascii_digit).collect();
    if clean.len() != 8 {
        return None;
    }
    NaiveDate::from_ymd_opt(
        clean[0..4].parse().ok()?,
        clean[4..6].parse().ok()?,
        clean[6..8].parse().ok()?,
    )
}

fn main() {
    let cli = Cli::parse();
    let since = cli.since.as_deref().map(|s| parse_date(s).unwrap_or_else(|| die(&format!("invalid --since: {s}"))));
    let until = cli.until.as_deref().map(|s| parse_date(s).unwrap_or_else(|| die(&format!("invalid --until: {s}"))));

    let dirs = parse::config_dirs();
    if dirs.is_empty() {
        die("no Claude config dir found (set CLAUDE_CONFIG_DIR or create ~/.claude)");
    }
    let files = parse::discover(&dirs);
    let records = parse::parse_all(&files);

    let mut groups: BTreeMap<String, Agg> = BTreeMap::new();
    for r in &records {
        let local_date = r.ts.with_timezone(&Local).date_naive();
        if since.is_some_and(|s| local_date < s) || until.is_some_and(|u| local_date > u) {
            continue;
        }
        let key = match cli.command.as_ref().unwrap_or(&Command::Daily) {
            Command::Daily => local_date.format("%Y-%m-%d").to_string(),
            Command::Monthly => format!("{:04}-{:02}", local_date.year(), local_date.month()),
            Command::Session => format!("{}/{}", r.project, r.session),
        };
        // Cost mode "auto": trust a recorded costUSD when present, else
        // calculate from tokens x the static price table.
        let cost = r.cost_usd.or_else(|| {
            pricing::cost_usd(
                &r.model, r.input, r.output, r.cache_write, r.cache_write_5m, r.cache_write_1h,
                r.cache_read,
            )
        });
        groups.entry(key).or_default().add(r, cost, true);
    }

    if cli.json {
        print_json(&groups);
    } else {
        print_table(&groups, cli.breakdown, matches!(cli.command, Some(Command::Session)));
    }
}

fn die(msg: &str) -> ! {
    eprintln!("ccusage-rs: {msg}");
    std::process::exit(1)
}

fn print_json(groups: &BTreeMap<String, Agg>) {
    let arr: Vec<serde_json::Value> = groups
        .iter()
        .map(|(k, a)| {
            serde_json::json!({
                "period": k,
                "input_tokens": a.input,
                "output_tokens": a.output,
                "cache_creation_tokens": a.cache_write,
                "cache_read_tokens": a.cache_read,
                "total_tokens": a.total_tokens(),
                "cost_usd": (a.cost * 100.0).round() / 100.0,
                "unpriced_tokens": a.unknown_model_tokens,
                "models": a.models.iter().map(|(m, ma)| {
                    serde_json::json!({
                        "model": m,
                        "input_tokens": ma.input,
                        "output_tokens": ma.output,
                        "cache_creation_tokens": ma.cache_write,
                        "cache_read_tokens": ma.cache_read,
                        "cost_usd": (ma.cost * 100.0).round() / 100.0,
                    })
                }).collect::<Vec<_>>(),
            })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&arr).unwrap());
}

fn commas(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn print_table(groups: &BTreeMap<String, Agg>, breakdown: bool, by_session: bool) {
    // Session reports get wide keys; order those by recency instead of name.
    let mut rows: Vec<(&String, &Agg)> = groups.iter().collect();
    if by_session {
        rows.sort_by_key(|(_, a)| a.last_ts);
    }
    let key_header = if by_session { "Session" } else { "Period" };
    // Breakdown rows render as "  └ <model>" (4 display cells + name),
    // so the key column must fit those too or the row overflows and
    // shifts every later column.
    let breakdown_w = if breakdown {
        rows.iter()
            .flat_map(|(_, a)| a.models.keys())
            .map(|m| m.chars().count() + 4)
            .max()
            .unwrap_or(0)
    } else {
        0
    };
    let key_w = rows
        .iter()
        .map(|(k, _)| k.chars().count())
        .chain([key_header.len(), breakdown_w])
        .max()
        .unwrap_or(8)
        .min(52);

    println!(
        "{:<key_w$}  {:>13} {:>13} {:>15} {:>17} {:>15} {:>11}",
        key_header, "Input", "Output", "Cache Create", "Cache Read", "Total", "Cost (USD)"
    );
    let width = key_w + 2 + 13 + 1 + 13 + 1 + 15 + 1 + 17 + 1 + 15 + 1 + 11;
    println!("{}", "-".repeat(width));

    let mut total = Agg::default();
    let mut unpriced = 0u64;
    for (key, a) in &rows {
        let mut key = (*key).clone();
        if key.len() > key_w {
            key.truncate(key_w - 1);
            key.push('…');
        }
        println!(
            "{:<key_w$}  {:>13} {:>13} {:>15} {:>17} {:>15} {:>11}",
            key,
            commas(a.input),
            commas(a.output),
            commas(a.cache_write),
            commas(a.cache_read),
            commas(a.total_tokens()),
            format!("${:.2}", a.cost),
        );
        if breakdown {
            for (m, ma) in &a.models {
                println!(
                    "{:<key_w$}  {:>13} {:>13} {:>15} {:>17} {:>15} {:>11}",
                    format!("  └ {m}"),
                    commas(ma.input),
                    commas(ma.output),
                    commas(ma.cache_write),
                    commas(ma.cache_read),
                    commas(ma.total_tokens()),
                    format!("${:.2}", ma.cost),
                );
            }
        }
        total.input += a.input;
        total.output += a.output;
        total.cache_write += a.cache_write;
        total.cache_read += a.cache_read;
        total.cost += a.cost;
        unpriced += a.unknown_model_tokens;
    }
    println!("{}", "-".repeat(width));
    println!(
        "{:<key_w$}  {:>13} {:>13} {:>15} {:>17} {:>15} {:>11}",
        "Total",
        commas(total.input),
        commas(total.output),
        commas(total.cache_write),
        commas(total.cache_read),
        commas(total.total_tokens()),
        format!("${:.2}", total.cost),
    );
    if unpriced > 0 {
        eprintln!(
            "note: {} tokens from models missing in the price table were counted but not priced",
            commas(unpriced)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commas_format() {
        assert_eq!(commas(0), "0");
        assert_eq!(commas(999), "999");
        assert_eq!(commas(1000), "1,000");
        assert_eq!(commas(120962900), "120,962,900");
    }

    #[test]
    fn date_parsing_both_forms() {
        assert_eq!(parse_date("20260822"), NaiveDate::from_ymd_opt(2026, 8, 22));
        assert_eq!(parse_date("2026-08-22"), NaiveDate::from_ymd_opt(2026, 8, 22));
        assert_eq!(parse_date("nope"), None);
    }
}
