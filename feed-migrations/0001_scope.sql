-- Future package/cache schema changes are migrated here for every named feed.
CREATE TABLE feed_schema_version (version INTEGER PRIMARY KEY);
INSERT INTO feed_schema_version VALUES (1);
