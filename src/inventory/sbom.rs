use super::model::{Component, Inventory};
use crate::{
    error::{ApiError, ApiResult},
    version::{NuGetVersion, package_id},
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

fn string(value: &Value, field: &str, limit: usize) -> ApiResult<String> {
    value[field]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= limit && !s.chars().any(char::is_control))
        .map(str::to_owned)
        .ok_or_else(|| ApiError::bad(format!("SBOM {field} is missing or invalid")))
}
fn decode(value: &str) -> ApiResult<String> {
    let bytes = value.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'%'
            && (bytes.get(i + 1).is_none_or(|c| !c.is_ascii_hexdigit())
                || bytes.get(i + 2).is_none_or(|c| !c.is_ascii_hexdigit()))
        {
            return Err(ApiError::bad("Invalid package URL percent encoding"));
        }
    }
    percent_encoding::percent_decode_str(value)
        .decode_utf8()
        .map(|v| v.into_owned())
        .map_err(|_| ApiError::bad("Invalid package URL encoding"))
}
fn nuget(value: &Value, purl: Option<&str>) -> ApiResult<(Option<String>, Option<String>)> {
    let Some(raw) = purl.and_then(|p| p.strip_prefix("pkg:nuget/")) else {
        return Ok((None, None));
    };
    let raw = raw.split(['?', '#']).next().unwrap_or_default();
    let (id, version) = raw
        .split_once('@')
        .map_or((raw, None), |(id, v)| (id, Some(v)));
    let id = package_id(&decode(id)?)?;
    let raw_version = version
        .map(decode)
        .transpose()?
        .or_else(|| value["version"].as_str().map(str::to_owned))
        .ok_or_else(|| ApiError::bad("NuGet components need an exact version"))?;
    let version = NuGetVersion::parse(&raw_version)?;
    if let Some(other) = value["version"].as_str()
        && NuGetVersion::parse(other)?.normalized != version.normalized
    {
        return Err(ApiError::bad("Component version and package URL disagree"));
    }
    Ok((Some(id), Some(version.normalized)))
}
fn collect(
    values: &[Value],
    depth: usize,
    components: &mut BTreeMap<String, Component>,
    application_root: bool,
) -> ApiResult<()> {
    if depth > 32 {
        return Err(ApiError::bad("SBOM component nesting exceeds 32 levels"));
    }
    for value in values {
        if components.len() >= 5000 {
            return Err(ApiError::bad("SBOM exceeds 5000 components"));
        }
        let name = string(value, "name", 512)?;
        let reference = if value.get("bom-ref").is_some() {
            string(value, "bom-ref", 1024)?
        } else {
            format!("urn:nushelf:component:{}", components.len())
        };
        let purl = value
            .get("purl")
            .map(|_| string(value, "purl", 2048))
            .transpose()?;
        let (package_id, version) = if application_root {
            (None, None)
        } else {
            nuget(value, purl.as_deref())?
        };
        if components
            .insert(
                reference.clone(),
                Component {
                    reference,
                    name,
                    purl,
                    package_id,
                    version,
                },
            )
            .is_some()
        {
            return Err(ApiError::bad("SBOM contains duplicate bom-ref values"));
        }
        if let Some(children) = value.get("components") {
            collect(
                children
                    .as_array()
                    .ok_or_else(|| ApiError::bad("SBOM components must be an array"))?,
                depth + 1,
                components,
                false,
            )?;
        }
    }
    Ok(())
}
pub(super) fn parse(document: &Value) -> ApiResult<Inventory> {
    if document["bomFormat"] != "CycloneDX"
        || !matches!(
            document["specVersion"].as_str(),
            Some("1.4" | "1.5" | "1.6" | "1.7")
        )
        || document["version"].as_u64().is_none_or(|v| v == 0)
    {
        return Err(ApiError::bad(
            "Supply a CycloneDX JSON 1.4–1.7 BOM with a positive document version",
        ));
    }
    let mut components = BTreeMap::new();
    let root = if let Some(component) = document.pointer("/metadata/component") {
        collect(std::slice::from_ref(component), 0, &mut components, true)?;
        let root = component["bom-ref"]
            .as_str()
            .unwrap_or("urn:nushelf:component:0")
            .to_owned();
        if let Some(node) = components.get_mut(&root) {
            node.package_id = None;
            node.version = None;
        }
        Some(root)
    } else {
        None
    };
    if let Some(values) = document.get("components") {
        collect(
            values
                .as_array()
                .ok_or_else(|| ApiError::bad("SBOM components must be an array"))?,
            0,
            &mut components,
            false,
        )?;
    }
    if components.is_empty() {
        return Err(ApiError::bad("SBOM contains no components"));
    }
    let mut edges = BTreeSet::new();
    if let Some(dependencies) = document.get("dependencies") {
        for item in dependencies
            .as_array()
            .ok_or_else(|| ApiError::bad("SBOM dependencies must be an array"))?
        {
            let parent = string(item, "ref", 1024)?;
            if !components.contains_key(&parent) {
                return Err(ApiError::bad("Dependency references an unknown component"));
            }
            for child in item["dependsOn"]
                .as_array()
                .ok_or_else(|| ApiError::bad("Each dependency entry needs a dependsOn array"))?
            {
                let child = child
                    .as_str()
                    .ok_or_else(|| ApiError::bad("Invalid dependency reference"))?;
                if !components.contains_key(child) {
                    return Err(ApiError::bad("Dependency references an unknown component"));
                }
                edges.insert((parent.clone(), child.to_owned()));
                if edges.len() > 20000 {
                    return Err(ApiError::bad("SBOM exceeds 20000 dependency edges"));
                }
            }
        }
    }
    Ok(Inventory {
        components: components.into_values().collect(),
        edges: edges.into_iter().collect(),
        root,
    })
}
