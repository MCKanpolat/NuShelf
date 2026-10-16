mod common;
use common::*;
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn legacy_feed_queries_and_private_scope() {
    let s = Server::start().await;
    let c = &s.client;
    let b = &s.base;
    for v in ["1.0.0", "1.2.0", "2.0.0", "3.0.0-beta", "4.0.0-beta.1"] {
        s.push("Example.Json", v).await;
    }
    for p in ["/api/v2", "/api/v2/", "/api/v2/$metadata"] {
        xml(c.get(format!("{b}{p}")).send().await.unwrap()).await;
    }
    let feed = xml(c
        .get(format!("{b}/api/v2/FindPackagesById()"))
        .query(&[("id", "'example.json'")])
        .send()
        .await
        .unwrap())
    .await;
    assert_eq!(properties(&feed, "Version").len(), 4);
    assert!(feed.contains("JSON parser &amp; helpers"));
    let feed = xml(c
        .get(format!("{b}/api/v2/Packages"))
        .query(&[
            (
                "$filter",
                "IsLatestVersion and substringof('json',tolower(Id))",
            ),
            ("$inlinecount", "allpages"),
        ])
        .send()
        .await
        .unwrap())
    .await;
    assert_eq!(properties(&feed, "Version"), ["2.0.0"]);
    assert_eq!(properties(&feed, "count"), ["1"]);
    let feed = xml(c
        .get(format!("{b}/api/v2/Packages"))
        .query(&[
            ("$orderby", "Version desc,Id asc"),
            ("$skip", "1"),
            ("$top", "1"),
            ("$select", "Id,Version"),
        ])
        .send()
        .await
        .unwrap())
    .await;
    assert_eq!(properties(&feed, "Version"), ["2.0.0"]);
    assert!(properties(&feed, "Description").is_empty());
    let count = c
        .get(format!("{b}/api/v2/Packages/$count"))
        .query(&[(
            "$filter",
            "DownloadCount ge 0 and Published gt datetime'2000-01-01T00:00:00Z'",
        )])
        .send()
        .await
        .unwrap();
    assert_eq!(count.text().await.unwrap(), "4");
    let feed = xml(c
        .get(format!("{b}/api/v2/GetUpdates"))
        .query(&[
            ("packageIds", "'Example.Json'"),
            ("versions", "'1.0.0'"),
            ("versionConstraints", "'[1.0.0,2.0.0)'"),
            ("includeAllVersions", "false"),
        ])
        .send()
        .await
        .unwrap())
    .await;
    assert_eq!(properties(&feed, "Version"), ["1.2.0"]);
    let entity = xml(c
        .get(format!(
            "{b}/api/v2/Packages(Id='Example.Json',Version='1.0')"
        ))
        .send()
        .await
        .unwrap())
    .await;
    assert_eq!(properties(&entity, "Version"), ["1.0.0"]);
    let download = c
        .get(format!("{b}/api/v2/package/EXAMPLE.JSON/1.0"))
        .send()
        .await
        .unwrap();
    assert_eq!(download.status(), 200);
    assert!(download.bytes().await.unwrap().starts_with(b"PK"));
    assert_eq!(
        c.head(format!("{b}/api/v2/package/Example.Json/1.0.0"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    for filter in [
        "Id eq 'x';DROP TABLE packages",
        "Unknown eq 'x'",
        "Id eq 'x' or 1 eq 1 --",
    ] {
        assert_eq!(
            c.get(format!("{b}/api/v2/Packages"))
                .query(&[("$filter", filter)])
                .send()
                .await
                .unwrap()
                .status(),
            400
        );
    }
    // Server-driven paging must preserve the query and stop at the requested total.
    sqlx::raw_sql("INSERT INTO packages(id,display_id,owner_id) SELECT 'page.'||n,'Page.'||n,(SELECT id FROM profiles LIMIT 1) FROM generate_series(1,101) n; INSERT INTO package_versions(package_id,version,display_version,version_sort,prerelease,semver2,title,description,authors,tags,nuspec,storage_key,size,sha512) SELECT id,'1.0.0','1.0.0','1',false,false,'','','','',decode('','hex'),id,1,'' FROM packages WHERE id LIKE 'page.%'").execute(&s.state.db).await.unwrap();
    let feed = xml(c
        .get(format!("{b}/api/v2/Packages"))
        .query(&[("$filter", "startswith(Id,'Page.')"), ("$top", "101")])
        .send()
        .await
        .unwrap())
    .await;
    assert_eq!(properties(&feed, "Version").len(), 100);
    let next = {
        let d = roxmltree::Document::parse(&feed).unwrap();
        d.descendants()
            .find(|n| n.attribute("rel") == Some("next"))
            .unwrap()
            .attribute("href")
            .unwrap()
            .to_string()
    };
    let feed = xml(c.get(next).send().await.unwrap()).await;
    assert_eq!(properties(&feed, "Version").len(), 1);
    let feed = response(
        c.post(format!("{b}/api/feeds"))
            .bearer_auth(ADMIN)
            .json(&serde_json::json!({"name":"secret"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert!(feed["private"].as_bool().unwrap());
    for path in [
        "Packages",
        "$metadata",
        "Packages/$count",
        "package/example.json/1.0.0",
    ] {
        assert_eq!(
            c.get(format!("{b}/feeds/secret/api/v2/{path}"))
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
    }
    let private = xml(c
        .get(format!("{b}/feeds/secret/api/v2/Packages"))
        .bearer_auth(ADMIN)
        .send()
        .await
        .unwrap())
    .await;
    assert!(properties(&private, "Version").is_empty());
    s.stop().await;
}
