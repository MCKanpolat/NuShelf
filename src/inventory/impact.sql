SELECT jsonb_build_object(
  'applicationId',a.id,'application',a.name,'releaseId',r.id,'release',r.version,
  'commitSha',r.commit_sha,'ref',c.bom_ref,'packageId',c.package_id,
  'version',c.version,'advisories',advisory.value
)
FROM public.audit_snapshots s
CROSS JOIN LATERAL jsonb_each(CASE WHEN jsonb_typeof(s.data)='object' THEN s.data ELSE '{}'::jsonb END) advisory
JOIN inventory_components c ON c.package_id=advisory.key
JOIN inventory_releases r ON r.id=c.release_id
JOIN inventory_applications a ON a.id=r.application_id
WHERE s.current AND ($1 OR a.owner_id=$2)
AND (NOT $3 OR r.id=(SELECT latest.id FROM inventory_releases latest WHERE latest.application_id=a.id ORDER BY latest.created_at DESC,latest.id DESC LIMIT 1))
AND ($4::uuid IS NULL OR (r.id,c.bom_ref)>($4,$5::text))
ORDER BY r.id,c.bom_ref
LIMIT $6
