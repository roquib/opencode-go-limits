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
    /// Authoritative opencode model id, taken from the console table's
    /// `data-model` attribute (e.g. "deepseek-v4.1-flash").
    /// Prefix with "opencode-go/" for `opencode -m`.
    #[serde(default)]
    slug: String,
    /// Estimated requests per 5 hours for the selected plan.
    /// `None` means Unlimited for that window.
    #[serde(
        default,
        deserialize_with = "de_opt_limit",
        serialize_with = "ser_opt_limit"
    )]
    requests_per_5h: Option<u64>,
    /// Monthly spend allowance for the selected plan, as published by the
    /// console (e.g. "$15"). `None` when the plan is unlimited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    monthly_limit: Option<String>,
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

/// Plans exposed by the console table. Kept as an allow-list because the plan
/// name is interpolated into a CSS selector.
const PLANS: [&str; 2] = ["go", "go-plus"];

/// Parse the console `Go` limits table.
///
/// The page renders one row per model, each carrying:
/// - `data-model="<slug>"` — authoritative model id,
/// - a `[data-layer="model-name"]` cell with the display name,
/// - a `td[data-allowance-type="requests"]` cell with one
///   `[data-plan="go"|"go-plus"] [data-layer="current-value"]` per plan,
/// - a `td[data-allowance-type="usage"]` cell with the monthly $ limit
///   per plan.
///
/// Only the requested plan's values are recorded.
fn scrape_plan_value(row: &scraper::ElementRef, cell_sel: &Selector, plan: &str) -> Option<String> {
    let cell = row.select(cell_sel).next()?;
    let value_sel = Selector::parse(&format!(
        "[data-plan=\"{plan}\"] [data-layer=\"current-value\"]"
    ))
    .ok()?;
    let el = cell.select(&value_sel).next()?;
    Some(cell_text(&el))
}

fn scrape_usage_limits(plan: &str) -> Result<Vec<ModelUsage>> {
    // The docs page no longer publishes the request table; it points at the
    // console Go page instead (the `#limits` anchor is client-side only).
    let url = "https://opencode.ai/console/go";
    let response = reqwest::blocking::get(url).context("Failed to fetch page")?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("{url} returned HTTP {status} (the console may require a login)");
    }

    let html = response.text().context("Failed to read response")?;
    let document = Html::parse_document(&html);

    let row_selector = Selector::parse("tr[data-model]").expect("Invalid row selector");
    let name_selector =
        Selector::parse("[data-layer=\"model-name\"]").expect("Invalid name selector");
    let requests_cell_selector = Selector::parse("td[data-allowance-type=\"requests\"]")
        .expect("Invalid requests cell selector");
    let usage_cell_selector =
        Selector::parse("td[data-allowance-type=\"usage\"]").expect("Invalid usage cell selector");

    let mut models = Vec::new();

    for row in document.select(&row_selector) {
        let slug = row
            .value()
            .attr("data-model")
            .unwrap_or_default()
            .trim()
            .to_string();
        if slug.is_empty() {
            continue;
        }

        // Display name: prefer the title attribute (full text), fall back to
        // the cell's text, then to the slug.
        let name = row
            .select(&name_selector)
            .next()
            .map(|el| {
                el.value()
                    .attr("title")
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| cell_text(&el))
            })
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| slug.clone());

        let requests = scrape_plan_value(&row, &requests_cell_selector, plan);
        let Some(requests) = requests else {
            // Row without this plan's value (e.g. plan-specific model).
            continue;
        };
        let requests_per_5h = parse_limit(&requests)?;

        let monthly_limit = scrape_plan_value(&row, &usage_cell_selector, plan)
            .filter(|v| !v.is_empty())
            .filter(|v| !parse_is_unlimited(v));

        models.push(ModelUsage {
            name,
            slug,
            requests_per_5h,
            monthly_limit,
        });
    }

    if models.is_empty() {
        anyhow::bail!(
            "No models found on {url} for plan \"{plan}\" — the page layout may have changed"
        );
    }

    // Unlimited (None) ranks highest so it sorts first.
    let key = |m: &ModelUsage| m.requests_per_5h.unwrap_or(u64::MAX);
    models.sort_by_key(|b| std::cmp::Reverse(key(b)));
    Ok(models)
}

/// True when a published limit cell means "no limit".
fn parse_is_unlimited(s: &str) -> bool {
    let lowered = s.trim().to_lowercase();
    lowered.contains("unlimit") || lowered.contains('∞') || lowered.contains("infinity")
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

fn usage() -> &'static str {
    "opencode-go-limits [--once] [--plan <go|go-plus>]

  --once            scrape once and exit (default: loop hourly)
  --plan <name>     which console plan to record (default: go)
  -h, --help        show this help"
}

fn parse_args() -> (bool, String) {
    let mut run_once = false;
    let mut plan = "go".to_string();
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--once" => run_once = true,
            "--plan" => match args.next() {
                Some(value) if PLANS.contains(&value.as_str()) => plan = value,
                Some(value) => {
                    eprintln!(
                        "❌ unknown plan \"{value}\" (expected one of: {})",
                        PLANS.join(", ")
                    );
                    std::process::exit(2);
                }
                None => {
                    eprintln!("❌ --plan needs a value (one of: {})", PLANS.join(", "));
                    std::process::exit(2);
                }
            },
            "-h" | "--help" => {
                println!("{}", usage());
                std::process::exit(0);
            }
            other => {
                eprintln!("❌ unknown argument: {other}\n\n{}", usage());
                std::process::exit(2);
            }
        }
    }

    (run_once, plan)
}

fn main() -> Result<()> {
    let (run_once, plan) = parse_args();

    loop {
        println!("🔍 Scraping OpenCode AI usage limits (plan: {plan})...");
        match scrape_usage_limits(&plan) {
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
            monthly_limit: None,
        };
        let toml_str = toml::to_string_pretty(&m).unwrap();
        assert!(toml_str.contains("\"Unlimited\"") || toml_str.contains("Unlimited"));
        // Unknown/unlimited monthly limit is omitted entirely.
        assert!(!toml_str.contains("monthly_limit"));
        let back: ModelUsage = toml::from_str(&toml_str).unwrap();
        assert_eq!(back.requests_per_5h, None);
        assert_eq!(back.monthly_limit, None);
    }

    #[test]
    fn legacy_numeric_toml_still_parses() {
        // Old snapshots also carry requests_per_week/month; unknown fields
        // must be ignored rather than failing the whole file.
        let back: ModelUsage = toml::from_str(
            "name = \"X\"\nslug = \"x\"\nrequests_per_5h = 6500\nrequests_per_week = 100\nrequests_per_month = 1000\n",
        )
        .unwrap();
        assert_eq!(back.requests_per_5h, Some(6_500));
        assert_eq!(back.monthly_limit, None);
    }

    #[test]
    fn monthly_limit_is_kept_when_published() {
        let m = ModelUsage {
            name: "X".into(),
            slug: "x".into(),
            requests_per_5h: Some(98),
            monthly_limit: Some("$15".into()),
        };
        let toml_str = toml::to_string_pretty(&m).unwrap();
        assert!(toml_str.contains("monthly_limit = \"$15\""));
        let back: ModelUsage = toml::from_str(&toml_str).unwrap();
        assert_eq!(back.monthly_limit.as_deref(), Some("$15"));
    }

    /// The console layout this scraper targets: one row per model with
    /// per-plan request/usage cells keyed by `data-plan`.
    const CONSOLE_HTML: &str = r#"
<table>
  <tbody>
    <tr data-model="kimi-k3">
      <th><span data-layer="model-name" title="Kimi K3">Kimi K3</span></th>
      <td data-allowance-type="requests">
        <div data-layer="requests">
          <span data-plan="go"><span data-layer="current-value">98</span></span>
          <span data-plan="go-plus"><span data-layer="current-value">392</span></span>
        </div>
      </td>
      <td data-allowance-type="usage">
        <div data-layer="usage">
          <span data-plan="go"><span data-layer="current-value">$15</span></span>
          <span data-plan="go-plus"><span data-layer="current-value">$60</span></span>
        </div>
      </td>
    </tr>
    <tr data-model="beam-free">
      <th><span data-layer="model-name" title="Beam Free">Beam Free</span></th>
      <td data-allowance-type="requests">
        <div data-layer="requests">
          <span data-plan="go"><span data-layer="current-value">∞</span></span>
          <span data-plan="go-plus"><span data-layer="current-value">∞</span></span>
        </div>
      </td>
      <td data-allowance-type="usage">
        <div data-layer="usage">
          <span data-plan="go"><span data-layer="current-value">∞</span></span>
          <span data-plan="go-plus"><span data-layer="current-value">∞</span></span>
        </div>
      </td>
    </tr>
  </tbody>
</table>
"#;

    #[test]
    fn parses_console_rows_for_go_plan() {
        let doc = Html::parse_document(CONSOLE_HTML);
        let row_selector = Selector::parse("tr[data-model]").unwrap();
        let name_selector = Selector::parse("[data-layer=\"model-name\"]").unwrap();
        let requests_cell = Selector::parse("td[data-allowance-type=\"requests\"]").unwrap();
        let usage_cell = Selector::parse("td[data-allowance-type=\"usage\"]").unwrap();

        let rows: Vec<_> = doc.select(&row_selector).collect();
        assert_eq!(rows.len(), 2);

        let kimi = &rows[0];
        assert_eq!(kimi.value().attr("data-model"), Some("kimi-k3"));
        assert_eq!(
            kimi.select(&name_selector)
                .next()
                .unwrap()
                .value()
                .attr("title"),
            Some("Kimi K3")
        );
        assert_eq!(
            scrape_plan_value(kimi, &requests_cell, "go").as_deref(),
            Some("98")
        );
        assert_eq!(
            scrape_plan_value(kimi, &requests_cell, "go-plus").as_deref(),
            Some("392")
        );
        assert_eq!(
            scrape_plan_value(kimi, &usage_cell, "go").as_deref(),
            Some("$15")
        );

        let beam = &rows[1];
        assert_eq!(
            parse_limit(&scrape_plan_value(beam, &requests_cell, "go").unwrap()).unwrap(),
            None
        );
        assert!(parse_is_unlimited(
            &scrape_plan_value(beam, &usage_cell, "go").unwrap()
        ));
    }
}
