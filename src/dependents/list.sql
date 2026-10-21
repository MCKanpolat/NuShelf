SELECT jsonb_build_object('id',p.display_id,'version',v.display_version,
    'normalizedVersion',v.version,'framework',d.framework,'range',d.version_range,'listed',v.listed)
FROM package_dependencies d JOIN package_versions v USING(package_id,version)
JOIN packages p ON p.id=d.package_id
WHERE dependency_id=$1 AND (v.listed OR $2)
    AND starts_with(d.package_id,$3) AND starts_with(d.package_id,$4)
ORDER BY d.package_id,v.version_sort DESC,d.framework,d.version_range
LIMIT $5 OFFSET $6
