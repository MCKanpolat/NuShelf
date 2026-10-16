//! Compatibility expansion for the .NET framework families understood by NuGet.
//! Unknown/platform frameworks retain exact matching, while platform projects
//! can also consume packages targeting their base framework.
use crate::error::{ApiError, ApiResult};
fn version(text: &str, compact: bool) -> Option<(u16, u16, u16)> {
    let parts = if compact && !text.contains('.') {
        text.chars().map(|c| c.to_string()).collect::<Vec<_>>()
    } else {
        text.split('.').map(str::to_string).collect()
    };
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let mut n = [0; 3];
    for (i, s) in parts.iter().enumerate() {
        n[i] = s.parse().ok()?;
    }
    Some((n[0], n[1], n[2]))
}
fn canonical(input: &str) -> String {
    let s = input.to_ascii_lowercase();
    for (name, prefix) in [
        (".netframework,version=v", "net"),
        (".netstandard,version=v", "netstandard"),
        (".netcoreapp,version=v", "netcoreapp"),
    ] {
        if let Some(v) = s.strip_prefix(name) {
            let v = v.split(',').next().unwrap_or_default();
            return format!(
                "{prefix}{}",
                if prefix == "net" {
                    v.replace('.', "")
                } else {
                    v.to_string()
                }
            );
        }
    }
    s
}
pub(super) fn compatible_targets(input: &str) -> ApiResult<Vec<String>> {
    if input.len() > 200 || !input.is_ascii() || input.chars().any(char::is_control) {
        return Err(ApiError::bad("Invalid target framework"));
    }
    let target = canonical(input);
    let base = target.split('-').next().unwrap_or_default();
    let mut out = vec![
        target.clone(),
        base.to_owned(),
        String::new(),
        "any".into(),
        "agnostic".into(),
    ];
    let (family, v) = if let Some(v) = base.strip_prefix("netstandard") {
        ("netstandard", version(v, false))
    } else if let Some(v) = base.strip_prefix("netcoreapp") {
        ("netcoreapp", version(v, false))
    } else if let Some(v) = base.strip_prefix("net") {
        ("net", version(v, !v.contains('.')))
    } else {
        return Ok(out);
    };
    let Some(v) = v else {
        return Err(ApiError::bad("Invalid target framework version"));
    };
    if v.0 > 100 || v.1 > 100 {
        return Err(ApiError::bad("Target framework version is out of bounds"));
    }
    let standard = match (family, v) {
        ("netstandard", v) => v,
        ("net", (major, _, _)) if major >= 5 => (2, 1, 0),
        ("net", v) if v >= (4, 6, 1) => (2, 0, 0),
        ("net", v) if v >= (4, 6, 0) => (1, 3, 0),
        ("net", v) if v >= (4, 5, 1) => (1, 2, 0),
        ("net", v) if v >= (4, 5, 0) => (1, 1, 0),
        ("netcoreapp", v) if v >= (3, 0, 0) => (2, 1, 0),
        ("netcoreapp", v) if v >= (2, 0, 0) => (2, 0, 0),
        ("netcoreapp", _) => (1, 6, 0),
        _ => (0, 0, 0),
    };
    for major in 1..=2 {
        for minor in 0..=6 {
            if (major, minor, 0) <= standard && (major == 1 || minor <= 1) {
                out.push(format!("netstandard{major}.{minor}"));
            }
        }
    }
    if family == "netcoreapp" || (family == "net" && v.0 >= 5) {
        let max = if family == "netcoreapp" { v } else { (3, 1, 0) };
        for major in 1..=3 {
            for minor in 0..=1 {
                if (major, minor, 0) <= max {
                    out.push(format!("netcoreapp{major}.{minor}"));
                }
            }
        }
    }
    if family == "net" {
        if v.0 >= 5 {
            for major in 5..=v.0 {
                let max_minor = if major == v.0 { v.1 } else { 0 };
                for minor in 0..=max_minor {
                    out.push(format!("net{major}.{minor}"));
                }
            }
        } else {
            for value in [
                "net11", "net20", "net35", "net40", "net403", "net45", "net451", "net452", "net46",
                "net461", "net462", "net47", "net471", "net472", "net48", "net481",
            ] {
                if version(&value[3..], true).is_some_and(|old| old <= v) {
                    out.push(value.into());
                }
            }
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn standard_and_legacy() {
        let net = compatible_targets("net8.0").unwrap();
        assert!(net.contains(&"netstandard2.1".into()));
        assert!(net.contains(&"netcoreapp3.1".into()));
        assert!(!net.contains(&"net48".into()));
        let old = compatible_targets(".NETFramework,Version=v4.6.1").unwrap();
        assert!(old.contains(&"netstandard2.0".into()));
        assert!(!old.contains(&"netstandard2.1".into()));
    }
}
