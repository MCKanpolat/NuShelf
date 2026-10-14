CREATE TABLE audit_events (
    id UUID PRIMARY KEY, feed_id UUID REFERENCES feeds(id),
    actor_profile UUID REFERENCES profiles(id), actor_key UUID REFERENCES api_keys(id),
    administrator BOOLEAN NOT NULL DEFAULT false,
    event_type TEXT NOT NULL, method TEXT NOT NULL, path TEXT NOT NULL,
    status INTEGER, target JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(), completed_at TIMESTAMPTZ
);
CREATE INDEX audit_events_recent_idx ON audit_events(created_at DESC,id);
CREATE TABLE webhooks (
    id UUID PRIMARY KEY, name TEXT NOT NULL, url TEXT NOT NULL,
    secret_env TEXT NOT NULL, events TEXT[] NOT NULL DEFAULT ARRAY['*'],
    feed_id UUID REFERENCES feeds(id), enabled BOOLEAN NOT NULL DEFAULT true,
    allow_http BOOLEAN NOT NULL DEFAULT false, allow_private_network BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE webhook_deliveries (
    id UUID PRIMARY KEY, webhook_id UUID NOT NULL REFERENCES webhooks(id),
    event_id UUID NOT NULL REFERENCES audit_events(id),
    attempts INTEGER NOT NULL DEFAULT 0, next_attempt TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ, last_status INTEGER, last_error TEXT,
    UNIQUE(webhook_id,event_id)
);
CREATE INDEX webhook_due_idx ON webhook_deliveries(next_attempt) WHERE completed_at IS NULL;
