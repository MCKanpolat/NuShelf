CREATE TABLE upstream_sources (
    id UUID PRIMARY KEY, name TEXT NOT NULL UNIQUE, url TEXT NOT NULL,
    priority INTEGER NOT NULL DEFAULT 100, enabled BOOLEAN NOT NULL DEFAULT true,
    patterns TEXT[] NOT NULL DEFAULT ARRAY['*'], excludes TEXT[] NOT NULL DEFAULT ARRAY[]::text[],
    private BOOLEAN NOT NULL DEFAULT false,
    auth_type TEXT NOT NULL DEFAULT 'none' CHECK(auth_type IN ('none','bearer','basic')),
    username TEXT, credential_env TEXT,
    allow_http BOOLEAN NOT NULL DEFAULT false, allow_private_network BOOLEAN NOT NULL DEFAULT false,
    timeout_seconds INTEGER NOT NULL DEFAULT 15 CHECK(timeout_seconds BETWEEN 1 AND 30),
    metadata_ttl_seconds INTEGER NOT NULL DEFAULT 300 CHECK(metadata_ttl_seconds BETWEEN 1 AND 86400),
    stale_on_error BOOLEAN NOT NULL DEFAULT true,
    service_index JSONB, index_checked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE upstream_grants (
    source_id UUID NOT NULL REFERENCES upstream_sources(id) ON DELETE CASCADE,
    profile_id UUID NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    PRIMARY KEY(source_id,profile_id)
);
CREATE TABLE upstream_metadata (
    package_id TEXT PRIMARY KEY, source_id UUID NOT NULL REFERENCES upstream_sources(id),
    versions JSONB NOT NULL, catalog JSONB,
    checked_at TIMESTAMPTZ NOT NULL DEFAULT now(), expires_at TIMESTAMPTZ NOT NULL,
    catalog_checked_at TIMESTAMPTZ
);
CREATE INDEX upstream_metadata_source_idx ON upstream_metadata(source_id);
CREATE TABLE upstream_misses (
    source_id UUID NOT NULL REFERENCES upstream_sources(id) ON DELETE CASCADE,
    package_id TEXT NOT NULL, expires_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY(source_id,package_id)
);
CREATE TABLE upstream_cache (
    package_id TEXT NOT NULL REFERENCES upstream_metadata(package_id) ON DELETE CASCADE,
    version TEXT NOT NULL, version_sort TEXT COLLATE "C" NOT NULL,
    prerelease BOOLEAN NOT NULL, semver2 BOOLEAN NOT NULL,
    title TEXT NOT NULL, description TEXT NOT NULL, authors TEXT NOT NULL, tags TEXT NOT NULL,
    catalog JSONB NOT NULL, nuspec BYTEA NOT NULL, storage_key TEXT NOT NULL UNIQUE,
    size BIGINT NOT NULL, sha512 TEXT NOT NULL, downloads BIGINT NOT NULL DEFAULT 0,
    cached_at TIMESTAMPTZ NOT NULL DEFAULT now(), PRIMARY KEY(package_id,version)
);
CREATE INDEX upstream_cache_versions_idx ON upstream_cache(package_id,version_sort DESC);
CREATE TABLE upstream_search_documents (
    package_id TEXT NOT NULL, channel SMALLINT NOT NULL, version TEXT NOT NULL,
    title TEXT NOT NULL, description TEXT NOT NULL, authors TEXT NOT NULL, tags TEXT NOT NULL,
    cached_at TIMESTAMPTZ NOT NULL,
    search_vector TSVECTOR GENERATED ALWAYS AS (
        setweight(to_tsvector('simple',package_id||' '||title),'A') ||
        setweight(to_tsvector('simple',tags||' '||authors),'B') ||
        setweight(to_tsvector('simple',description),'C')
    ) STORED,
    PRIMARY KEY(package_id,channel),
    FOREIGN KEY(package_id,version) REFERENCES upstream_cache(package_id,version) ON DELETE CASCADE
);
CREATE INDEX upstream_search_fts_idx ON upstream_search_documents USING GIN(search_vector);
CREATE INDEX upstream_search_trigram_idx ON upstream_search_documents USING GIN(package_id gin_trgm_ops);
CREATE INDEX upstream_search_prefix_idx ON upstream_search_documents(package_id text_pattern_ops);
CREATE FUNCTION refresh_upstream_search(target TEXT) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM upstream_search_documents WHERE package_id=target;
    INSERT INTO upstream_search_documents(package_id,channel,version,title,description,authors,tags,cached_at)
    SELECT DISTINCT ON(c.channel) v.package_id,c.channel,v.version,v.title,v.description,v.authors,v.tags,v.cached_at
    FROM upstream_cache v
    CROSS JOIN (VALUES(0::smallint),(1::smallint),(2::smallint),(3::smallint)) c(channel)
    WHERE v.package_id=target AND (c.channel IN (1,3) OR NOT v.prerelease)
        AND (c.channel IN (2,3) OR NOT v.semver2)
        AND COALESCE((v.catalog->>'listed')::boolean,true)
    ORDER BY c.channel,v.version_sort DESC;
END;
$$;
CREATE FUNCTION upstream_pattern_score(pattern TEXT,target TEXT) RETURNS INTEGER LANGUAGE SQL IMMUTABLE AS $$
 SELECT CASE WHEN right(pattern,1)='*' AND left(target,length(pattern)-1)=left(pattern,length(pattern)-1) THEN length(pattern)-1
             WHEN pattern=target THEN 1000+length(pattern) ELSE -1 END;
$$;
CREATE FUNCTION upstream_route_allowed(target TEXT,source UUID) RETURNS BOOLEAN LANGUAGE SQL STABLE AS $$
 WITH matching AS (
   SELECT s.id,(SELECT max(upstream_pattern_score(p,target)) FROM unnest(s.patterns) p) AS specificity
   FROM upstream_sources s WHERE s.enabled
     AND NOT EXISTS(SELECT 1 FROM unnest(s.excludes) p WHERE upstream_pattern_score(p,target)>=0)
 ) SELECT EXISTS(SELECT 1 FROM matching WHERE id=source AND specificity>=0 AND specificity=(SELECT max(specificity) FROM matching));
$$;
