pub(crate) fn strip_date_suffix(model_id: &str) -> Option<String> {
    if model_id.len() > 9 {
        let suffix = &model_id[model_id.len() - 8..];
        if suffix.chars().all(|ch| ch.is_ascii_digit())
            && model_id.as_bytes()[model_id.len() - 9] == b'-'
        {
            return Some(model_id[..model_id.len() - 9].to_string());
        }
    }

    if model_id.len() > 11 {
        let suffix = &model_id[model_id.len() - 10..];
        let bytes = suffix.as_bytes();
        let is_dash_date = bytes[4] == b'-'
            && bytes[7] == b'-'
            && suffix
                .chars()
                .enumerate()
                .all(|(idx, ch)| idx == 4 || idx == 7 || ch.is_ascii_digit());
        if is_dash_date && model_id.as_bytes()[model_id.len() - 11] == b'-' {
            return Some(model_id[..model_id.len() - 11].to_string());
        }
    }

    None
}
