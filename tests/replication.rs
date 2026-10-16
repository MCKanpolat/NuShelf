mod common;
use common::*;
use nushelf::upstream::mirror::{Progress, step};
use serde_json::json;
use uuid::Uuid;
async fn source(s: &Server, name: &str, url: &str, private: bool) -> Uuid {
    Uuid::parse_str(response(s.client.post(format!("{}/api/upstreams",s.base)).bearer_auth(ADMIN).json(&json!({"name":name,"url":url,"allowHttp":true,"allowPrivateNetwork":true,"private":private,"patterns":["*"],"metadataTtlSeconds":1})).send().await.unwrap()).await["id"].as_str().unwrap()).unwrap()
}
async fn run(s: &Server, id: Uuid) {
    response(
        s.client
            .post(format!("{}/api/upstreams/{id}/mirror/run", s.base))
            .bearer_auth(ADMIN)
            .send()
            .await
            .unwrap(),
    )
    .await;
    for _ in 0..100 {
        let result = step(&s.state, id).await.unwrap();
        if result == Progress::Idle {
            return;
        }
        if result == Progress::Busy {
            continue;
        }
    }
    panic!("Mirror did not finish");
}
async fn search_only_index(origin: &Server) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/index.json", listener.local_addr().unwrap());
    let mut index = nushelf::registry::service_index(axum::extract::State(origin.state.clone()))
        .await
        .unwrap()
        .0;
    index["resources"]
        .as_array_mut()
        .unwrap()
        .retain(|r| r["@type"] != "Catalog/3.0.0");
    let app = axum::Router::new().route(
        "/index.json",
        axum::routing::get(move || {
            let index = index.clone();
            async move { axum::Json(index) }
        }),
    );
    (
        url,
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() }),
    )
}
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn full_search_mirror_and_live_federation() {
    let origin = Server::start().await;
    origin.push("Remote.Json", "1.0.0").await;
    origin.push("Remote.Json", "2.0.0").await;
    let r = origin
        .client
        .delete(format!("{}/api/v2/package/Remote.Json/2.0.0", origin.base))
        .header("X-NuGet-ApiKey", &origin.key)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    let second = Server::start().await;
    second.push("Partner.Json", "1.0.0").await;
    let (search_url, search_task) = search_only_index(&origin).await;
    let destination = Server::start().await;
    let id = source(&destination, "first", &search_url, false).await;
    let private = source(
        &destination,
        "private",
        &format!("{}/v3/index.json", second.base),
        true,
    )
    .await;
    let url = format!("{}/api/upstreams/search", destination.base);
    let found = response(
        destination
            .client
            .get(&url)
            .query(&[("q", "json")])
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(found["sources"].as_array().unwrap().len(), 1);
    assert_eq!(found["sources"][0]["data"][0]["id"], "Remote.Json");
    let found = response(
        destination
            .client
            .get(&url)
            .bearer_auth(ADMIN)
            .query(&[("q", "json")])
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(found["sources"].as_array().unwrap().len(), 2);
    assert!(!found["partial"].as_bool().unwrap());
    assert_eq!(
        found["sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["sourceId"] == private.to_string())
            .unwrap()["data"][0]["id"],
        "Partner.Json"
    );
    // Searches discover metadata without silently pinning IDs or downloading archives.
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM upstream_metadata")
        .fetch_one(&destination.state.db)
        .await
        .unwrap();
    assert_eq!(count, 0);
    run(&destination, id).await;
    let rows:Vec<(String,bool,bool)>=sqlx::query_as("SELECT version,mirrored,(catalog->>'listed')::boolean FROM upstream_cache ORDER BY version_sort").fetch_all(&destination.state.db).await.unwrap();
    assert_eq!(
        rows,
        vec![("1.0.0".into(), true, true), ("2.0.0".into(), true, false)]
    );
    let status = response(
        destination
            .client
            .get(format!("{}/api/upstreams/{id}/mirror", destination.base))
            .bearer_auth(ADMIN)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status["status"], "completed");
    sqlx::raw_sql("UPDATE upstream_cache SET cached_at=now()-interval '10 days',last_accessed_at=now()-interval '10 days'; UPDATE cache_policy SET enabled=true,idle_days=1,max_bytes=1").execute(&destination.state.db).await.unwrap();
    let result = nushelf::maintenance::execute(&destination.state, false, false)
        .await
        .unwrap();
    assert!(result["selected"].as_array().unwrap().is_empty());
    let settings = json!({"enabled":true,"intervalSeconds":60});
    response(
        destination
            .client
            .patch(format!("{}/api/upstreams/{id}/mirror", destination.base))
            .bearer_auth(ADMIN)
            .json(&settings)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        destination
            .client
            .delete(format!("{}/api/upstreams/{id}/cache", destination.base))
            .bearer_auth(ADMIN)
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    response(
        destination
            .client
            .post(format!(
                "{}/api/upstreams/{id}/mirror/cancel",
                destination.base
            ))
            .bearer_auth(ADMIN)
            .send()
            .await
            .unwrap(),
    )
    .await;
    // Exact upstream metadata is also available through V2 before archive download.
    let metadata = xml(destination
        .client
        .get(format!("{}/api/v2/FindPackagesById", destination.base))
        .bearer_auth(ADMIN)
        .query(&[("id", "'Partner.Json'")])
        .send()
        .await
        .unwrap())
    .await;
    assert_eq!(properties(&metadata, "Version"), ["1.0.0"]);
    origin.stop().await;
    let download = destination
        .client
        .get(format!(
            "{}/v3/flatcontainer/remote.json/2.0.0/remote.json.2.0.0.nupkg",
            destination.base
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(download.status(), 200);
    let found = response(
        destination
            .client
            .get(&url)
            .query(&[("q", "json")])
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(found["partial"], true);
    assert!(found["sources"][0]["error"].is_string());
    // An unavailable source must fail discovery, not report a completed mirror based on stale metadata.
    run(&destination, id).await;
    let status = response(
        destination
            .client
            .get(format!("{}/api/upstreams/{id}/mirror", destination.base))
            .bearer_auth(ADMIN)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status["status"], "failed");
    search_task.abort();
    second.stop().await;
    destination.stop().await;
    let _ = private;
}
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn catalog_mirror_checkpoints_changes_and_deletes() {
    let origin = Server::start().await;
    origin.push("Catalog.Json", "1.0.0").await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let events = std::sync::Arc::new(tokio::sync::RwLock::new(vec![
        json!({"@type":"nuget:PackageDetails","nuget:id":"Catalog.Json","nuget:version":"1.0.0","commitTimeStamp":"2026-01-01T00:00:00Z"}),
    ]));
    let router=axum::Router::new().route("/index.json",axum::routing::get({let base=base.clone();let origin=origin.base.clone();move||{let base=base.clone();let origin=origin.clone();async move{axum::Json(json!({"version":"3.0.0","resources":[{"@id":format!("{origin}/v3/flatcontainer/"),"@type":"PackageBaseAddress/3.0.0"},{"@id":format!("{origin}/v3/registration/"),"@type":"RegistrationsBaseUrl/3.6.0"},{"@id":format!("{base}/catalog/index.json"),"@type":"Catalog/3.0.0"}]}))}}})).route("/catalog/index.json",axum::routing::get({let base=base.clone();let events=events.clone();move||{let base=base.clone();let events=events.clone();async move{let events=events.read().await;axum::Json(json!({"items":events.iter().enumerate().map(|(i,e)|json!({"@id":format!("{base}/catalog/page{i}.json"),"commitTimeStamp":e["commitTimeStamp"]})).collect::<Vec<_>>()}))}}})).route("/catalog/{page}",axum::routing::get({let events=events.clone();move|axum::extract::Path(page):axum::extract::Path<String>|{let events=events.clone();async move{let i:usize=page.strip_prefix("page").unwrap().strip_suffix(".json").unwrap().parse().unwrap();let e=events.read().await[i].clone();axum::Json(json!({"commitTimeStamp":e["commitTimeStamp"],"items":[e]}))}}}));
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let target = Server::start().await;
    let id = source(&target, "catalog", &format!("{base}/index.json"), false).await;
    response(
        target
            .client
            .post(format!("{}/api/upstreams/{id}/mirror/run", target.base))
            .bearer_auth(ADMIN)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(step(&target.state, id).await.unwrap(), Progress::Advanced);
    // A fresh AppState resumes the persisted discovery page instead of starting over.
    let resumed = target.state.clone();
    for _ in 0..10 {
        if step(&resumed, id).await.unwrap() == Progress::Idle {
            break;
        }
    }
    let bytes: i64 = sqlx::query_scalar("SELECT count(*) FROM upstream_cache WHERE mirrored")
        .fetch_one(&target.state.db)
        .await
        .unwrap();
    assert_eq!(bytes, 1);
    events.write().await.push(json!({"@type":"nuget:PackageDelete","nuget:id":"Catalog.Json","nuget:version":"1.0.0","commitTimeStamp":"2026-01-02T00:00:00Z"}));
    run(&target, id).await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM upstream_cache")
        .fetch_one(&target.state.db)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let clean: i64 = sqlx::query_scalar("SELECT count(*) FROM storage_cleanup")
        .fetch_one(&target.state.db)
        .await
        .unwrap();
    assert_eq!(clean, 1);
    let status = response(
        target
            .client
            .get(format!("{}/api/upstreams/{id}/mirror", target.base))
            .bearer_auth(ADMIN)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status["cursor"]["timestamp"], "2026-01-02T00:00:00Z");
    task.abort();
    origin.stop().await;
    target.stop().await;
}
