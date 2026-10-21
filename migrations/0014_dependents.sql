CREATE TABLE package_dependencies (
    package_id TEXT NOT NULL,
    version TEXT NOT NULL,
    framework TEXT NOT NULL,
    dependency_id TEXT NOT NULL,
    version_range TEXT NOT NULL,
    PRIMARY KEY (package_id, version, framework, dependency_id, version_range),
    FOREIGN KEY (package_id, version) REFERENCES package_versions(package_id, version) ON DELETE CASCADE
);
CREATE INDEX package_dependents_idx ON package_dependencies(dependency_id, package_id, version);

CREATE FUNCTION refresh_package_dependencies() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM package_dependencies WHERE package_id=NEW.package_id AND version=NEW.version;
    INSERT INTO package_dependencies
    SELECT DISTINCT NEW.package_id, NEW.version, COALESCE(g->>'targetFramework',''),
           lower(d->>'id'), COALESCE(d->>'range','*')
    FROM jsonb_array_elements(NEW.dependencies) g
    CROSS JOIN LATERAL jsonb_array_elements(g->'dependencies') d;
    RETURN NEW;
END;
$$;
CREATE TRIGGER package_dependencies_changed AFTER INSERT OR UPDATE OF dependencies ON package_versions
FOR EACH ROW EXECUTE FUNCTION refresh_package_dependencies();
INSERT INTO package_dependencies
SELECT DISTINCT v.package_id,v.version,COALESCE(g->>'targetFramework',''),lower(d->>'id'),COALESCE(d->>'range','*')
FROM package_versions v CROSS JOIN LATERAL jsonb_array_elements(v.dependencies) g
CROSS JOIN LATERAL jsonb_array_elements(g->'dependencies') d;
