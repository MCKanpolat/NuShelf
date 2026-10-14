use crate::{
    error::{ApiError, ApiResult},
    version::{NuGetVersion, package_id},
};
use bytes::Bytes;
use roxmltree::Node;
use serde_json::{Value, json};
use std::io::{Cursor, Read};

pub struct ParsedPackage {
    pub id: String,
    pub display_id: String,
    pub version: NuGetVersion,
    pub title: String,
    pub description: String,
    pub authors: String,
    pub tags: String,
    pub project_url: Option<String>,
    pub license_expression: Option<String>,
    pub dependencies: Value,
    pub nuspec: Vec<u8>,
}
fn child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}
fn text(node: Node<'_, '_>, name: &str) -> String {
    child(node, name)
        .and_then(|c| c.text())
        .unwrap_or_default()
        .trim()
        .to_owned()
}
pub fn parse(bytes: &Bytes) -> ApiResult<ParsedPackage> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| ApiError::bad("Upload a valid .nupkg ZIP archive"))?;
    if archive.len() > 10_000 {
        return Err(ApiError::bad("Package contains too many entries"));
    }
    let mut manifest = None;
    let mut total = 0u64;
    for i in 0..archive.len() {
        let file = archive
            .by_index(i)
            .map_err(|_| ApiError::bad("Invalid ZIP entry"))?;
        total = total
            .checked_add(file.size())
            .ok_or_else(|| ApiError::bad("Package is too large"))?;
        if total > 512 * 1024 * 1024 {
            return Err(ApiError::bad("Uncompressed package exceeds 512 MiB"));
        }
        let name = file.name();
        if !name.contains(['/', '\\']) && name.to_ascii_lowercase().ends_with(".nuspec") {
            if manifest.is_some() {
                return Err(ApiError::bad(
                    "Package must contain exactly one root .nuspec",
                ));
            }
            manifest = Some(i);
        }
    }
    let mut file = archive
        .by_index(manifest.ok_or_else(|| ApiError::bad("Package has no root .nuspec"))?)
        .map_err(|_| ApiError::bad("Invalid manifest entry"))?;
    if file.size() > 1024 * 1024 {
        return Err(ApiError::bad("Manifest exceeds 1 MiB"));
    }
    let mut nuspec = Vec::new();
    (&mut file)
        .take(1024 * 1024 + 1)
        .read_to_end(&mut nuspec)
        .map_err(|_| ApiError::bad("Cannot read package manifest"))?;
    if nuspec.len() > 1024 * 1024 {
        return Err(ApiError::bad("Manifest exceeds 1 MiB"));
    }
    let xml = std::str::from_utf8(&nuspec).map_err(|_| ApiError::bad("Manifest must be UTF-8"))?;
    let doc = roxmltree::Document::parse(xml)
        .map_err(|_| ApiError::bad("Invalid nuspec XML (DTDs are not supported)"))?;
    if doc.root_element().tag_name().name() != "package" {
        return Err(ApiError::bad("Invalid nuspec root"));
    }
    let metadata = child(doc.root_element(), "metadata")
        .ok_or_else(|| ApiError::bad("Manifest has no metadata"))?;
    let display_id = text(metadata, "id");
    let id = package_id(&display_id)?;
    let mut version = NuGetVersion::parse(&text(metadata, "version"))?;
    let authors = text(metadata, "authors");
    let description = text(metadata, "description");
    if authors.is_empty() || description.is_empty() {
        return Err(ApiError::bad(
            "Manifest authors and description are required",
        ));
    }
    let mut groups = Vec::new();
    if let Some(deps) = child(metadata, "dependencies") {
        let grouped: Vec<_> = deps
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == "group")
            .collect();
        let containers = if grouped.is_empty() {
            vec![deps]
        } else {
            grouped
        };
        for group in containers {
            let mut dependencies = Vec::new();
            for dep in group
                .children()
                .filter(|c| c.is_element() && c.tag_name().name() == "dependency")
            {
                let dep_id = dep
                    .attribute("id")
                    .ok_or_else(|| ApiError::bad("Dependency ID is required"))?;
                package_id(dep_id)?;
                let range = dep.attribute("version").unwrap_or("(,)");
                if range.contains('*') {
                    return Err(ApiError::bad("Dependency ranges cannot contain wildcards"));
                }
                version.semver2 |= range
                    .split(['[', ']', '(', ')', ','])
                    .any(|v| NuGetVersion::parse(v.trim()).is_ok_and(|v| v.semver2));
                dependencies.push(json!({"id":dep_id,"range":range}));
            }
            groups.push(json!({"targetFramework":group.attribute("targetFramework").unwrap_or(""),"dependencies":dependencies}));
        }
    }
    let title = text(metadata, "title");
    let title = if title.is_empty() {
        display_id.clone()
    } else {
        title
    };
    let project_url = text(metadata, "projectUrl");
    let project_url = if project_url.is_empty() {
        None
    } else {
        let parsed =
            url::Url::parse(&project_url).map_err(|_| ApiError::bad("Invalid project URL"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(ApiError::bad("Project URL must use HTTP(S)"));
        }
        Some(project_url)
    };
    let license_expression = child(metadata, "license")
        .filter(|c| c.attribute("type") == Some("expression"))
        .and_then(|c| c.text())
        .map(str::to_owned);
    Ok(ParsedPackage {
        id,
        display_id,
        version,
        title,
        description,
        authors,
        tags: text(metadata, "tags"),
        project_url,
        license_expression,
        dependencies: json!(groups),
        nuspec,
    })
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::io::Write;
    pub fn fixture(id: &str, version: &str) -> Bytes {
        let xml = format!(
            r#"<package xmlns="http://schemas.microsoft.com/packaging/2013/05/nuspec.xsd"><metadata><id>{id}</id><version>{version}</version><authors>Test Owner</authors><description>A JSON test library</description><tags>json serialization</tags><dependencies><group targetFramework="net8.0"><dependency id="Other.Package" version="[1.0.0,2.0.0)" /></group></dependencies></metadata></package>"#
        );
        archive(&xml)
    }
    pub fn archive(xml: &str) -> Bytes {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("test.nuspec", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(xml.as_bytes()).unwrap();
        Bytes::from(writer.finish().unwrap().into_inner())
    }
    #[test]
    fn extracts_dependencies_and_identity() {
        let p = parse(&fixture("Acme.Json", "1.02.3-beta.2+sha")).unwrap();
        assert_eq!(p.id, "acme.json");
        assert_eq!(p.version.normalized, "1.2.3-beta.2");
        assert!(p.version.semver2);
        assert_eq!(p.dependencies[0]["dependencies"][0]["id"], "Other.Package");
    }
    #[test]
    fn rejects_invalid_archives_and_xml() {
        assert!(parse(&Bytes::from_static(b"not a zip")).is_err());
        assert!(parse(&archive("<!DOCTYPE package [<!ENTITY x SYSTEM 'file:///etc/passwd'>]><package><metadata/></package>")).is_err());
        assert!(parse(&fixture("../test", "1.0.0")).is_err());
    }
}
