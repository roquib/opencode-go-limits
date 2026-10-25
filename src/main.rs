use anyhow::{Context, Result};
use chrono::{Local, NaiveDate};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Clone)]
struct ModelUsage {
    name: String,
    /// opencode model id from the Endpoints table (e.g. "deepseek-v4-flash").
    /// Prefix with "opencode-go/" for `opencode -m`. Falls back to a slugified
    /// name when the Endpoints table has no matching row.
    #[serde(default)]
    slug: String,
    /// `None` means Unlimited for that window.
    #[serde(
        default,
        deserialize_with = "de_opt_limit",
        serialize_with = "ser_opt_limit"
    )]
    requests_per_5h: Option<u64>,
    #[serde(
        default,
        deserialize_with = "de_opt_limit",
        serialize_with = "ser_opt_limit"
    )]
    requests_per_week: Option<u64>,
    #[serde(
        default,
        deserialize_with = "de_opt_limit",
        serialize_with = "ser_opt_limit"
    )]
    requests_per_month: Option<u64>,
}

/// Serialize `Some(n)` as a TOML integer, `None` (unlimited) as `"Unlimited"`.
fn ser_opt_limit<S: serde::Serializer>(
    v: &Option<u64>,
    s: S,
) -> std::result::Result<S::Ok, S::Error> {
    match v {
        Some(n) => s.serialize_u64(*n),
        None => s.serialize_str("Unlimited"),
    }
}

/// Accept a TOML integer, a numeric string, or "Unlimited" (any case).
/// Missing field (via `#[serde(default)]`) also maps to `None`.
fn de_opt_limit<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<u64>, D::Error> {
    use serde::de::{self, Visitor};

    struct OptLimitVisitor;

    impl Visitor<'_> for OptLimitVisitor {
        type Value = Option<u64>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("an integer, a numeric string, or \"Unlimited\"")
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Self::Value, E> {
            Ok(Some(v))
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Self::Value, E> {
            if v < 0 {
                Err(E::custom(format!("negative limit is not valid: {v}")))
            } else {
                Ok(Some(v as u64))
            }
        }

        fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Self::Value, E> {
            if v.trim().to_lowercase().contains("unlimit") {
                return Ok(None);
            }
            // Allow numeric strings (e.g. legacy hand-edited TOML).
            let digits: String = v.chars().filter(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                return Err(E::custom(format!("No number found in: {v}")));
            }
            digits
                .parse::<u64>()
                .map(Some)
                .map_err(|_| E::custom(format!("Failed to parse number: {v}")))
        }

        fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Self::Value, E> {
            self.visit_str(&v)
        }
    }

    d.deserialize_any(OptLimitVisitor)
}

/// "Muse Spark 1.3 Contributor" -> "muse-spark-1.3-contributor" (fallback only).
/// Keeps dots since authoritative ids contain them (qwen3.8-flash,
/// deepseek-v4.1-flash); collapses any other run of non-alphanumerics
/// into a single hyphen.
fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_dash = true; // trim leading separators
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() || c == '.' {
            out.push(c);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Normalize a display name for cross-table lookup: the Endpoints table
/// uses "MiMo V2.5" while the Estimated-requests table uses "MiMo-V2.5",
/// and promo rows append suffixes ("DeepSeek V4.1 Flash4x · Ends Sep 20").
/// Lowercase, drop promo suffix after '·', turn every non-alphanumeric
/// run into one space, collapse whitespace.
fn normalize_name(name: &str) -> String {
    let base = name.split('·').next().unwrap_or(name);
    base.to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Look up the authoritative slug: exact normalized match first, then a
/// prefix match so "deepseek v4 1 flash 4x" still resolves to the
/// "deepseek v4 1 flash" endpoint row.
fn lookup_slug(slugs: &BTreeMap<String, String>, name: &str) -> Option<String> {
    let norm = normalize_name(name);
    if let Some(id) = slugs.get(&norm) {
        return Some(id.clone());
    }
    slugs
        .iter()
        .find(|(k, _)| norm.starts_with(k.as_str()))
        .map(|(_, v)| v.clone())
}

#[derive(Debug, Serialize, Deserialize)]
struct UsageData {
    models: BTreeMap<String, Vec<ModelUsage>>,
}

fn get_output_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Failed to get home directory")?;
    let config_dir = home.join(".config").join("usages_limit");
    fs::create_dir_all(&config_dir).context("Failed to create config directory")?;
    Ok(config_dir.join("models.toml"))
}

fn cell_text(cell: &scraper::ElementRef) -> String {
    // Join text nodes with spaces: promo cells like `6,500<strong>26,000</strong>`
    // would otherwise concatenate into "6,50026,000" with no separator.
    cell.text()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `None` = Unlimited for that window. Accepts "Unlimited" (any case),
/// the infinity symbol, promo cells with two figures (takes the max),
/// and plain numbers with commas.
fn parse_limit(s: &str) -> Result<Option<u64>> {
    let lowered = s.trim().to_lowercase();
    if lowered.contains("unlimit") || lowered.contains('∞') || lowered.contains("infinity") {
        return Ok(None);
    }
    // Promo cells hold two figures ("6,500 26,000" = base + 4x boost).
    // Commas are thousands separators, so split only on chars that are
    // neither digit nor comma, then take the max figure.
    let mut best: Option<u64> = None;
    for chunk in s.split(|c: char| !(c.is_ascii_digit() || c == ',')) {
        let digits: String = chunk.chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            continue;
        }
        let n: u64 = digits
            .parse()
            .context(format!("Failed to parse number: {}", s))?;
        best = Some(best.map_or(n, |b| b.max(n)));
    }
    best.map(Some).context(format!("No number found in: {s}"))
}

/// Parse the Endpoints table (columns: Model | Model ID | Endpoint | AI SDK
/// Package) into a map of normalized display name -> model id slug.
/// Keys are normalized so "MiMo V2.5" matches the "MiMo-V2.5" usage row.
fn scrape_endpoints(document: &Html) -> BTreeMap<String, String> {
    let table_selector = Selector::parse("table").expect("Invalid table selector");
    let row_selector = Selector::parse("tr").expect("Invalid row selector");
    let cell_selector = Selector::parse("td").expect("Invalid cell selector");
    let th_selector = Selector::parse("th").expect("Invalid th selector");

    let mut map = BTreeMap::new();

    for table in document.select(&table_selector) {
        let headers: Vec<String> = table
            .select(&th_selector)
            .map(|h| h.text().collect::<String>().trim().to_string())
            .collect();

        if headers.contains(&"Model".to_string()) && headers.contains(&"Model ID".to_string()) {
            for row in table.select(&row_selector) {
                let cells: Vec<_> = row.select(&cell_selector).collect();
                if cells.len() >= 2 {
                    let name = cells[0].text().collect::<String>().trim().to_string();
                    let id = cells[1].text().collect::<String>().trim().to_string();
                    if !name.is_empty() && !id.is_empty() && name != "Model" {
                        map.insert(normalize_name(&name), id);
                    }
                }
            }
        }
    }

    map
}

/// True when `headers` look like the Estimated-requests table:
/// `Model | Requests per 5 hours | Requests per week | Requests per month`.
/// Compared case-insensitively — the site uses "Requests per 5 hours".
fn is_requests_table(headers: &[String]) -> bool {
    headers.iter().any(|h| h == "Model")
        && headers
            .iter()
            .any(|h| h.to_lowercase().contains("requests per 5 hour"))
}

fn scrape_usage_limits() -> Result<Vec<ModelUsage>> {
    let url = "https://opencode.ai/docs/go/";
    let response = reqwest::blocking::get(url).context("Failed to fetch page")?;

    let html = response.text().context("Failed to read response")?;
    let document = Html::parse_document(&html);

    // The Endpoints table maps each model's display name to its "Model ID"
    // (the slug used by `opencode -m opencode-go/<id>`). Read it first so we can
    // attach the authoritative slug to each usage row.
    let slugs = scrape_endpoints(&document);

    let table_selector = Selector::parse("table").expect("Invalid table selector");
    let go_panel_table_selector =
        Selector::parse("#go-requests-panel-go table").expect("Invalid panel selector");
    let row_selector = Selector::parse("tr").expect("Invalid row selector");
    let cell_selector = Selector::parse("td").expect("Invalid cell selector");
    let th_selector = Selector::parse("th").expect("Invalid th selector");

    // The page now renders one requests table per plan tab (`go` and
    // `go-plus` with ~3x the limits). Only the `go` panel is ours — the
    // generic `table` selector is just a fallback for older page layouts.
    let panel_tables: Vec<_> = document.select(&go_panel_table_selector).collect();
    let use_panel = !panel_tables.is_empty();

    let mut models = Vec::new();

    // Collect the table elements first: `document.select()` borrows the
    // document, and panel scoping needs the borrow to end before the loop.
    let table_html: Vec<String> = if use_panel {
        panel_tables.iter().map(|t| t.html()).collect()
    } else {
        document.select(&table_selector).map(|t| t.html()).collect()
    };

    for html in &table_html {
        let table = Html::parse_fragment(html);
        let header_row = table.select(&th_selector).collect::<Vec<_>>();
        if header_row.len() >= 4 {
            let headers: Vec<String> = header_row
                .iter()
                .map(|h| h.text().collect::<String>().trim().to_string())
                .collect();

            if is_requests_table(&headers) {
                for row in table.select(&row_selector) {
                    let cells: Vec<_> = row.select(&cell_selector).collect();
                    if cells.len() >= 4 {
                        let name = cell_text(&cells[0]);
                        let requests_5h = parse_limit(&cell_text(&cells[1]))?;
                        let requests_week = parse_limit(&cell_text(&cells[2]))?;
                        let requests_month = parse_limit(&cell_text(&cells[3]))?;

                        if !name.is_empty() && name != "Model" {
                            let slug = lookup_slug(&slugs, &name).unwrap_or_else(|| slugify(&name));
                            models.push(ModelUsage {
                                name,
                                slug,
                                requests_per_5h: requests_5h,
                                requests_per_week: requests_week,
                                requests_per_month: requests_month,
                            });
                        }
                    }
                }
            }
        }
    }

    // Unlimited (None) ranks highest so it sorts first.
    let key = |m: &ModelUsage| m.requests_per_5h.unwrap_or(u64::MAX);
    models.sort_by_key(|b| std::cmp::Reverse(key(b)));
    Ok(models)
}

fn read_existing_data(path: &PathBuf) -> Result<UsageData> {
    if !path.exists() {
        return Ok(UsageData {
            models: BTreeMap::new(),
        });
    }
    let content = fs::read_to_string(path).context("Failed to read TOML")?;
    match toml::from_str(&content) {
        Ok(data) => Ok(data),
        Err(_) => Ok(UsageData {
            models: BTreeMap::new(),
        }),
    }
}

fn write_toml(models: Vec<ModelUsage>) -> Result<()> {
    let output_path = get_output_path()?;
    let mut data = read_existing_data(&output_path)?;

    let today = Local::now().format("%Y-%m-%d").to_string();
    data.models.insert(today, models);

    let cutoff = Local::now().date_naive() - chrono::Duration::days(3);
    data.models.retain(|date, _| {
        NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .map(|d| d >= cutoff)
            .unwrap_or(false)
    });

    let toml_content = toml::to_string_pretty(&data).context("Failed to serialize to TOML")?;
    fs::write(&output_path, &toml_content)
        .context(format!("Failed to write to {}", output_path.display()))?;

    println!(
        "✅ Updated usage limits at {} (keeping {} days)",
        Local::now().format("%H:%M:%S"),
        data.models.len()
    );
    Ok(())
}

use std::thread;
use std::time::Duration;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let run_once = args.contains(&"--once".to_string());

    loop {
        println!("🔍 Scraping OpenCode AI usage limits...");
        match scrape_usage_limits() {
            Ok(models) => {
                println!("📊 Found {} models", models.len());
                if let Err(e) = write_toml(models) {
                    eprintln!("❌ Failed to write TOML: {}", e);
                }
            }
            Err(e) => {
                eprintln!("❌ Scrape failed: {}", e);
            }
        }

        if run_once {
            break;
        }

        println!("⏰ Next update in 1 hour...");
        thread::sleep(Duration::from_secs(3600));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_promo_numbers() {
        assert_eq!(parse_limit("6,500").unwrap(), Some(6_500));
        assert_eq!(parse_limit("6,500 26,000").unwrap(), Some(26_000));
    }

    #[test]
    fn parses_unlimited_case_insensitively() {
        assert_eq!(parse_limit("Unlimited").unwrap(), None);
        assert_eq!(parse_limit("  unlimited  ").unwrap(), None);
        assert_eq!(parse_limit("UNLIMITED").unwrap(), None);
        assert_eq!(parse_limit("∞").unwrap(), None);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_limit("").is_err());
        assert!(parse_limit("N/A").is_err());
    }

    #[test]
    fn unlimited_serde_roundtrip() {
        let m = ModelUsage {
            name: "X".into(),
            slug: "x".into(),
            requests_per_5h: None,
            requests_per_week: Some(100),
            requests_per_month: None,
        };
        let toml_str = toml::to_string_pretty(&m).unwrap();
        assert!(toml_str.contains("\"Unlimited\"") || toml_str.contains("Unlimited"));
        let back: ModelUsage = toml::from_str(&toml_str).unwrap();
        assert_eq!(back.requests_per_5h, None);
        assert_eq!(back.requests_per_week, Some(100));
        assert_eq!(back.requests_per_month, None);
    }

    #[test]
    fn legacy_numeric_toml_still_parses() {
        let back: ModelUsage = toml::from_str(
            "name = \"X\"\nslug = \"x\"\nrequests_per_5h = 6500\nrequests_per_week = 100\nrequests_per_month = 1000\n",
        )
        .unwrap();
        assert_eq!(back.requests_per_5h, Some(6_500));
    }
}
