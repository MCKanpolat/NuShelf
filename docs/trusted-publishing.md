# Trusted Publishing with GitHub Actions

**Manage → Trusted Publishing** binds a publisher profile and package prefix to a GitHub repository's immutable numeric repository/owner IDs, an exact workflow file and an exact branch/tag reference. An optional environment must match the job's environment. An optional feed ID selects a named feed; otherwise the policy targets the root registry. The profile needs publisher membership in a named feed.

NuShelf verifies GitHub's RS256 signature using its official JWKS, issuer, audience, token lifetime, repository/owner IDs, workflow, ref, environment and default subject. Both legacy and immutable GitHub default subject formats are supported. Pull-request and `pull_request_target` events are rejected. Only `push`, `release` and `workflow_dispatch` events are accepted. Custom subject formats and other CI providers are not yet supported.

Exchange tokens at the **root** `/api/trusted-publishing/exchange` endpoint. Set the GitHub OIDC audience to the root `PUBLIC_URL` exactly. Each OIDC token ID can be exchanged only once, across replicas. Signing keys are cached for five minutes with refresh throttling. A token must be issued within ten minutes and have an expiry no more than fifteen minutes after issue.

Successful exchange returns `key`, `expiresAt` and `source`. The temporary key lives for ten minutes, has only `package:push` and `package:read`, and is restricted to the policy's package prefix and feed. It cannot create credentials, unlist packages or publish to another feed. Disabling a policy revokes its temporary keys. Profile membership and package ownership continue to apply.

## Workflow example

A token must match exactly one enabled policy. Use distinct workflow/ref/environment combinations for separate publishing targets; overlapping policies are rejected rather than selecting a target implicitly.

Replace the registry URL below and create a policy for this repository's IDs, `.github/workflows/publish.yml`, `refs/heads/main` and the desired package prefix. The runner needs HTTPS access to your registry; use a self-hosted runner for an internal deployment. Retrieve numeric IDs from GitHub's repository/owner APIs; names alone are not sufficient.

```yaml
name: Publish packages
on:
  workflow_dispatch:
permissions:
  contents: read
  id-token: write
jobs:
  publish:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-dotnet@v4
        with:
          dotnet-version: '10.0.x'
      - run: dotnet pack --configuration Release --output ./artifacts
      - name: Exchange OIDC token and publish
        env:
          REGISTRY: https://packages.example.com
        shell: bash
        run: |
          set -euo pipefail
          audience=$(jq -rn --arg value "$REGISTRY" '$value|@uri')
          oidc=$(curl --fail --silent --show-error \
            -H "Authorization: Bearer $ACTIONS_ID_TOKEN_REQUEST_TOKEN" \
            "${ACTIONS_ID_TOKEN_REQUEST_URL}&audience=${audience}" | jq -r '.value')
          echo "::add-mask::$oidc"
          result=$(jq -n --arg token "$oidc" '{token:$token}' | \
            curl --fail --silent --show-error -H 'Content-Type: application/json' \
              --data-binary @- "$REGISTRY/api/trusted-publishing/exchange")
          key=$(jq -r '.key' <<< "$result")
          source=$(jq -r '.source' <<< "$result")
          echo "::add-mask::$key"
          config=$(mktemp)
          trap 'rm -f "$config"' EXIT
          printf '<configuration><packageSources><clear/></packageSources></configuration>' > "$config"
          dotnet nuget add source "$source" --name NuShelf --username profile \
            --password "$key" --store-password-in-clear-text --configfile "$config"
          dotnet nuget push './artifacts/*.nupkg' --source "$source" \
            --api-key "$key" --configfile "$config"
```

The temporary config is only for authenticating private service-index discovery and is deleted when the step exits. Acquire the key immediately before publishing; long builds should finish before exchange.

Administrator APIs: `GET/POST /api/trusted-publishing`, `DELETE /api/trusted-publishing/<id>` (disable/revoke). Policy fields: `profileId`, `name`, `repository`, `repositoryId`, `ownerId`, `workflow`, `gitRef`, optional `environment`, `packagePrefix`, optional `feedId`. Exchange accepts `{ "token": "<GitHub OIDC JWT>" }` without a persistent API key.

Tests use an explicitly disposable RSA fixture only in isolated Docker databases. A real GitHub Actions publishing run requires configuring a policy and a runner that can reach your deployment.

Reference: [GitHub OIDC claims](https://docs.github.com/en/actions/reference/security/oidc).
