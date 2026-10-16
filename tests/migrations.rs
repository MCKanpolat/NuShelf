use sqlx::{PgPool, migrate::Migrator, postgres::PgPoolOptions};
use uuid::Uuid;
async fn historical(pool: &PgPool, folder: &str, max: u32) {
    let dir = tempfile::tempdir().unwrap();
    for entry in std::fs::read_dir(format!("{}/{folder}", env!("CARGO_MANIFEST_DIR"))).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        let version = name
            .to_str()
            .unwrap()
            .split('_')
            .next()
            .unwrap()
            .parse::<u32>()
            .unwrap();
        if version <= max {
            std::fs::copy(entry.path(), dir.path().join(name)).unwrap();
        }
    }
    Migrator::new(dir.path())
        .await
        .unwrap()
        .run(pool)
        .await
        .unwrap();
}
async fn seed(pool: &PgPool, id: &str, profile: Uuid) {
    sqlx::query("INSERT INTO packages(id,display_id,owner_id) VALUES($1,$1,$2)")
        .bind(id)
        .bind(profile)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO package_versions(package_id,version,display_version,version_sort,prerelease,semver2,title,description,authors,tags,nuspec,storage_key,size,sha512) VALUES($1,'1.0.0','1.0.0','1',false,false,'','Before catalog','Team','',decode('','hex'),$1,1,'hash')").bind(id).execute(pool).await.unwrap();
}
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn root_and_existing_feed_upgrade_bootstrap_without_duplicate_history() {
    let database = std::env::var("TEST_DATABASE_URL").unwrap();
    let root = PgPool::connect(&database).await.unwrap();
    let name = format!("upgrade_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&root)
        .await
        .unwrap();
    let mut url = url::Url::parse(&database).unwrap();
    url.set_path(&format!("/{name}"));
    let db = PgPool::connect(url.as_str()).await.unwrap();
    historical(&db, "migrations", 10).await;
    let profile = Uuid::new_v4();
    sqlx::query("INSERT INTO profiles(id,name) VALUES($1,'Existing owner')")
        .bind(profile)
        .execute(&db)
        .await
        .unwrap();
    seed(&db, "root.existing", profile).await;
    let feed = Uuid::new_v4();
    let schema = format!("feed_{}", feed.simple());
    sqlx::raw_sql(&format!("CREATE SCHEMA {schema}; CREATE VIEW {schema}.profiles AS SELECT * FROM public.profiles; CREATE VIEW {schema}.api_keys AS SELECT * FROM public.api_keys;")).execute(&db).await.unwrap();
    let schema_copy = schema.clone();
    let scoped = PgPoolOptions::new()
        .after_connect(move |conn, _| {
            let sql = format!("SET search_path TO {schema_copy},public");
            Box::pin(async move {
                sqlx::query(&sql).execute(conn).await?;
                Ok(())
            })
        })
        .connect(url.as_str())
        .await
        .unwrap();
    let initial = include_str!("../migrations/0001_registry.sql");
    let initial = &initial[initial.find("CREATE TABLE packages (").unwrap()..];
    sqlx::raw_sql(&initial.replace("REFERENCES profiles(id)", "REFERENCES public.profiles(id)"))
        .execute(&scoped)
        .await
        .unwrap();
    sqlx::raw_sql(
        &include_str!("../migrations/0002_upstreams.sql")
            .replace("REFERENCES profiles(id)", "REFERENCES public.profiles(id)"),
    )
    .execute(&scoped)
    .await
    .unwrap();
    historical(&scoped, "feed-migrations", 5).await;
    seed(&scoped, "feed.existing", profile).await;
    sqlx::migrate!("./migrations").run(&db).await.unwrap();
    sqlx::migrate!("./feed-migrations")
        .run(&scoped)
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&db).await.unwrap();
    sqlx::migrate!("./feed-migrations")
        .run(&scoped)
        .await
        .unwrap();
    for (pool, id) in [(&db, "root.existing"), (&scoped, "feed.existing")] {
        let rows: Vec<(String, serde_json::Value)> =
            sqlx::query_as("SELECT package_id,payload FROM catalog_events")
                .fetch_all(pool)
                .await
                .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, id);
        assert_eq!(rows[0].1["description"], "Before catalog");
        let mirrors:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema=current_schema() AND table_name='upstream_cache' AND column_name='mirrored')").fetch_one(pool).await.unwrap();
        assert!(mirrors);
    }
    scoped.close().await;
    db.close().await;
    sqlx::query(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .execute(&root)
        .await
        .unwrap();
    root.close().await;
}
