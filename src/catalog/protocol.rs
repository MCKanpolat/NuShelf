use super::model::{Event, PageSummary};
use crate::AppState;
use serde_json::{Value, json};
fn context() -> Value {
    json!({"@vocab":"http://schema.nuget.org/schema#","catalog":"http://schema.nuget.org/catalog#","nuget":"http://schema.nuget.org/schema#","commitTimeStamp":{"@id":"catalog:commitTimeStamp","@type":"http://www.w3.org/2001/XMLSchema#dateTime"},"items":{"@id":"catalog:item","@container":"@set"}})
}
fn root(state: &AppState) -> String {
    format!("{}/v3/catalog/index.json", state.config.public_url)
}
fn leaf_url(state: &AppState, event: &Event) -> String {
    format!(
        "{}/v3/catalog/leaves/{}.json",
        state.config.public_url, event.event_id
    )
}
pub(super) fn index(state: &AppState, pages: &[PageSummary]) -> Value {
    let last = pages.last();
    json!({"@id":root(state),"@type":["CatalogRoot","AppendOnlyCatalog","Permalink"],"@context":context(),"commitId":last.map(|p|p.event_id).unwrap_or_else(uuid::Uuid::nil),"commitTimeStamp":last.map(|p|p.committed_at).unwrap_or(chrono::DateTime::UNIX_EPOCH),"count":pages.len(),"items":pages.iter().map(|p|json!({"@id":format!("{}/v3/catalog/pages/{}.json",state.config.public_url,p.page),"@type":"CatalogPage","commitId":p.event_id,"commitTimeStamp":p.committed_at,"count":p.count})).collect::<Vec<_>>()})
}
pub(super) fn page(state: &AppState, number: i64, events: &[Event]) -> Value {
    let last = events.last().expect("handler rejects empty catalog pages");
    json!({"@id":format!("{}/v3/catalog/pages/{number}.json",state.config.public_url),"@type":["CatalogPage","Permalink"],"@context":context(),"parent":root(state),"commitId":last.event_id,"commitTimeStamp":last.committed_at,"count":events.len(),"items":events.iter().map(|e|json!({"@id":leaf_url(state,e),"@type":format!("nuget:{}",e.event_type),"commitId":e.event_id,"commitTimeStamp":e.committed_at,"nuget:id":e.payload["id"].as_str().unwrap_or(&e.package_id),"nuget:version":e.payload["version"].as_str().unwrap_or(&e.version)})).collect::<Vec<_>>()})
}
pub(super) fn leaf(state: &AppState, event: &Event) -> Value {
    let mut payload = event.payload.clone();
    payload["@id"] = json!(leaf_url(state, event));
    payload["@type"] = json!([event.event_type, "catalog:Permalink"]);
    payload["@context"] = context();
    payload["catalog:commitId"] = json!(event.event_id);
    payload["catalog:commitTimeStamp"] = json!(event.committed_at);
    if event.event_type == "PackageDetails" {
        payload["packageContent"] = json!(format!(
            "{}/v3/flatcontainer/{}/{}/{}.{}.nupkg",
            state.config.public_url,
            event.package_id,
            event.version,
            event.package_id,
            event.version
        ));
        if let Some(groups) = payload["dependencyGroups"].as_array_mut() {
            for group in groups {
                if let Some(deps) = group["dependencies"].as_array_mut() {
                    for dep in deps {
                        if let Some(id) = dep["id"].as_str() {
                            dep["registration"] = json!(format!(
                                "{}/v3/registration/{}/index.json",
                                state.config.public_url,
                                id.to_ascii_lowercase()
                            ));
                        }
                    }
                }
            }
        }
    }
    payload
}
