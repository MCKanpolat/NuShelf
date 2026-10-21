CREATE TABLE inventory_applications (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 100),
    owner_id UUID REFERENCES public.profiles(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE UNIQUE INDEX inventory_applications_name ON inventory_applications(lower(name));
CREATE INDEX inventory_applications_owner ON inventory_applications(owner_id);
CREATE TABLE inventory_releases (
    id UUID PRIMARY KEY,
    application_id UUID NOT NULL REFERENCES inventory_applications(id) ON DELETE CASCADE,
    version TEXT NOT NULL CHECK(length(version) BETWEEN 1 AND 100),
    repository_url TEXT,
    commit_sha TEXT,
    sbom_hash TEXT NOT NULL,
    document JSONB NOT NULL,
    root_ref TEXT,
    component_count INTEGER NOT NULL,
    nuget_count INTEGER NOT NULL,
    edge_count INTEGER NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE(application_id,version)
);
CREATE INDEX inventory_releases_recent ON inventory_releases(application_id,created_at DESC,id DESC);
CREATE TABLE inventory_components (
    release_id UUID NOT NULL REFERENCES inventory_releases(id) ON DELETE CASCADE,
    bom_ref TEXT NOT NULL,
    name TEXT NOT NULL,
    purl TEXT,
    package_id TEXT,
    version TEXT,
    PRIMARY KEY(release_id,bom_ref)
);
CREATE INDEX inventory_components_package ON inventory_components(package_id,version,release_id) WHERE package_id IS NOT NULL;
CREATE TABLE inventory_edges (
    release_id UUID NOT NULL,
    parent_ref TEXT NOT NULL,
    child_ref TEXT NOT NULL,
    PRIMARY KEY(release_id,parent_ref,child_ref),
    FOREIGN KEY(release_id,parent_ref) REFERENCES inventory_components(release_id,bom_ref) ON DELETE CASCADE,
    FOREIGN KEY(release_id,child_ref) REFERENCES inventory_components(release_id,bom_ref) ON DELETE CASCADE
);
