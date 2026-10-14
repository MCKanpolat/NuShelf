CREATE TABLE feeds (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE CHECK(name ~ '^[a-z][a-z0-9-]{0,39}$'),
    private BOOLEAN NOT NULL DEFAULT true,
    enabled BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE feed_members (
    feed_id UUID NOT NULL REFERENCES feeds(id),
    profile_id UUID NOT NULL REFERENCES profiles(id),
    role TEXT NOT NULL CHECK(role IN ('reader','publisher','administrator')),
    PRIMARY KEY(feed_id,profile_id)
);
