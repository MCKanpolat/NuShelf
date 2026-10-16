mod common;
use common::*;
use serde_json::{Value, json};
use uuid::Uuid;
async fn catalog(s: &Server) -> Value {
    response(
        s.client
            .get(format!("{}/v3/catalog/index.json", s.base))
            .send()
            .await
            .unwrap(),
    )
    .await
}
async fn page(s: &Server, index: &Value, number: usize) -> Value {
    response(
        s.client
            .get(index["items"][number]["@id"].as_str().unwrap())
            .send()
            .await
            .unwrap(),
    )
    .await
}
async fn leaf(s: &Server, page: &Value, number: usize) -> Value {
    response(
        s.client
            .get(page["items"][number]["@id"].as_str().unwrap())
            .send()
            .await
            .unwrap(),
    )
    .await
}
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn catalog_is_transactional_ordered_immutable_and_scoped() {
    let s = Server::start().await;
    let empty = catalog(&s).await;
    assert_eq!(empty["count"], 0);
    assert!(empty["commitId"].is_string());
    s.push("Catalog.Local", "1.0.0+build.1").await;
    let first_index = catalog(&s).await;
    let first_page = page(&s, &first_index, 0).await;
    let first = leaf(&s, &first_page, 0).await;
    assert_eq!(first["@type"][0], "PackageDetails");
    assert_eq!(first["version"], "1.0.0+build.1");
    assert_eq!(first["packageHashAlgorithm"], "SHA512");
    assert!(first["packageSize"].as_i64().unwrap() > 0);
    assert!(
        first["packageContent"]
            .as_str()
            .unwrap()
            .ends_with("catalog.local.1.0.0.nupkg")
    );
    let unlist = format!("{}/api/v2/package/Catalog.Local/1.0.0", s.base);
    assert_eq!(
        s.client
            .delete(&unlist)
            .header("X-NuGet-ApiKey", &s.key)
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    assert_eq!(
        s.client
            .delete(&unlist)
            .header("X-NuGet-ApiKey", &s.key)
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    let index = catalog(&s).await;
    let p = page(&s, &index, 0).await;
    assert_eq!(p["count"], 2);
    let unlisted = leaf(&s, &p, 1).await;
    assert_eq!(unlisted["listed"], false);
    assert!(
        unlisted["published"]
            .as_str()
            .unwrap()
            .starts_with("1900-01-01")
    );
    assert_eq!(
        s.client
            .post(&unlist)
            .header("X-NuGet-ApiKey", &s.key)
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    assert_eq!(
        s.client
            .put(format!(
                "{}/api/packages/Catalog.Local/1.0.0/deprecation",
                s.base
            ))
            .header("X-NuGet-ApiKey", &s.key)
            .json(&json!({"reasons":["Legacy"],"message":"Use a newer version"}))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    assert_eq!(
        s.client
            .delete(format!(
                "{}/api/packages/Catalog.Local/1.0.0/deprecation",
                s.base
            ))
            .header("X-NuGet-ApiKey", &s.key)
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    let index = catalog(&s).await;
    let p = page(&s, &index, 0).await;
    assert_eq!(p["count"], 5);
    assert_eq!(leaf(&s, &p, 3).await["deprecation"]["reasons"][0], "Legacy");
    assert!(leaf(&s, &p, 4).await.get("deprecation").is_none());
    assert_eq!(
        leaf(&s, &p, 0).await,
        first,
        "Past snapshots must never change"
    );
    let stamps = p["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            chrono::DateTime::parse_from_rfc3339(v["commitTimeStamp"].as_str().unwrap()).unwrap()
        })
        .collect::<Vec<_>>();
    assert!(stamps.windows(2).all(|p| p[0] < p[1]));
    // A publication and its event roll back together, including the monotonic head.
    let mut tx = s.state.db.begin().await.unwrap();
    sqlx::query("UPDATE package_versions SET listed=false WHERE package_id='catalog.local'")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(catalog(&s).await, index);
    let changed = sqlx::query("UPDATE catalog_events SET event_type='PackageDelete'")
        .execute(&s.state.db)
        .await;
    assert!(changed.is_err());
    let r = s
        .client
        .get(p["items"][0]["@id"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    let etag = r.headers()["etag"].clone();
    assert_eq!(
        s.client
            .get(p["items"][0]["@id"].as_str().unwrap())
            .header("If-None-Match", etag)
            .send()
            .await
            .unwrap()
            .status(),
        304
    );
    assert_eq!(
        s.client
            .head(p["items"][0]["@id"].as_str().unwrap())
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap()
            .len(),
        0
    );
    // Concurrent publishers still yield strictly ordered, independently committed events.
    let ((), ()) = tokio::join!(
        s.push("Concurrent.A", "1.0.0"),
        s.push("Concurrent.B", "1.0.0")
    );
    let seq: Vec<(i64, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as("SELECT sequence,committed_at FROM catalog_events ORDER BY sequence")
            .fetch_all(&s.state.db)
            .await
            .unwrap();
    assert!(seq.windows(2).all(|p| p[0].1 < p[1].1));
    let f = response(
        s.client
            .post(format!("{}/api/feeds", s.base))
            .bearer_auth(ADMIN)
            .json(&json!({"name":"catalog-private"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    let url = format!("{}/feeds/catalog-private/v3/catalog/index.json", s.base);
    assert_eq!(s.client.get(&url).send().await.unwrap().status(), 401);
    assert_eq!(
        response(s.client.get(&url).bearer_auth(ADMIN).send().await.unwrap()).await["count"],
        0
    );
    let root_event = p["items"][0]["@id"]
        .as_str()
        .unwrap()
        .split('/')
        .next_back()
        .unwrap();
    assert_eq!(
        s.client
            .get(format!(
                "{}/feeds/catalog-private/v3/catalog/leaves/{root_event}",
                s.base
            ))
            .bearer_auth(ADMIN)
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let _ = f;
    // Durable page boundaries remain unchanged when the next page is introduced.
    sqlx::query("INSERT INTO packages(id,display_id,owner_id) SELECT 'page.'||n,'Page.'||n,(SELECT id FROM profiles LIMIT 1) FROM generate_series(1,500) n").execute(&s.state.db).await.unwrap();
    sqlx::query("INSERT INTO package_versions(package_id,version,display_version,version_sort,prerelease,semver2,title,description,authors,tags,nuspec,storage_key,size,sha512) SELECT id,'1.0.0','1.0.0','1',false,false,'','','','',decode('','hex'),id,1,'' FROM packages WHERE id LIKE 'page.%'").execute(&s.state.db).await.unwrap();
    let index = catalog(&s).await;
    assert_eq!(index["count"], 2);
    let sealed = page(&s, &index, 0).await;
    assert!((1..=500).contains(&sealed["count"].as_i64().unwrap()));
    s.push("More.Events", "1.0.0").await;
    assert_eq!(page(&s, &catalog(&s).await, 0).await, sealed);
    // Privileged hard deletes produce tombstones; ordinary NuGet DELETE only unlists.
    sqlx::query("DELETE FROM search_documents WHERE package_id='catalog.local'")
        .execute(&s.state.db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM package_versions WHERE package_id='catalog.local'")
        .execute(&s.state.db)
        .await
        .unwrap();
    let newest: Uuid =
        sqlx::query_scalar("SELECT event_id FROM catalog_events ORDER BY sequence DESC LIMIT 1")
            .fetch_one(&s.state.db)
            .await
            .unwrap();
    let deletion = response(
        s.client
            .get(format!("{}/v3/catalog/leaves/{newest}.json", s.base))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(deletion["@type"][0], "PackageDelete");
    assert!(deletion.get("packageHash").is_none());
    s.stop().await;
}
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn catalog_drives_another_nushelf_mirror() {
    let origin = Server::start().await;
    let target = Server::start().await;
    origin.push("Chained.Package", "1.0.0").await;
    let source=response(target.client.post(format!("{}/api/upstreams",target.base)).bearer_auth(ADMIN).json(&json!({"name":"NuShelf catalog","url":format!("{}/v3/index.json",origin.base),"allowHttp":true,"allowPrivateNetwork":true})).send().await.unwrap()).await;
    let id = Uuid::parse_str(source["id"].as_str().unwrap()).unwrap();
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
    for _ in 0..20 {
        if nushelf::upstream::mirror::step(&target.state, id)
            .await
            .unwrap()
            == nushelf::upstream::mirror::Progress::Idle
        {
            break;
        }
    }
    let version: Option<String> = sqlx::query_scalar(
        "SELECT version FROM upstream_cache WHERE package_id='chained.package' AND mirrored",
    )
    .fetch_optional(&target.state.db)
    .await
    .unwrap();
    assert_eq!(version.as_deref(), Some("1.0.0"));
    origin.stop().await;
    target.stop().await;
}
