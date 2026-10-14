#!/usr/bin/env bash
# Manual live test. Run with the NuShelf app container's network namespace.
set -euo pipefail
cd /tmp
dotnet new console --name LiveConsumer --output /tmp/LiveConsumer --no-restore
cat > /tmp/LiveConsumer/NuGet.Config <<'CONFIG'
<configuration><packageSources><clear/><add key="NuShelf" value="http://localhost:5000/v3/index.json" allowInsecureConnections="true"/></packageSources></configuration>
CONFIG
dotnet add /tmp/LiveConsumer/LiveConsumer.csproj package Newtonsoft.Json --version 13.0.3 --source http://localhost:5000/v3/index.json
cat > /tmp/LiveConsumer/Program.cs <<'CS'
using Newtonsoft.Json;
Console.WriteLine(JsonConvert.SerializeObject(new { Ready = true }));
CS
dotnet restore /tmp/LiveConsumer/LiveConsumer.csproj --no-cache --force -p:NuGetAudit=false
dotnet build /tmp/LiveConsumer/LiveConsumer.csproj --no-restore
dotnet run --project /tmp/LiveConsumer/LiveConsumer.csproj --no-build
echo 'PASS: live Newtonsoft.Json from nuget.org through NuShelf cache, restored, compiled and executed.'
