#!/usr/bin/env bash
set -euo pipefail
task_root=$(mktemp -d /tmp/nushelf-client.XXXXXX)
cd "$task_root"
admin=test-only-admin-key-at-least-32-characters
base=http://registry:5000
name="ClientTest-$(date +%s)"
profile=$(curl --fail-with-body -s "$base/api/profiles" -H "Authorization: Bearer $admin" -H 'Content-Type: application/json' -d "{\"name\":\"$name\"}")
profile_id=$(echo "$profile" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
credential=$(curl --fail-with-body -s "$base/api/profiles/$profile_id/keys" -H "Authorization: Bearer $admin" -H 'Content-Type: application/json' -d '{"label":"dotnet test","scopes":["package:push","package:unlist","mcp:read"]}')
key=$(echo "$credential" | sed -n 's/.*"key":"\([^"]*\)".*/\1/p')
test -n "$key"
package="NuShelf.ClientTest.$(date +%s)"
dotnet new classlib -n TestPackage -o "$task_root/TestPackage" --no-restore
dotnet pack "$task_root/TestPackage/TestPackage.csproj" -p:PackageId="$package" -p:Version=1.0.0 -o "$task_root/packages"
dotnet nuget push "$task_root/packages/$package.1.0.0.nupkg" --source "$base/v3/index.json" --api-key "$key"
dotnet new console -n Consumer -o "$task_root/Consumer" --no-restore
cat > "$task_root/Consumer/NuGet.Config" <<CONFIG
<configuration><packageSources><clear/><add key="NuShelf" value="$base/v3/index.json" allowInsecureConnections="true"/></packageSources></configuration>
CONFIG
dotnet add "$task_root/Consumer/Consumer.csproj" package "$package" --version 1.0.0 --source "$base/v3/index.json"
dotnet restore "$task_root/Consumer/Consumer.csproj" --no-cache --force
dotnet build "$task_root/Consumer/Consumer.csproj" --no-restore
dotnet nuget delete "$package" 1.0.0 --source "$base/v3/index.json" --api-key "$key" --non-interactive
dotnet nuget locals all --clear
dotnet restore "$task_root/Consumer/Consumer.csproj" --no-cache --force
if [ -d /artifacts ]; then
  dotnet pack "$task_root/TestPackage/TestPackage.csproj" -p:PackageId="NuShelf.UiSample.$(date +%s)" -p:Version=1.0.0 -o "$task_root/ui-package"
  cp "$task_root"/ui-package/*.nupkg /artifacts/ui-sample.nupkg
fi
echo 'PASS: real dotnet push, package install, restore, build, unlist, and restore after unlisting via S3 storage.'

# Symbols: one `dotnet nuget push` of the .nupkg also sends its sibling .snupkg to
# the SymbolPackagePublish endpoint, reusing the same API key.
symbol_package="NuShelf.SymbolTest.$(date +%s)"
dotnet pack "$task_root/TestPackage/TestPackage.csproj" -p:PackageId="$symbol_package" -p:Version=1.0.0 -p:IncludeSymbols=true -p:SymbolPackageFormat=snupkg -o "$task_root/symbol-packages"
test -f "$task_root/symbol-packages/$symbol_package.1.0.0.snupkg"
dotnet nuget push "$task_root/symbol-packages/$symbol_package.1.0.0.nupkg" --source "$base/v3/index.json" --api-key "$key" | tee "$task_root/symbol-push.log"
grep -q 'snupkg' "$task_root/symbol-push.log"
# The symbol key is computed independently of the server: from the CodeView record
# in the built DLL (GUID stored little-endian) plus the portable-PDB age.
dll="$task_root/TestPackage/bin/Release/net8.0/TestPackage.dll"
offset=$(grep -obUaP 'RSDS' "$dll" | head -1 | cut -d: -f1)
bytes=($(od -An -tx1 -j $((offset + 4)) -N16 -v "$dll"))
symbol_key="${bytes[3]}${bytes[2]}${bytes[1]}${bytes[0]}${bytes[5]}${bytes[4]}${bytes[7]}${bytes[6]}${bytes[8]}${bytes[9]}${bytes[10]}${bytes[11]}${bytes[12]}${bytes[13]}${bytes[14]}${bytes[15]}ffffffff"
curl --fail-with-body -s "$base/symbols/testpackage.pdb/$symbol_key/testpackage.pdb" -o "$task_root/served.pdb"
cmp "$task_root/served.pdb" "$task_root/TestPackage/bin/Release/net8.0/TestPackage.pdb"
# A repeated push is skipped as a duplicate by the real client (409 for both files).
dotnet nuget push "$task_root/symbol-packages/$symbol_package.1.0.0.nupkg" --source "$base/v3/index.json" --api-key "$key" --skip-duplicate
echo 'PASS: real dotnet pushed a .snupkg through SymbolPackagePublish and the symbol server returned the exact PDB for the key derived from the DLL.'

# A second origin serves a real generated package. Only NuShelf is configured on the consumer.
mkdir -p /artifacts/proxy-fixtures
rm -f /artifacts/proxy-fixtures/.offline
proxy_package="NuShelf.ProxyTest.$(date +%s)"
dotnet pack "$task_root/TestPackage/TestPackage.csproj" -p:PackageId="$proxy_package" -p:Version=1.0.0 -o /artifacts/proxy-fixtures
source=$(curl --fail-with-body -s "$base/api/upstreams" -H "Authorization: Bearer $admin" -H 'Content-Type: application/json' -d "{\"name\":\"Docker upstream $proxy_package\",\"url\":\"http://proxy-origin:8080/v3/index.json\",\"patterns\":[\"$proxy_package\"],\"allowHttp\":true,\"allowPrivateNetwork\":true,\"metadataTtlSeconds\":1}")
dotnet add "$task_root/Consumer/Consumer.csproj" package "$proxy_package" --version 1.0.0 --source "$base/v3/index.json"
proxy_lower=$(echo "$proxy_package" | tr '[:upper:]' '[:lower:]')
curl --fail-with-body -s -D "$task_root/cache-headers" "$base/v3/flatcontainer/$proxy_lower/1.0.0/$proxy_lower.1.0.0.nupkg" -o "$task_root/cached.nupkg"
grep -qi 'x-nushelf-cache: HIT' "$task_root/cache-headers"
origin_before=$(curl --fail-with-body -s http://proxy-origin:8080/stats)
echo "$origin_before" | grep -q '"downloads": 1'
touch /artifacts/proxy-fixtures/.offline
sleep 2
dotnet nuget locals all --clear
dotnet restore "$task_root/Consumer/Consumer.csproj" --no-cache --force
dotnet build "$task_root/Consumer/Consumer.csproj" --no-restore
origin_after=$(curl --fail-with-body -s http://proxy-origin:8080/stats)
test "$origin_before" = "$origin_after"
rm -f /artifacts/proxy-fixtures/.offline
echo 'PASS: real dotnet restore through upstream proxy, cache HIT, one origin download, and restore/build during an upstream outage.'

# Force a cold restore through the legacy feed, including an unlisted exact version.
sed -i 's|/v3/index.json|/api/v2|g' "$task_root/Consumer/NuGet.Config"
dotnet restore "$task_root/Consumer/Consumer.csproj" --no-cache --force --packages "$task_root/v2-packages"
dotnet build "$task_root/Consumer/Consumer.csproj" --no-restore
echo 'PASS: real dotnet cold restore and build through OData V2.'
