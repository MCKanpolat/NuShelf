use super::{
    model::{Options, boolean, literal, range_bounds},
    repository::base_query,
};
use crate::{
    AppState,
    error::{ApiError, ApiResult},
    upstream::{self, sources::Reader},
    version::{NuGetVersion, package_id},
};
use serde_json::{Value, json};
use sqlx::{Postgres, QueryBuilder};
pub(super) async fn build(
    state: &AppState,
    reader: &Reader,
    path: &str,
    opts: &Options,
) -> ApiResult<(QueryBuilder<'static, Postgres>, bool)> {
    let semver = path.starts_with("Packages(")
        || opts.get("semVerLevel").is_some_and(|s| {
            NuGetVersion::parse(s)
                .is_ok_and(|v| v.sort_key >= NuGetVersion::parse("2.0.0").unwrap().sort_key)
        });
    let exact_id = if path.starts_with("FindPackagesById") {
        opts.get("id").map(|v| literal(v)).transpose()?
    } else if let Some(key) = path
        .strip_prefix("Packages(")
        .and_then(|s| s.strip_suffix(')'))
    {
        key.split(',')
            .find_map(|part| part.trim().strip_prefix("Id="))
            .map(literal)
            .transpose()?
    } else {
        None
    };
    let mut extras = Vec::new();
    if let Some(id) = exact_id {
        let id = package_id(&id)?;
        crate::feeds::check_package(state, &id)?;
        if !upstream::cache::local_exists(state, &id).await? {
            match upstream::cache::catalog(state, reader, &id).await {
                Ok((entries, _)) => {
                    for e in entries {
                        let v = NuGetVersion::parse(e["version"].as_str().unwrap_or_default())?;
                        extras.push(json!({"Id":e["id"],"Version":e["version"],"NormalizedVersion":v.normalized,"_sort":v.sort_key,"_semver2":v.semver2,"Title":e["title"],"Description":e["description"],"Authors":e["authors"],"Tags":e["tags"].as_array().map(|v|v.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")).unwrap_or_default(),"ProjectUrl":e["projectUrl"],"Published":e["published"],"Created":e["published"],"LastUpdated":e["published"],"IsPrerelease":v.prerelease,"Listed":e["listed"],"_dependencies":e["dependencyGroups"],"DownloadCount":0,"VersionDownloadCount":0}));
                    }
                }
                Err(e) if e.0 == axum::http::StatusCode::NOT_FOUND => {}
                Err(e) => return Err(e),
            }
        }
    }
    let mut b = base_query(state, reader, semver, json!(extras))?;
    b.push("TRUE");
    let mut single = false;
    if let Some(key) = path
        .strip_prefix("Packages(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let mut keys = Options::new();
        for item in key.split(',') {
            let (k, v) = item
                .split_once('=')
                .ok_or_else(|| ApiError::bad("Invalid OData entity key"))?;
            keys.insert(k.trim().to_string(), literal(v)?);
        }
        if keys.len() != 2 || !keys.contains_key("Id") || !keys.contains_key("Version") {
            return Err(ApiError::bad("Id and Version are required"));
        }
        let id = package_id(&keys["Id"])?;
        crate::feeds::check_package(state, &id)?;
        let version = NuGetVersion::parse(&keys["Version"])?.normalized;
        b.push(" AND id=")
            .push_bind(id)
            .push(" AND data->>'NormalizedVersion'=")
            .push_bind(version);
        single = true;
    } else {
        match path {
            "Packages" | "Packages()" => {}
            "FindPackagesById" | "FindPackagesById()" => {
                let id = package_id(&literal(
                    opts.get("id")
                        .ok_or_else(|| ApiError::bad("id is required"))?,
                )?)?;
                crate::feeds::check_package(state, &id)?;
                b.push(" AND id=").push_bind(id);
            }
            "Search" | "Search()" => {
                let q = literal(opts.get("searchTerm").map(String::as_str).unwrap_or("''"))?
                    .to_ascii_lowercase();
                if q.len() > 256 {
                    return Err(ApiError::bad("Search term is too long"));
                }
                b.push(" AND (data->>'Listed')::boolean AND (").push_bind(boolean(opts,"includePrerelease",false)?).push(" OR NOT (data->>'IsPrerelease')::boolean) AND strpos(lower(concat_ws(' ',data->>'Id',data->>'Title',data->>'Description',data->>'Tags')),").push_bind(q).push(")>0");
            }
            "GetUpdates" | "GetUpdates()" => {
                let ids = literal(
                    opts.get("packageIds")
                        .ok_or_else(|| ApiError::bad("packageIds is required"))?,
                )?;
                let versions = literal(
                    opts.get("versions")
                        .ok_or_else(|| ApiError::bad("versions is required"))?,
                )?;
                let ids = ids.split('|').collect::<Vec<_>>();
                let versions = versions.split('|').collect::<Vec<_>>();
                if ids.len() != versions.len() || ids.len() > 100 {
                    return Err(ApiError::bad(
                        "Provide matching packageIds and versions (at most 100)",
                    ));
                }
                let constraints = opts
                    .get("versionConstraints")
                    .map(|v| literal(v))
                    .transpose()?
                    .unwrap_or_default();
                let constraints = constraints.split('|').collect::<Vec<_>>();
                if constraints.len() != 1 && constraints.len() != ids.len() {
                    return Err(ApiError::bad("versionConstraints must match packageIds"));
                }
                b.push(" AND (data->>'Listed')::boolean AND (")
                    .push_bind(boolean(opts, "includePrerelease", false)?)
                    .push(" OR NOT (data->>'IsPrerelease')::boolean) AND (");
                for (i, (id, v)) in ids.iter().zip(versions).enumerate() {
                    if i > 0 {
                        b.push(" OR ");
                    }
                    let id = package_id(id)?;
                    let v = NuGetVersion::parse(v)?;
                    b.push("(id=")
                        .push_bind(id)
                        .push(" AND version_sort > ")
                        .push_bind(v.sort_key);
                    let constraint = constraints.get(i).copied().unwrap_or("");
                    if !constraint.is_empty() {
                        let (lower, upper, inc_lower, inc_upper) = range_bounds(constraint)?;
                        if let Some(v) = lower {
                            b.push(if inc_lower {
                                " AND version_sort >= "
                            } else {
                                " AND version_sort > "
                            })
                            .push_bind(v);
                        }
                        if let Some(v) = upper {
                            b.push(if inc_upper {
                                " AND version_sort <= "
                            } else {
                                " AND version_sort < "
                            })
                            .push_bind(v);
                        }
                    }
                    b.push(")");
                }
                b.push(")");
            }
            _ => return Err(ApiError::not_found()),
        }
    }
    let framework = opts.get(if path.starts_with("GetUpdates") {
        "targetFrameworks"
    } else {
        "targetFramework"
    });
    if let Some(framework) = framework {
        let framework = literal(framework)?;
        if !framework.is_empty() {
            let mut allowed = Vec::new();
            for target in framework.split('|') {
                allowed.extend(super::frameworks::compatible_targets(target)?);
            }
            allowed.sort();
            allowed.dedup();
            b.push(" AND (COALESCE(jsonb_array_length(data->'_frameworks'),0)=0 OR EXISTS(SELECT 1 FROM jsonb_array_elements_text(data->'_frameworks') f WHERE lower(f)=ANY(").push_bind(allowed).push(")))");
        }
    }
    if let Some(filter) = opts.get("$filter") {
        b.push(" AND (");
        super::filter::append(&mut b, filter)?;
        b.push(")");
    }

    Ok((b, single))
}
