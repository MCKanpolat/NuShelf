WITH visible AS (
    SELECT sequence,event_id,committed_at FROM catalog_events
    WHERE starts_with(package_id,$1)
), pages AS (
    SELECT (sequence-1)/$2 AS page,count(*) AS count,max(sequence) AS latest
    FROM visible GROUP BY (sequence-1)/$2
)
SELECT p.page,p.count,e.event_id,e.committed_at
FROM pages p JOIN visible e ON e.sequence=p.latest
ORDER BY p.page
