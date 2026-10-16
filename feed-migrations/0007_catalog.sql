-- The catalog records authoritative local publication changes. Read-through
-- caches have their own origins; fetching/evicting a cache entry is not a publish.
ALTER TABLE package_versions ADD COLUMN listed_at TIMESTAMPTZ NOT NULL DEFAULT now();
UPDATE package_versions SET listed_at=published_at;
CREATE TABLE catalog_head (
    singleton BOOLEAN PRIMARY KEY DEFAULT true CHECK(singleton),
    committed_at TIMESTAMPTZ NOT NULL DEFAULT '1970-01-01T00:00:00Z'
);
INSERT INTO catalog_head(singleton) VALUES(true);
CREATE TABLE catalog_events (
    sequence BIGSERIAL PRIMARY KEY,
    event_id UUID NOT NULL UNIQUE,
    committed_at TIMESTAMPTZ NOT NULL UNIQUE,
    package_id TEXT NOT NULL,
    version TEXT NOT NULL,
    event_type TEXT NOT NULL CHECK(event_type IN ('PackageDetails','PackageDelete')),
    payload JSONB NOT NULL
);
CREATE INDEX catalog_package_events_idx ON catalog_events(package_id,version,sequence DESC);
CREATE INDEX catalog_prefix_idx ON catalog_events(package_id text_pattern_ops,sequence);
CREATE FUNCTION catalog_append(target TEXT,ver TEXT,kind TEXT,data JSONB) RETURNS void LANGUAGE plpgsql AS $$
DECLARE stamp TIMESTAMPTZ;
BEGIN
    -- Row lock lasts until the publishing transaction commits. A later catalog
    -- timestamp can never become visible before an earlier transaction commits.
    SELECT greatest(clock_timestamp(),committed_at+interval '1 microsecond') INTO stamp
      FROM catalog_head WHERE singleton FOR UPDATE;
    UPDATE catalog_head SET committed_at=stamp WHERE singleton;
    INSERT INTO catalog_events(event_id,committed_at,package_id,version,event_type,payload)
      VALUES(gen_random_uuid(),stamp,target,ver,kind,data);
END;
$$;
CREATE FUNCTION catalog_snapshot(v package_versions,display TEXT) RETURNS JSONB LANGUAGE SQL AS $$
    SELECT jsonb_strip_nulls(jsonb_build_object(
        'id',display,'originalId',display,
        'version',v.version || CASE WHEN strpos(v.display_version,'+')>0 THEN '+'||split_part(v.display_version,'+',2) ELSE '' END,
        'verbatimVersion',v.display_version,'title',v.title,'description',v.description,
        'authors',v.authors,'tags',regexp_split_to_array(trim(v.tags),'\s+'),
        'projectUrl',v.project_url,'licenseExpression',v.license_expression,
        'dependencyGroups',v.dependencies,'deprecation',v.deprecation,
        'listed',v.listed,'isPrerelease',v.prerelease,'created',v.published_at,
        'published',CASE WHEN v.listed THEN v.listed_at ELSE '1900-01-01T00:00:00Z'::timestamptz END,
        'packageHash',v.sha512,'packageHashAlgorithm','SHA512','packageSize',v.size));
$$;
CREATE FUNCTION catalog_listing_time() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.listed AND NOT OLD.listed THEN NEW.listed_at=clock_timestamp(); END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER catalog_listing_time BEFORE UPDATE OF listed ON package_versions
    FOR EACH ROW EXECUTE FUNCTION catalog_listing_time();
CREATE FUNCTION catalog_record_package() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE display TEXT;
BEGIN
    IF TG_OP='DELETE' THEN
        SELECT display_id INTO display FROM packages WHERE id=OLD.package_id;
        PERFORM catalog_append(OLD.package_id,OLD.version,'PackageDelete',jsonb_build_object(
            'id',COALESCE(display,OLD.package_id),'version',OLD.display_version,'published',clock_timestamp()));
        RETURN OLD;
    END IF;
    IF TG_OP='UPDATE' AND NEW.listed IS NOT DISTINCT FROM OLD.listed
        AND NEW.deprecation IS NOT DISTINCT FROM OLD.deprecation THEN RETURN NEW; END IF;
    SELECT display_id INTO display FROM packages WHERE id=NEW.package_id;
    PERFORM catalog_append(NEW.package_id,NEW.version,'PackageDetails',catalog_snapshot(NEW,display));
    RETURN NEW;
END;
$$;
CREATE TRIGGER catalog_package_change AFTER INSERT OR UPDATE OR DELETE ON package_versions
    FOR EACH ROW EXECUTE FUNCTION catalog_record_package();
-- Existing installations receive one snapshot per current local version.
DO $$
DECLARE v package_versions; display TEXT;
BEGIN
    FOR v IN SELECT * FROM package_versions ORDER BY package_id,version_sort LOOP
        SELECT display_id INTO display FROM packages WHERE id=v.package_id;
        PERFORM catalog_append(v.package_id,v.version,'PackageDetails',catalog_snapshot(v,display));
    END LOOP;
END;
$$;
CREATE FUNCTION catalog_immutable() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'Catalog events are immutable' USING ERRCODE='55000'; END;
$$;
CREATE TRIGGER catalog_events_immutable BEFORE UPDATE OR DELETE ON catalog_events
    FOR EACH ROW EXECUTE FUNCTION catalog_immutable();
