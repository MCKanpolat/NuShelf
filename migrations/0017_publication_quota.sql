CREATE TABLE registry_usage (
    singleton BOOLEAN PRIMARY KEY DEFAULT true CHECK(singleton),
    package_count BIGINT NOT NULL DEFAULT 0 CHECK(package_count>=0),
    version_count BIGINT NOT NULL DEFAULT 0 CHECK(version_count>=0),
    package_bytes BIGINT NOT NULL DEFAULT 0 CHECK(package_bytes>=0),
    symbol_bytes BIGINT NOT NULL DEFAULT 0 CHECK(symbol_bytes>=0),
    max_bytes BIGINT NOT NULL DEFAULT 0 CHECK(max_bytes>=0),
    max_packages BIGINT NOT NULL DEFAULT 0 CHECK(max_packages>=0),
    max_versions BIGINT NOT NULL DEFAULT 0 CHECK(max_versions>=0),
    revision BIGINT NOT NULL DEFAULT 1
);
INSERT INTO registry_usage(singleton,package_count,version_count,package_bytes,symbol_bytes)
SELECT true,(SELECT count(*) FROM packages),(SELECT count(*) FROM package_versions),
    (SELECT COALESCE(sum(size),0) FROM package_versions),(SELECT COALESCE(sum(size),0) FROM symbol_files);

CREATE FUNCTION check_publication_quota(packages BIGINT,versions BIGINT,archives BIGINT,symbols BIGINT) RETURNS void LANGUAGE plpgsql AS $$
DECLARE u registry_usage%ROWTYPE;
BEGIN
    SELECT * INTO STRICT u FROM registry_usage WHERE singleton FOR UPDATE;
    IF (packages>0 AND u.max_packages>0 AND u.package_count+packages>u.max_packages)
       OR (versions>0 AND u.max_versions>0 AND u.version_count+versions>u.max_versions)
       OR (archives+symbols>0 AND u.max_bytes>0 AND u.package_bytes+u.symbol_bytes+archives+symbols>u.max_bytes)
    THEN RAISE EXCEPTION 'Publication quota exceeded' USING ERRCODE='P0001'; END IF;
END;
$$;
CREATE FUNCTION account_publication_usage() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE p BIGINT:=0; v BIGINT:=0; a BIGINT:=0; s BIGINT:=0;
BEGIN
    IF TG_TABLE_NAME='packages' THEN
        p:=CASE WHEN TG_OP='INSERT' THEN 1 ELSE -1 END;
    ELSIF TG_TABLE_NAME='package_versions' THEN
        v:=CASE WHEN TG_OP='INSERT' THEN 1 WHEN TG_OP='DELETE' THEN -1 ELSE 0 END;
        a:=CASE WHEN TG_OP='DELETE' THEN -OLD.size WHEN TG_OP='INSERT' THEN NEW.size ELSE NEW.size-OLD.size END;
    ELSE
        s:=CASE WHEN TG_OP='DELETE' THEN -OLD.size WHEN TG_OP='INSERT' THEN NEW.size ELSE NEW.size-OLD.size END;
    END IF;
    PERFORM check_publication_quota(p,v,a,s);
    UPDATE registry_usage SET package_count=package_count+p,version_count=version_count+v,
        package_bytes=package_bytes+a,symbol_bytes=symbol_bytes+s WHERE singleton;
    RETURN NULL;
END;
$$;
CREATE TRIGGER usage_packages AFTER INSERT OR DELETE ON packages FOR EACH ROW EXECUTE FUNCTION account_publication_usage();
CREATE TRIGGER usage_versions AFTER INSERT OR DELETE OR UPDATE OF size ON package_versions FOR EACH ROW EXECUTE FUNCTION account_publication_usage();
CREATE TRIGGER usage_symbols AFTER INSERT OR DELETE OR UPDATE OF size ON symbol_files FOR EACH ROW EXECUTE FUNCTION account_publication_usage();
CREATE TABLE registry_usage_history (
    day DATE PRIMARY KEY,
    package_count BIGINT NOT NULL,
    version_count BIGINT NOT NULL,
    package_bytes BIGINT NOT NULL,
    symbol_bytes BIGINT NOT NULL,
    sampled_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
INSERT INTO registry_usage_history SELECT ((now() AT TIME ZONE 'UTC')::date),package_count,version_count,package_bytes,symbol_bytes,now() FROM registry_usage;

-- TRUNCATE bypasses row triggers; keep administrative bulk operations accurate too.
CREATE FUNCTION reset_publication_usage() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_TABLE_NAME='packages' THEN
        UPDATE registry_usage SET package_count=0 WHERE singleton;
    ELSIF TG_TABLE_NAME='package_versions' THEN
        UPDATE registry_usage SET version_count=0,package_bytes=0 WHERE singleton;
    ELSE
        UPDATE registry_usage SET symbol_bytes=0 WHERE singleton;
    END IF;
    RETURN NULL;
END;
$$;
CREATE TRIGGER usage_packages_truncated AFTER TRUNCATE ON packages EXECUTE FUNCTION reset_publication_usage();
CREATE TRIGGER usage_versions_truncated AFTER TRUNCATE ON package_versions EXECUTE FUNCTION reset_publication_usage();
CREATE TRIGGER usage_symbols_truncated AFTER TRUNCATE ON symbol_files EXECUTE FUNCTION reset_publication_usage();
