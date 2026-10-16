use super::{
    filter::field,
    model::{Options, literal},
};
use crate::error::{ApiError, ApiResult};
pub(super) fn validate(opts: &Options) -> ApiResult<()> {
    if opts.len() > 16 || opts.iter().any(|(k, v)| k.len() > 100 || v.len() > 4096) {
        return Err(ApiError::bad("OData query is too large"));
    }
    for key in opts.keys() {
        if ![
            "$filter",
            "$orderby",
            "$skip",
            "$top",
            "$inlinecount",
            "$select",
            "id",
            "searchTerm",
            "targetFramework",
            "includePrerelease",
            "semVerLevel",
            "packageIds",
            "versions",
            "includeAllVersions",
            "targetFrameworks",
            "versionConstraints",
        ]
        .contains(&key.as_str())
        {
            return Err(ApiError::bad("Unsupported OData query option"));
        }
    }
    if opts
        .get("$inlinecount")
        .is_some_and(|s| !matches!(s.as_str(), "allpages" | "none"))
    {
        return Err(ApiError::bad("Invalid inline count"));
    }
    if let Some(select) = opts.get("$select") {
        for name in select.split(',') {
            field(name.trim())?;
        }
    }
    for key in ["targetFramework", "targetFrameworks"] {
        if let Some(v) = opts.get(key) {
            literal(v)?;
        }
    }
    Ok(())
}
