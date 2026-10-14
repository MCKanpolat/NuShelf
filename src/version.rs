use crate::error::{ApiError, ApiResult};

#[derive(Debug, Clone)]
pub struct NuGetVersion {
    pub normalized: String,
    pub display: String,
    pub sort_key: String,
    pub prerelease: bool,
    pub semver2: bool,
}

impl NuGetVersion {
    pub fn parse(input: &str) -> ApiResult<Self> {
        let invalid = || ApiError::bad("Invalid NuGet version");
        if input.is_empty() || input.len() > 128 || !input.is_ascii() {
            return Err(invalid());
        }
        let (base, metadata) = input
            .split_once('+')
            .map_or((input, None), |(a, b)| (a, Some(b)));
        let (numbers, pre) = base
            .split_once('-')
            .map_or((base, None), |(a, b)| (a, Some(b)));
        let parts: Vec<_> = numbers.split('.').collect();
        if parts.is_empty() || parts.len() > 4 {
            return Err(invalid());
        }
        let mut nums = [0u32; 4];
        for (i, part) in parts.iter().enumerate() {
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return Err(invalid());
            }
            nums[i] = part.parse().map_err(|_| invalid())?;
            if nums[i] > i32::MAX as u32 {
                return Err(invalid());
            }
        }
        for label in [pre, metadata].into_iter().flatten() {
            if label
                .split('.')
                .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
            {
                return Err(invalid());
            }
        }
        let mut display = format!("{}.{}.{}", nums[0], nums[1], nums[2]);
        if nums[3] != 0 {
            display.push_str(&format!(".{}", nums[3]));
        }
        if let Some(pre) = pre {
            display.push('-');
            display.push_str(pre);
        }
        let normalized = display.to_ascii_lowercase();
        if let Some(metadata) = metadata {
            display.push('+');
            display.push_str(metadata);
        }
        let mut sort_key = nums
            .iter()
            .map(|n| format!("{n:010}"))
            .collect::<Vec<_>>()
            .join(".");
        match pre {
            None => sort_key.push_str("/1"),
            Some(pre) => {
                sort_key.push_str("/0");
                for p in pre.to_ascii_lowercase().split('.') {
                    if p.bytes().all(|b| b.is_ascii_digit()) {
                        let stripped = p.trim_start_matches('0');
                        let stripped = if stripped.is_empty() { "0" } else { stripped };
                        sort_key.push_str(&format!("0{:03}{stripped}!", stripped.len()));
                    } else {
                        sort_key.push('1');
                        sort_key.push_str(&hex::encode(p));
                        sort_key.push('!');
                    }
                }
            }
        }
        Ok(Self {
            normalized,
            display,
            sort_key,
            prerelease: pre.is_some(),
            semver2: metadata.is_some() || pre.is_some_and(|p| p.contains('.')),
        })
    }
}

pub fn package_id(input: &str) -> ApiResult<String> {
    if input.is_empty()
        || input.len() > 100
        || !input
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        || !input.bytes().any(|b| b.is_ascii_alphanumeric())
    {
        return Err(ApiError::bad(
            "Package ID must contain 1–100 letters, digits, dots, hyphens or underscores",
        ));
    }
    Ok(input.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalization() {
        for (input, expected) in [
            ("01.2", "1.2.0"),
            ("1.2.3.0+build.4", "1.2.3"),
            ("1.2.3.4-RC.1", "1.2.3.4-rc.1"),
        ] {
            assert_eq!(NuGetVersion::parse(input).unwrap().normalized, expected);
        }
        for bad in [
            "",
            "1..2",
            "1.2.3-",
            "1.2.3+",
            "1.2.3/a",
            "1.2.3.4.5",
            "2147483648",
        ] {
            assert!(NuGetVersion::parse(bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn semantic_order() {
        let versions = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.2",
            "1.0.0-alpha.10",
            "1.0.0-alpha0",
            "1.0.0-beta",
            "1.0.0",
            "1.0.0.1",
            "1.0.2",
            "1.0.10",
            "2.0.0",
        ];
        let keys: Vec<_> = versions
            .iter()
            .map(|v| NuGetVersion::parse(v).unwrap().sort_key)
            .collect();
        for pair in keys.windows(2) {
            assert!(pair[0] < pair[1], "{pair:?}");
        }
    }
    #[test]
    fn path_safe_ids() {
        for id in ["../foo", "a/b", "...", "a%20b"] {
            assert!(package_id(id).is_err());
        }
    }
}
