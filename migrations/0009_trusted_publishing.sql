CREATE TABLE trusted_publishers (
    id UUID PRIMARY KEY, profile_id UUID NOT NULL REFERENCES profiles(id),
    name TEXT NOT NULL, repository TEXT NOT NULL, repository_id TEXT NOT NULL,
    owner_id TEXT NOT NULL, workflow TEXT NOT NULL, git_ref TEXT NOT NULL,
    environment TEXT, package_prefix TEXT NOT NULL,
    feed_id UUID REFERENCES feeds(id), enabled BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE trusted_oidc_keys (
    singleton BOOLEAN PRIMARY KEY DEFAULT true CHECK(singleton),
    document JSONB, fetched_at TIMESTAMPTZ, last_attempt TIMESTAMPTZ
);
INSERT INTO trusted_oidc_keys(singleton) VALUES(true);
CREATE TABLE trusted_token_uses (
    token_id TEXT PRIMARY KEY, expires_at TIMESTAMPTZ NOT NULL
);
ALTER TABLE api_keys ADD COLUMN bound_feed BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE api_keys ADD COLUMN feed_id UUID REFERENCES feeds(id);
ALTER TABLE api_keys ADD COLUMN trusted_publisher_id UUID REFERENCES trusted_publishers(id);
