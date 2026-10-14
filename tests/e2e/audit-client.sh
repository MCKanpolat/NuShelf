#!/usr/bin/env bash
set -euo pipefail
task_root=$(mktemp -d /tmp/nushelf-audit.XXXXXX)
cd "$task_root"
admin=test-only-admin-key-at-least-32-characters
base=http://registry:5000
name="audit-$(date +%s)"
profile=$(curl --fail-with-body -s "$base/api/profiles" -H "Authorization: Bearer $admin" -H 'Content-Type: application/json' -d "{\"name\":\"$name\"}")
profile_id=$(echo "$profile" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
credential=$(curl --fail-with-body -s "$base/api/profiles/$profile_id/keys" -H "Authorization: Bearer $admin" -H 'Content-Type: application/json' -d '{"label":"audit client","scopes":["package:push","package:read","package:deprecate"]}')
key=$(echo "$credential" | sed -n 's/.*"key":"\([^"]*\)".*/\1/p')
feed=$(curl --fail-with-body -s "$base/api/feeds" -H "Authorization: Bearer $admin" -H 'Content-Type: application/json' -d "{\"name\":\"$name\"}")
feed_id=$(echo "$feed" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
curl --fail-with-body -s -X PUT "$base/api/feeds/$feed_id/members/$profile_id" -H "Authorization: Bearer $admin" -H 'Content-Type: application/json' -d '{"role":"publisher"}'
package="NuShelf.AuditTest.$(date +%s)"
source="$base/feeds/$name/v3/index.json"
dotnet new classlib -n AuditPackage --no-restore
dotnet pack AuditPackage -p:PackageId="$package" -p:Version=1.0.0 -o packages
printf '<configuration><packageSources><clear/></packageSources></configuration>' > NuGet.Config
dotnet nuget add source "$source" --name NuShelf --username profile --password "$key" --store-password-in-clear-text --configfile NuGet.Config
dotnet nuget push "packages/$package.1.0.0.nupkg" --source "$source" --api-key "$key"
package_lower=$(echo "$package" | tr '[:upper:]' '[:lower:]')
curl --fail-with-body -s "$base/api/audit/import" -H "Authorization: Bearer $admin" -H 'Content-Type: application/json' -d "{\"$package_lower\":[{\"severity\":2,\"versions\":\"[1.0.0,2.0.0)\",\"url\":\"https://github.com/advisories/GHSA-fixture\"}]}" >/dev/null
dotnet new console -n AuditConsumer --no-restore
cat > AuditConsumer/NuGet.Config <<CONFIG
<configuration><packageSources><clear/></packageSources></configuration>
CONFIG
dotnet nuget add source "$source" --name NuShelf --username profile --password "$key" --store-password-in-clear-text --configfile AuditConsumer/NuGet.Config
dotnet add AuditConsumer package "$package" --version 1.0.0
dotnet nuget locals all --clear
dotnet restore AuditConsumer --no-cache --force -p:NuGetAudit=true -p:NuGetAuditLevel=low 2>&1 | tee audit-output.txt
grep -q 'NU1903' audit-output.txt
curl --fail-with-body -s -X PUT "$base/feeds/$name/api/packages/$package_lower/1.0.0/deprecation" -H "Authorization: Bearer $key" -H 'Content-Type: application/json' -d '{"reasons":["Legacy"],"message":"Test deprecation","alternatePackage":{"id":"NuShelf.Replacement","range":"*"}}'
dotnet list AuditConsumer package --deprecated 2>&1 | tee deprecation-output.txt
grep -q 'NuShelf.Replacement' deprecation-output.txt
if [ -d /artifacts ]; then cp audit-output.txt /artifacts/audit-client-output.txt; fi
echo 'PASS: authenticated private-feed restore reports NU1903 and the client detects deprecation metadata.'
