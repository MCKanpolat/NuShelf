use crate::{
    error::{ApiError, ApiResult},
    version::NuGetVersion,
};
use std::collections::BTreeMap;
pub(super) type Options = BTreeMap<String, String>;
pub(super) const NS: &str = "xmlns=\"http://www.w3.org/2005/Atom\" xmlns:d=\"http://schemas.microsoft.com/ado/2007/08/dataservices\" xmlns:m=\"http://schemas.microsoft.com/ado/2007/08/dataservices/metadata\"";
pub(super) const FIELDS: &[(&str, &str)] = &[
    ("Id", "String"),
    ("Version", "String"),
    ("NormalizedVersion", "String"),
    ("Title", "String"),
    ("Description", "String"),
    ("Summary", "String"),
    ("Authors", "String"),
    ("Tags", "String"),
    ("ProjectUrl", "String"),
    ("LicenseUrl", "String"),
    ("IconUrl", "String"),
    ("Dependencies", "String"),
    ("PackageHash", "String"),
    ("PackageHashAlgorithm", "String"),
    ("PackageSize", "Int64"),
    ("DownloadCount", "Int64"),
    ("VersionDownloadCount", "Int64"),
    ("Published", "DateTime"),
    ("Created", "DateTime"),
    ("LastUpdated", "DateTime"),
    ("IsPrerelease", "Boolean"),
    ("IsLatestVersion", "Boolean"),
    ("IsAbsoluteLatestVersion", "Boolean"),
    ("Listed", "Boolean"),
    ("RequireLicenseAcceptance", "Boolean"),
    ("MinClientVersion", "String"),
    ("Language", "String"),
    ("ReleaseNotes", "String"),
    ("Copyright", "String"),
    ("ReportAbuseUrl", "String"),
    ("GalleryDetailsUrl", "String"),
];
pub(super) fn literal(s: &str) -> ApiResult<String> {
    let s = s.trim();
    if !s.starts_with('\'') || !s.ends_with('\'') || s.len() < 2 {
        return Err(ApiError::bad(
            "OData string parameters must be single quoted",
        ));
    }
    let inner = &s[1..s.len() - 1];
    let mut out = String::new();
    let mut it = inner.chars();
    while let Some(c) = it.next() {
        if c == '\'' && it.next() != Some('\'') {
            return Err(ApiError::bad("Invalid OData quoted string"));
        }
        out.push(c);
    }
    Ok(out)
}
pub(super) fn number(opts: &Options, key: &str, default: i64, max: i64) -> ApiResult<i64> {
    let n = opts
        .get(key)
        .map(|s| s.parse::<i64>())
        .transpose()
        .map_err(|_| ApiError::bad("Invalid OData pagination"))?
        .unwrap_or(default);
    if !(0..=max).contains(&n) {
        return Err(ApiError::bad("OData pagination is out of bounds"));
    }
    Ok(n)
}
pub(super) fn boolean(opts: &Options, key: &str, default: bool) -> ApiResult<bool> {
    opts.get(key)
        .map(|s| s.parse())
        .transpose()
        .map_err(|_| ApiError::bad("Invalid OData boolean"))
        .map(|v| v.unwrap_or(default))
}
type Bounds = (Option<String>, Option<String>, bool, bool);
pub(super) fn range_bounds(s: &str) -> ApiResult<Bounds> {
    crate::audit::validate_range(s)?;
    if s.starts_with(['[', '(']) {
        let low = s.starts_with('[');
        let high = s.ends_with(']');
        let inner = &s[1..s.len() - 1];
        if let Some((a, b)) = inner.split_once(',') {
            Ok((
                if a.trim().is_empty() {
                    None
                } else {
                    Some(NuGetVersion::parse(a.trim())?.sort_key)
                },
                if b.trim().is_empty() {
                    None
                } else {
                    Some(NuGetVersion::parse(b.trim())?.sort_key)
                },
                low,
                high,
            ))
        } else {
            let v = NuGetVersion::parse(inner)?.sort_key;
            Ok((Some(v.clone()), Some(v), true, true))
        }
    } else {
        Ok((Some(NuGetVersion::parse(s)?.sort_key), None, true, false))
    }
}
