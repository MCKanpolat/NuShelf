use super::{
    model::{FIELDS, NS, Options},
    repository::Page,
};
use crate::{
    AppState,
    error::{ApiError, ApiResult},
};
use axum::{
    http::{Uri, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
fn escape(s: &str) -> String {
    s.chars()
        .filter(|c| *c == '\t' || *c == '\n' || *c == '\r' || *c >= ' ')
        .fold(String::new(), |mut s, c| {
            s.push_str(match c {
                '&' => "&amp;",
                '<' => "&lt;",
                '>' => "&gt;",
                '"' => "&quot;",
                '\'' => "&apos;",
                _ => {
                    s.push(c);
                    return s;
                }
            });
            s
        })
}
fn xml(s: String, kind: &str) -> Response {
    (
        [
            (header::CONTENT_TYPE, format!("{kind}; charset=utf-8")),
            (
                header::HeaderName::from_static("dataserviceversion"),
                "2.0;".into(),
            ),
        ],
        s,
    )
        .into_response()
}
fn entry(state: &AppState, mut data: Value, select: Option<&str>) -> String {
    let id = data["Id"].as_str().unwrap_or_default().to_owned();
    let ver = data["NormalizedVersion"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let entity = format!(
        "{}/api/v2/Packages(Id='{}',Version='{}')",
        state.config.public_url, id, ver
    );
    let download = format!("{}/api/v2/package/{}/{}", state.config.public_url, id, ver);
    let deps = data["_dependencies"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|g| {
            g["dependencies"]
                .as_array()
                .into_iter()
                .flatten()
                .map(move |d| {
                    format!(
                        "{}:{}:{}",
                        d["id"].as_str().unwrap_or_default(),
                        d["range"].as_str().unwrap_or_default(),
                        g["targetFramework"].as_str().unwrap_or_default()
                    )
                })
        })
        .collect::<Vec<_>>()
        .join("|");
    data["Dependencies"] = json!(deps);
    data["PackageHashAlgorithm"] = json!("SHA512");
    data["RequireLicenseAcceptance"] = json!(false);
    data["GalleryDetailsUrl"] = json!(format!("{}/packages/{id}/{ver}", state.config.public_url));
    let mut out = format!(
        "<entry {NS}><id>{}</id><title type=\"text\">{}</title><updated>{}</updated><author><name>{}</name></author><category term=\"NuGetGallery.V2FeedPackage\" scheme=\"http://schemas.microsoft.com/ado/2007/08/dataservices/scheme\"/><link rel=\"edit\" title=\"V2FeedPackage\" href=\"{}\"/><content type=\"application/zip\" src=\"{}\"/><m:properties>",
        escape(&entity),
        escape(&id),
        escape(
            data["LastUpdated"]
                .as_str()
                .unwrap_or("1970-01-01T00:00:00Z")
        ),
        escape(data["Authors"].as_str().unwrap_or_default()),
        escape(&entity),
        escape(&download)
    );
    for (name, ty) in FIELDS {
        if select.is_some_and(|s| {
            !s.split(',').any(|v| v.trim() == *name) && !matches!(*name, "Id" | "Version")
        }) {
            continue;
        }
        let v = &data[*name];
        if v.is_null() {
            out.push_str(&format!("<d:{name} m:type=\"Edm.{ty}\" m:null=\"true\"/>"));
        } else {
            out.push_str(&format!(
                "<d:{name} m:type=\"Edm.{ty}\">{}</d:{name}>",
                escape(
                    &v.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| v.to_string())
                )
            ));
        }
    }
    out.push_str("</m:properties></entry>");
    out
}
pub(super) fn service(state: &AppState) -> Response {
    xml(
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><service xmlns=\"http://www.w3.org/2007/app\" xmlns:atom=\"http://www.w3.org/2005/Atom\" xml:base=\"{}/api/v2/\"><workspace><atom:title>NuShelf</atom:title><collection href=\"Packages\"><atom:title>Packages</atom:title></collection></workspace></service>",
            escape(&state.config.public_url)
        ),
        "application/atomsvc+xml",
    )
}
pub(super) fn metadata() -> Response {
    let fields = FIELDS
        .iter()
        .map(|(name, ty)| {
            format!(
                "<Property Name=\"{name}\" Type=\"Edm.{ty}\" Nullable=\"{}\"/>",
                !matches!(*name, "Id" | "Version")
            )
        })
        .collect::<String>();
    xml(
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><edmx:Edmx Version=\"1.0\" xmlns:edmx=\"http://schemas.microsoft.com/ado/2007/06/edmx\"><edmx:DataServices m:DataServiceVersion=\"2.0\" xmlns:m=\"http://schemas.microsoft.com/ado/2007/08/dataservices/metadata\"><Schema Namespace=\"NuGetGallery\" xmlns=\"http://schemas.microsoft.com/ado/2008/09/edm\"><EntityType Name=\"V2FeedPackage\" m:HasStream=\"true\"><Key><PropertyRef Name=\"Id\"/><PropertyRef Name=\"Version\"/></Key>{fields}</EntityType><EntityContainer Name=\"V2FeedContext\" m:IsDefaultEntityContainer=\"true\"><EntitySet Name=\"Packages\" EntityType=\"NuGetGallery.V2FeedPackage\"/><FunctionImport Name=\"FindPackagesById\" ReturnType=\"Collection(NuGetGallery.V2FeedPackage)\" EntitySet=\"Packages\" m:HttpMethod=\"GET\"><Parameter Name=\"id\" Type=\"Edm.String\" Mode=\"In\"/></FunctionImport><FunctionImport Name=\"Search\" ReturnType=\"Collection(NuGetGallery.V2FeedPackage)\" EntitySet=\"Packages\" m:HttpMethod=\"GET\"><Parameter Name=\"searchTerm\" Type=\"Edm.String\" Mode=\"In\"/><Parameter Name=\"targetFramework\" Type=\"Edm.String\" Mode=\"In\"/><Parameter Name=\"includePrerelease\" Type=\"Edm.Boolean\" Mode=\"In\"/></FunctionImport><FunctionImport Name=\"GetUpdates\" ReturnType=\"Collection(NuGetGallery.V2FeedPackage)\" EntitySet=\"Packages\" m:HttpMethod=\"GET\"><Parameter Name=\"packageIds\" Type=\"Edm.String\" Mode=\"In\"/><Parameter Name=\"versions\" Type=\"Edm.String\" Mode=\"In\"/><Parameter Name=\"includePrerelease\" Type=\"Edm.Boolean\" Mode=\"In\"/><Parameter Name=\"includeAllVersions\" Type=\"Edm.Boolean\" Mode=\"In\"/><Parameter Name=\"targetFrameworks\" Type=\"Edm.String\" Mode=\"In\"/><Parameter Name=\"versionConstraints\" Type=\"Edm.String\" Mode=\"In\"/></FunctionImport></EntityContainer></Schema></edmx:DataServices></edmx:Edmx>"
        ),
        "application/xml",
    )
}

pub(super) fn render(
    state: &AppState,
    uri: &Uri,
    opts: &Options,
    path: &str,
    page: Page,
) -> ApiResult<Response> {
    if page.count {
        return Ok((
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            page.total.to_string(),
        )
            .into_response());
    }
    if page.single {
        return page
            .rows
            .into_iter()
            .next()
            .map(|v| {
                xml(
                    entry(state, v, opts.get("$select").map(String::as_str)),
                    "application/atom+xml",
                )
            })
            .ok_or_else(ApiError::not_found);
    }
    let Page {
        rows,
        total,
        single: _,
        count: _,
        take,
        skip,
        requested,
    } = page;
    let has_next = rows.len() as i64 > take && (!opts.contains_key("$top") || requested > take);
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><feed {NS} xml:base=\"{}/api/v2/\"><id>{}</id><title type=\"text\">Packages</title><updated>{}</updated>",
        escape(&state.config.public_url),
        escape(&format!(
            "{}{}",
            state.config.public_url,
            uri.path_and_query().map(|v| v.as_str()).unwrap_or_default()
        )),
        chrono::Utc::now().to_rfc3339()
    );
    if opts.get("$inlinecount").is_some_and(|s| s == "allpages") {
        out.push_str(&format!("<m:count>{total}</m:count>"));
    }
    for row in rows.into_iter().take(take as usize) {
        out.push_str(&entry(state, row, opts.get("$select").map(String::as_str)));
    }
    if has_next {
        let mut opts = opts.clone();
        opts.insert("$skip".into(), (skip + take).to_string());
        if opts.contains_key("$top") {
            opts.insert("$top".into(), (requested - take).to_string());
        }
        let query = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(opts.iter())
            .finish();
        out.push_str(&format!(
            "<link rel=\"next\" href=\"{}/api/v2/{}?{}\"/>",
            escape(&state.config.public_url),
            escape(path),
            escape(&query)
        ));
    }
    out.push_str("</feed>");
    Ok(xml(out, "application/atom+xml"))
}
