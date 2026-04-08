pub(crate) fn parse_timestamp_str(value: &str) -> Option<i64> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(value) {
        return Some(dt.timestamp_millis());
    }

    if let Ok(numeric) = value.parse::<i64>() {
        if numeric >= 1_000_000_000_000 {
            return Some(numeric);
        }
        return Some(numeric * 1000);
    }

    None
}
