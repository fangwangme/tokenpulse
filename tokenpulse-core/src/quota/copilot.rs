use crate::auth::copilot::CopilotAuth;
use crate::provider::{QuotaFetcher, QuotaSnapshot, RateWindow};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Utc};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;
use tracing::debug;

const REQUEST_TIMEOUT_SECS: u64 = 20;
const COPILOT_API_URL: &str = "https://api.github.com/copilot_internal/user";
/// Compute the actual billing period in milliseconds based on the reset date.
/// Falls back to the current calendar-month span ending at next month start.
fn billing_period_ms(reset_date: &Option<String>) -> i64 {
    let Some(reset_str) = reset_date else {
        let reset_utc = next_month_reset_utc(Utc::now());
        return subtract_one_month(reset_utc)
            .map(|start| (reset_utc - start).num_milliseconds())
            .unwrap_or(30 * 24 * 60 * 60 * 1000);
    };

    // Paid tier: RFC3339 date, e.g. "2025-08-01T00:00:00Z"
    if let Ok(reset_dt) = DateTime::parse_from_rfc3339(reset_str) {
        let reset_utc = reset_dt.with_timezone(&Utc);
        // Billing period starts one calendar month before the reset date
        if let Some(start) = subtract_one_month(reset_utc) {
            return (reset_utc - start).num_milliseconds();
        }
    }

    // Free tier: plain date, e.g. "2025-08-01"
    if let Ok(nd) = NaiveDate::parse_from_str(reset_str, "%Y-%m-%d") {
        if let Some(dt) = nd.and_hms_opt(0, 0, 0) {
            let reset_utc = DateTime::from_naive_utc_and_offset(dt, Utc);
            if let Some(start) = subtract_one_month(reset_utc) {
                return (reset_utc - start).num_milliseconds();
            }
        }
    }

    let reset_utc = next_month_reset_utc(Utc::now());
    subtract_one_month(reset_utc)
        .map(|start| (reset_utc - start).num_milliseconds())
        .unwrap_or(30 * 24 * 60 * 60 * 1000)
}

/// Subtract one calendar month from a DateTime (e.g. Aug 15 → Jul 15).
fn subtract_one_month(dt: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let (year, month) = if dt.month() == 1 {
        (dt.year() - 1, 12)
    } else {
        (dt.year(), dt.month() - 1)
    };
    let day = dt.day().min(days_in_month(year, month));
    dt.with_day(day)?.with_month(month)?.with_year(year)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    NaiveDate::from_ymd_opt(
        if month == 12 { year + 1 } else { year },
        if month == 12 { 1 } else { month + 1 },
        1,
    )
    .and_then(|date| date.pred_opt())
    .map(|date| date.day())
    .unwrap_or(30)
}

fn next_month_reset_utc(now: DateTime<Utc>) -> DateTime<Utc> {
    let (year, month) = if now.month() == 12 {
        (now.year() + 1, 1)
    } else {
        (now.year(), now.month() + 1)
    };
    Utc.with_ymd_and_hms(year, month, 1, 0, 0, 0)
        .single()
        .unwrap_or(now)
}

fn parse_reset_date_utc(reset_date: &Option<String>) -> Option<DateTime<Utc>> {
    reset_date.as_ref().and_then(|value| {
        DateTime::parse_from_rfc3339(value)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
            .or_else(|| {
                NaiveDate::parse_from_str(value, "%Y-%m-%d")
                    .ok()
                    .and_then(|date| date.and_hms_opt(0, 0, 0))
                    .map(|dt| DateTime::from_naive_utc_and_offset(dt, Utc))
            })
    })
}

// --- Response types ---

#[derive(Debug, Deserialize)]
struct CopilotUserResponse {
    #[serde(default)]
    copilot_plan: Option<String>,

    // Paid tier fields
    #[serde(default)]
    quota_reset_date: Option<String>,
    #[serde(default)]
    quota_snapshots: Option<HashMap<String, QuotaSnapshotEntry>>,

    // Free tier fields
    #[serde(default)]
    limited_user_quotas: Option<HashMap<String, f64>>,
    #[serde(default)]
    monthly_quotas: Option<HashMap<String, f64>>,
    #[serde(default)]
    limited_user_reset_date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct QuotaSnapshotEntry {
    #[serde(default)]
    percent_remaining: Option<f64>,
    #[serde(default)]
    entitlement: Option<f64>,
    #[serde(rename = "remaining", default)]
    _remaining: Option<f64>,
}

// --- Fetcher ---

pub struct CopilotQuotaFetcher {
    client: Client,
    auth: CopilotAuth,
}

impl CopilotQuotaFetcher {
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            client,
            auth: CopilotAuth::new(),
        }
    }
}

impl Default for CopilotQuotaFetcher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl QuotaFetcher for CopilotQuotaFetcher {
    fn provider_name(&self) -> &str {
        "copilot"
    }

    fn provider_display_name(&self) -> &str {
        "GitHub Copilot"
    }

    async fn fetch_quota(&self) -> Result<QuotaSnapshot> {
        let candidates = self.auth.token_candidates();
        if candidates.is_empty() {
            return Err(anyhow!("GitHub Copilot credentials not found"));
        }

        debug!("Fetching Copilot quota");

        let mut saw_unauthorized = false;
        let mut last_error = None;

        for token in candidates {
            let response = self
                .client
                .get(COPILOT_API_URL)
                .header("Authorization", format!("token {}", token))
                .header("Accept", "application/json")
                .header("Editor-Version", "vscode/1.96.2")
                .header("Editor-Plugin-Version", "copilot-chat/0.26.7")
                .header("User-Agent", "GitHubCopilotChat/0.26.7")
                .header("X-Github-Api-Version", "2025-04-01")
                .send()
                .await?;

            debug!("Copilot quota response status: {}", response.status());

            if response.status() == reqwest::StatusCode::UNAUTHORIZED {
                saw_unauthorized = true;
                continue;
            }

            if !response.status().is_success() {
                let status = response.status();
                let text = response.text().await?;
                last_error = Some(anyhow!("Copilot API error {}: {}", status, text));
                break;
            }

            let response_text = response.text().await?;
            debug!(
                "Copilot quota response body length: {} bytes",
                response_text.len()
            );

            let data: CopilotUserResponse = match serde_json::from_str(&response_text) {
                Ok(d) => d,
                Err(e) => {
                    return Err(anyhow!(
                        "Failed to parse Copilot quota response: {}. First 200 chars: {}",
                        e,
                        &response_text[..response_text.len().min(200)]
                    ));
                }
            };
            debug!(
                "Copilot quota parsed: plan={:?}, paid_snapshots={}, free_quotas={}",
                data.copilot_plan.as_deref(),
                data.quota_snapshots
                    .as_ref()
                    .map(|entries| entries.len())
                    .unwrap_or(0),
                data.monthly_quotas
                    .as_ref()
                    .map(|entries| entries.len())
                    .unwrap_or(0)
            );

            let windows = if let Some(snapshots) = &data.quota_snapshots {
                self.parse_paid_tier(snapshots, &data.quota_reset_date)
            } else if let Some(remaining) = &data.limited_user_quotas {
                self.parse_free_tier(
                    remaining,
                    &data.monthly_quotas,
                    &data.limited_user_reset_date,
                )
            } else {
                Vec::new()
            };

            return Ok(QuotaSnapshot {
                provider: "copilot".to_string(),
                plan: data.copilot_plan,
                windows,
                credits: None,
                fetched_at: Utc::now(),
            });
        }

        if saw_unauthorized {
            return Err(anyhow!(
                "GitHub Copilot session expired. Please run `gh auth login` or update GITHUB_TOKEN."
            ));
        }

        if let Some(error) = last_error {
            return Err(error);
        }

        Err(anyhow!("GitHub Copilot credentials not found"))
    }
}

impl CopilotQuotaFetcher {
    fn parse_paid_tier(
        &self,
        snapshots: &HashMap<String, QuotaSnapshotEntry>,
        reset_date: &Option<String>,
    ) -> Vec<RateWindow> {
        let resets_at =
            parse_reset_date_utc(reset_date).or_else(|| Some(next_month_reset_utc(Utc::now())));

        let period_ms = billing_period_ms(reset_date);

        let mut windows: Vec<RateWindow> = snapshots
            .iter()
            .map(|(name, entry)| {
                let used_percent =
                    (100.0 - entry.percent_remaining.unwrap_or(100.0)).clamp(0.0, 100.0);
                let label = if let Some(ent) = entry.entitlement {
                    format!("{} ({})", pretty_label(name), ent as i64)
                } else {
                    pretty_label(name)
                };

                RateWindow {
                    label,
                    used_percent,
                    resets_at,
                    period_duration_ms: Some(period_ms),
                }
            })
            .collect();

        windows.sort_by(|a, b| a.label.cmp(&b.label));
        windows
    }

    fn parse_free_tier(
        &self,
        remaining: &HashMap<String, f64>,
        monthly: &Option<HashMap<String, f64>>,
        reset_date: &Option<String>,
    ) -> Vec<RateWindow> {
        let resets_at =
            parse_reset_date_utc(reset_date).or_else(|| Some(next_month_reset_utc(Utc::now())));

        let monthly = match monthly {
            Some(m) => m,
            None => return Vec::new(),
        };

        let period_ms = billing_period_ms(reset_date);

        let mut windows: Vec<RateWindow> = monthly
            .iter()
            .map(|(name, &total)| {
                let rem = remaining.get(name).copied().unwrap_or(total);
                let used_percent = if total > 0.0 {
                    ((total - rem) / total * 100.0).clamp(0.0, 100.0)
                } else {
                    0.0
                };

                RateWindow {
                    label: format!("{} ({})", pretty_label(name), total as i64),
                    used_percent,
                    resets_at,
                    period_duration_ms: Some(period_ms),
                }
            })
            .collect();

        windows.sort_by(|a, b| a.label.cmp(&b.label));
        windows
    }
}

fn pretty_label(key: &str) -> String {
    key.split('_')
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Timelike};

    #[test]
    fn pretty_label_capitalizes_words() {
        assert_eq!(pretty_label("chat_completions"), "Chat Completions");
        assert_eq!(pretty_label("completions"), "Completions");
        assert_eq!(pretty_label("premium_requests"), "Premium Requests");
    }

    #[test]
    fn parse_paid_tier_basic() {
        let fetcher = CopilotQuotaFetcher::new();
        let mut snapshots = HashMap::new();
        snapshots.insert(
            "completions".to_string(),
            QuotaSnapshotEntry {
                percent_remaining: Some(75.0),
                entitlement: Some(100.0),
                _remaining: None,
            },
        );
        let windows = fetcher.parse_paid_tier(&snapshots, &None);
        assert_eq!(windows.len(), 1);
        assert!((windows[0].used_percent - 25.0).abs() < 0.01);
        assert!(windows[0].label.contains("Completions"));
        assert!(windows[0].label.contains("100"));
    }

    #[test]
    fn parse_paid_tier_clamps_negative() {
        let fetcher = CopilotQuotaFetcher::new();
        let mut snapshots = HashMap::new();
        snapshots.insert(
            "completions".to_string(),
            QuotaSnapshotEntry {
                percent_remaining: Some(150.0),
                entitlement: None,
                _remaining: None,
            },
        );
        let windows = fetcher.parse_paid_tier(&snapshots, &None);
        assert_eq!(windows[0].used_percent, 0.0);
    }

    #[test]
    fn parse_paid_tier_clamps_over_100() {
        let fetcher = CopilotQuotaFetcher::new();
        let mut snapshots = HashMap::new();
        snapshots.insert(
            "completions".to_string(),
            QuotaSnapshotEntry {
                percent_remaining: Some(-50.0),
                entitlement: None,
                _remaining: None,
            },
        );
        let windows = fetcher.parse_paid_tier(&snapshots, &None);
        assert_eq!(windows[0].used_percent, 100.0);
    }

    #[test]
    fn parse_free_tier_basic() {
        let fetcher = CopilotQuotaFetcher::new();
        let mut remaining = HashMap::new();
        remaining.insert("chat_completions".to_string(), 40.0);
        let mut monthly = HashMap::new();
        monthly.insert("chat_completions".to_string(), 100.0);

        let windows = fetcher.parse_free_tier(&remaining, &Some(monthly), &None);
        assert_eq!(windows.len(), 1);
        assert!((windows[0].used_percent - 60.0).abs() < 0.01);
    }

    #[test]
    fn parse_free_tier_clamps_negative() {
        let fetcher = CopilotQuotaFetcher::new();
        let mut remaining = HashMap::new();
        remaining.insert("completions".to_string(), 200.0); // more remaining than total
        let mut monthly = HashMap::new();
        monthly.insert("completions".to_string(), 100.0);

        let windows = fetcher.parse_free_tier(&remaining, &Some(monthly), &None);
        assert_eq!(windows[0].used_percent, 0.0);
    }

    #[test]
    fn parse_free_tier_zero_total() {
        let fetcher = CopilotQuotaFetcher::new();
        let remaining = HashMap::new();
        let mut monthly = HashMap::new();
        monthly.insert("completions".to_string(), 0.0);

        let windows = fetcher.parse_free_tier(&remaining, &Some(monthly), &None);
        assert_eq!(windows[0].used_percent, 0.0);
    }

    #[test]
    fn parse_free_tier_no_monthly_returns_empty() {
        let fetcher = CopilotQuotaFetcher::new();
        let remaining = HashMap::new();
        let windows = fetcher.parse_free_tier(&remaining, &None, &None);
        assert!(windows.is_empty());
    }

    #[test]
    fn parse_paid_tier_with_reset_date() {
        let fetcher = CopilotQuotaFetcher::new();
        let mut snapshots = HashMap::new();
        snapshots.insert(
            "completions".to_string(),
            QuotaSnapshotEntry {
                percent_remaining: Some(50.0),
                entitlement: Some(1000.0),
                _remaining: None,
            },
        );
        let reset = Some("2025-08-01T00:00:00Z".to_string());
        let windows = fetcher.parse_paid_tier(&snapshots, &reset);
        assert!(windows[0].resets_at.is_some());
    }

    #[test]
    fn billing_period_ms_defaults_when_no_date() {
        let period = billing_period_ms(&None);
        assert!(
            period == 28 * 24 * 60 * 60 * 1000
                || period == 29 * 24 * 60 * 60 * 1000
                || period == 30 * 24 * 60 * 60 * 1000
                || period == 31 * 24 * 60 * 60 * 1000
        );
    }

    #[test]
    fn billing_period_ms_rfc3339_date() {
        // Aug 1 to Jul 1 = 31 days (July has 31 days)
        let reset = Some("2025-08-01T00:00:00Z".to_string());
        let period = billing_period_ms(&reset);
        let thirty_one_days_ms = 31 * 24 * 60 * 60 * 1000;
        assert_eq!(period, thirty_one_days_ms);
    }

    #[test]
    fn billing_period_ms_plain_date() {
        // Mar 1 to Feb 1 = 28 days (non-leap year 2025)
        let reset = Some("2025-03-01".to_string());
        let period = billing_period_ms(&reset);
        let twenty_eight_days_ms = 28 * 24 * 60 * 60 * 1000;
        assert_eq!(period, twenty_eight_days_ms);
    }

    #[test]
    fn days_in_month_various() {
        assert_eq!(days_in_month(2025, 2), 28); // non-leap
        assert_eq!(days_in_month(2024, 2), 29); // leap
        assert_eq!(days_in_month(2025, 1), 31);
        assert_eq!(days_in_month(2025, 4), 30);
        assert_eq!(days_in_month(2025, 12), 31);
    }

    #[test]
    fn subtract_one_month_basic() {
        let aug = Utc.with_ymd_and_hms(2025, 8, 15, 0, 0, 0).unwrap();
        let jul = subtract_one_month(aug).unwrap();
        assert_eq!(jul.month(), 7);
        assert_eq!(jul.day(), 15);
    }

    #[test]
    fn subtract_one_month_jan_wraps_to_dec() {
        let jan = Utc.with_ymd_and_hms(2025, 1, 10, 0, 0, 0).unwrap();
        let dec = subtract_one_month(jan).unwrap();
        assert_eq!(dec.year(), 2024);
        assert_eq!(dec.month(), 12);
        assert_eq!(dec.day(), 10);
    }

    #[test]
    fn subtract_one_month_clamps_day() {
        // Mar 31 → Feb doesn't have 31 days, should clamp
        let mar = Utc.with_ymd_and_hms(2025, 3, 31, 0, 0, 0).unwrap();
        let feb = subtract_one_month(mar).unwrap();
        assert_eq!(feb.month(), 2);
        assert_eq!(feb.day(), 28);
    }

    #[test]
    fn next_month_reset_is_first_day_midnight() {
        let now = Utc.with_ymd_and_hms(2025, 8, 15, 12, 30, 0).unwrap();
        let reset = next_month_reset_utc(now);
        assert_eq!(reset, Utc.with_ymd_and_hms(2025, 9, 1, 0, 0, 0).unwrap());
    }

    #[test]
    fn parse_paid_tier_without_reset_date_uses_next_month_reset() {
        let fetcher = CopilotQuotaFetcher::new();
        let mut snapshots = HashMap::new();
        snapshots.insert(
            "completions".to_string(),
            QuotaSnapshotEntry {
                percent_remaining: Some(50.0),
                entitlement: Some(1000.0),
                _remaining: None,
            },
        );

        let windows = fetcher.parse_paid_tier(&snapshots, &None);
        assert!(windows[0].resets_at.is_some());
        let reset = windows[0].resets_at.unwrap();
        assert_eq!(reset.day(), 1);
        assert_eq!(reset.hour(), 0);
        assert_eq!(reset.minute(), 0);
    }
}
