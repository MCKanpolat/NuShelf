CREATE TABLE package_ownership_transfers (
    id UUID PRIMARY KEY,
    package_id TEXT NOT NULL REFERENCES packages(id),
    source_profile UUID NOT NULL REFERENCES public.profiles(id),
    target_profile UUID NOT NULL REFERENCES public.profiles(id),
    status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','accepted','cancelled','expired')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT now()+interval '7 days',
    completed_at TIMESTAMPTZ,
    CHECK(source_profile<>target_profile)
);
CREATE UNIQUE INDEX ownership_pending_idx ON package_ownership_transfers(package_id) WHERE status='pending';
CREATE INDEX ownership_recipient_idx ON package_ownership_transfers(target_profile,created_at DESC);
