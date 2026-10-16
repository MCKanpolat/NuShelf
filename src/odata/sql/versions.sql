WITH versions AS (SELECT p.id AS id,v.version_sort,v.prerelease,v.listed,v.semver2,jsonb_build_object(
        'Id',p.display_id,
        'Version',v.display_version,
        'NormalizedVersion',v.version,
        'Title',v.title,
        'Description',v.description,
        'Authors',v.authors,
        'Tags',v.tags,
        'ProjectUrl',v.project_url,
        'DownloadCount',p.downloads,
        'VersionDownloadCount',v.downloads,
        'PackageSize',v.size,
        'PackageHash',v.sha512,
        'Published',v.published_at,
        'Created',v.published_at,
        'LastUpdated',v.published_at,
        'IsPrerelease',v.prerelease,
        'Listed',v.listed,
        '_frameworks',v.assets->'frameworks',
        '_dependencies',v.dependencies) AS data
    FROM package_versions v JOIN packages p ON p.id=v.package_id
    UNION ALL
    SELECT c.package_id,c.version_sort,c.prerelease,COALESCE((c.catalog->>'listed')::boolean,true),c.semver2,jsonb_build_object(
        'Id',c.catalog->>'id',
        'Version',c.catalog->>'version',
        'NormalizedVersion',c.version,
        'Title',c.title,
        'Description',c.description,
        'Authors',c.authors,
        'Tags',c.tags,
        'ProjectUrl',c.catalog->>'projectUrl',
        'DownloadCount',(SELECT sum(downloads)
    FROM upstream_cache
    WHERE package_id=c.package_id),
        'VersionDownloadCount',c.downloads,
        'PackageSize',c.size,
        'PackageHash',c.sha512,
        'Published',c.catalog->>'published',
        '_frameworks',c.assets->'frameworks',
        'Created',c.cached_at,
        'LastUpdated',c.cached_at,
        'IsPrerelease',c.prerelease,
        'Listed',COALESCE((c.catalog->>'listed')::boolean,true),
        '_dependencies',c.catalog->'dependencyGroups')
    FROM upstream_cache c JOIN upstream_metadata m USING(package_id) JOIN upstream_sources s ON s.id=m.source_id
    WHERE s.enabled
      AND upstream_route_allowed(c.package_id,s.id)
      AND NOT EXISTS(SELECT 1
    FROM packages p
    WHERE p.id=c.package_id)
      AND (NOT s.private OR $1 OR ($2::uuid IS NOT NULL
      AND starts_with(c.package_id,$3)
      AND EXISTS(SELECT 1
    FROM upstream_grants g
    WHERE g.source_id=s.id
      AND g.profile_id=$4)))
    UNION ALL
    SELECT lower(x->>'Id'),x->>'_sort',(x->>'IsPrerelease')::boolean,(x->>'Listed')::boolean,(x->>'_semver2')::boolean,x
    FROM jsonb_array_elements($5::jsonb) x
    WHERE NOT EXISTS(SELECT 1
    FROM upstream_cache c
    WHERE c.package_id=lower(x->>'Id')
      AND c.version=x->>'NormalizedVersion')
      AND NOT EXISTS(SELECT 1
    FROM packages p
    WHERE p.id=lower(x->>'Id'))) ,
visible AS (SELECT *
    FROM versions
    WHERE starts_with(id,$6)
      AND ($7 OR NOT semver2)),
packages AS (SELECT data || jsonb_build_object(
        'IsLatestVersion',listed
      AND NOT prerelease
      AND version_sort=max(version_sort) FILTER(WHERE listed
      AND NOT prerelease) OVER(PARTITION BY id),
        'IsAbsoluteLatestVersion',listed
      AND version_sort=max(version_sort) FILTER(WHERE listed) OVER(PARTITION BY id)) AS data, id,version_sort
    FROM visible) SELECT data,id,version_sort
    FROM packages
    WHERE 
