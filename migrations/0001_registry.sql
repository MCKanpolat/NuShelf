CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE TABLE profiles (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE CHECK (length(name) BETWEEN 1 AND 80),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE api_keys (
    id UUID PRIMARY KEY,
    profile_id UUID NOT NULL REFERENCES profiles(id),
    label TEXT NOT NULL,
    key_hash TEXT NOT NULL UNIQUE,
    key_hint TEXT NOT NULL,
    scopes TEXT[] NOT NULL,
    package_prefix TEXT NOT NULL DEFAULT '',
    expires_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX api_keys_profile_idx ON api_keys(profile_id);

CREATE TABLE packages (
    id TEXT PRIMARY KEY,
    display_id TEXT NOT NULL,
    owner_id UUID NOT NULL REFERENCES profiles(id),
    downloads BIGINT NOT NULL DEFAULT 0 CHECK (downloads >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX packages_owner_idx ON packages(owner_id);
CREATE TABLE package_versions (
    package_id TEXT NOT NULL REFERENCES packages(id),
    version TEXT NOT NULL,
    display_version TEXT NOT NULL,
    version_sort TEXT COLLATE "C" NOT NULL,
    prerelease BOOLEAN NOT NULL,
    semver2 BOOLEAN NOT NULL,
    listed BOOLEAN NOT NULL DEFAULT true,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    authors TEXT NOT NULL,
    tags TEXT NOT NULL,
    project_url TEXT,
    license_expression TEXT,
    dependencies JSONB NOT NULL DEFAULT '[]',
    nuspec BYTEA NOT NULL,
    storage_key TEXT NOT NULL UNIQUE,
    size BIGINT NOT NULL CHECK (size > 0),
    sha512 TEXT NOT NULL,
    downloads BIGINT NOT NULL DEFAULT 0,
    published_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY(package_id, version)
);
CREATE INDEX versions_latest_idx ON package_versions(package_id, version_sort DESC) WHERE listed;

-- Four search channels: stable/all prereleases, with/without SemVer 2.
-- One document per package/channel keeps search independent of version history size.
CREATE TABLE search_documents (
    package_id TEXT NOT NULL REFERENCES packages(id),
    channel SMALLINT NOT NULL,
    version TEXT NOT NULL,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    authors TEXT NOT NULL,
    tags TEXT NOT NULL,
    published_at TIMESTAMPTZ NOT NULL,
    search_vector TSVECTOR GENERATED ALWAYS AS (
        setweight(to_tsvector('simple', package_id || ' ' || title), 'A') ||
        setweight(to_tsvector('simple', tags || ' ' || authors), 'B') ||
        setweight(to_tsvector('simple', description), 'C')
    ) STORED,
    PRIMARY KEY(package_id, channel),
    FOREIGN KEY(package_id, version) REFERENCES package_versions(package_id, version)
);
CREATE INDEX search_fulltext_idx ON search_documents USING GIN(search_vector);
CREATE INDEX search_trigram_idx ON search_documents USING GIN(package_id gin_trgm_ops);
CREATE INDEX search_prefix_idx ON search_documents(package_id text_pattern_ops);
CREATE INDEX search_channel_recent_idx ON search_documents(channel, published_at DESC, package_id);

CREATE FUNCTION refresh_search(target TEXT) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM search_documents WHERE package_id = target;
    INSERT INTO search_documents(package_id, channel, version, title, description, authors, tags, published_at)
    SELECT DISTINCT ON (c.channel) v.package_id, c.channel, v.version, v.title, v.description, v.authors, v.tags, v.published_at
    FROM package_versions v
    CROSS JOIN (VALUES (0::smallint), (1::smallint), (2::smallint), (3::smallint)) c(channel)
    WHERE v.package_id = target AND v.listed
        AND (c.channel IN (1,3) OR NOT v.prerelease)
        AND (c.channel IN (2,3) OR NOT v.semver2)
    ORDER BY c.channel, v.version_sort DESC;
END;
$$;
