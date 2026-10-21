use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationInput {
    pub name: String,
    #[serde(rename = "ownerProfileId")]
    pub owner_profile_id: Option<uuid::Uuid>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseInput {
    pub version: String,
    pub repository_url: Option<String>,
    pub commit_sha: Option<String>,
    pub sbom: Value,
}
pub(super) struct Component {
    pub reference: String,
    pub name: String,
    pub purl: Option<String>,
    pub package_id: Option<String>,
    pub version: Option<String>,
}
pub(super) struct Inventory {
    pub components: Vec<Component>,
    pub edges: Vec<(String, String)>,
    pub root: Option<String>,
}
