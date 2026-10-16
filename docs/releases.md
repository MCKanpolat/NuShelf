# Releasing a selected revision

Releases are explicit requests. A normal push or merge to `main` runs CI and does not publish an image, chart, tag or GitHub Release. Pushing a tag alone does not publish artifacts either.

## Request a release

In **Actions → Release → Run workflow**, keep the workflow branch as `main`, enter a commit SHA (recommended) or a ref in `revision`, and select `auto`, `patch`, `minor` or `major`.

Equivalent CLI:

```bash
gh workflow run release.yml --repo MCKanpolat/NuShelf --ref main \
  -f revision=<commit-sha> -f bump=auto
```

The revision must already belong to the history of remote `main` and have a successful **Docker checks** push run for that exact SHA on `main`. Wait for CI to finish before requesting the release. A branch or tag input is resolved once to its full SHA; all builds use that SHA. The workflow implementation itself comes from the `main` revision at dispatch time, allowing an older application revision to be released using the current release tooling.

## Version calculation

Stable releases use `vMAJOR.MINOR.PATCH` Git tags. The highest stable tag is the baseline; the selected revision must descend from it. The initial release uses the selected revision's stable `Cargo.toml` package version, currently `0.1.0`.

For subsequent releases, `auto` examines **every commit since the previous release up to the selected revision**, including commit bodies:

| Commit message | Increment |
| --- | --- |
| `#major`, `feat!:`, another conventional `type!:`, or `BREAKING CHANGE:` / `BREAKING-CHANGE:` | Major |
| `#minor` or `feat:` / `feat(scope):` | Minor |
| `#patch` or no matching marker | Patch |

Hashtags are case-insensitive and must be complete tokens. The highest applicable increment wins, and the version increments once per release. An explicit `patch`, `minor` or `major` input overrides detection. The first release always uses the declared baseline.

`MCKanpolat/auto-semver-action` currently reads commit messages from a push event payload and does not accept a target revision / comparison range. Manual releases therefore use the repository's version resolver rather than relying on an empty dispatch payload. Its integration can be revisited when the action supports explicit revision ranges.

## Publication

1. Resolve the revision, calculate the version and confirm successful CI.
2. Reserve an annotated Git tag on the exact source SHA and create a **draft** GitHub Release.
3. Stamp the release version into `Cargo.toml` and `Cargo.lock` in the build checkout. These build-only changes are not committed and do not move the source tag.
4. Build native `linux/amd64` and `linux/arm64` images and publish a multi-architecture image as `ghcr.io/mckanpolat/nushelf:<version>`.
5. Package and publish `oci://ghcr.io/mckanpolat/charts/nushelf` with matching chart `version` and `appVersion`. Its default image tag is that same version.
6. Attach the chart archive, its `SHA256SUMS`, and `provenance.json` containing the source SHA, version and image manifest digest.
7. Publish the GitHub Release with generated notes listing every commit in the exact release range, the comparison link and container digest.

The workflow uses `GITHUB_TOKEN` with write permissions only on jobs that require them. The repository must permit these workflow token permissions. No personal access token or chained tag-push trigger is required. Images and charts are built within the same workflow, since tags created with `GITHUB_TOKEN` do not start another push workflow.

Configure the repository Actions secrets `DOCKERHUB_USERNAME` and `DOCKERHUB_TOKEN` (a Docker Hub read-only PAT) for base-image pulls in CI and releases. Docker Hub login is skipped for fork and Dependabot pull requests, which do not receive these secrets; those checks use public pulls. GHCR publication continues to use `GITHUB_TOKEN`.

New GHCR packages can initially be private. Set the `nushelf` and `charts/nushelf` package visibility to public if unauthenticated installation is intended; otherwise configure registry pull credentials.

## Failure and retry

A single release concurrency group serializes version allocation and publication. Existing tags are never moved. If a build or publication fails, the tag and draft remain available for diagnosis.

Use **Re-run failed jobs** to resume the same run, or dispatch the **same revision** again. When that revision already has the latest stable tag, its version is reused, even if a different bump was supplied. An already published release is left unchanged and artifact publication is skipped. Partial artifacts can be rebuilt while the release is still a draft.

Finish an existing stable draft before selecting a different revision. Historical revisions older than the latest stable tag are rejected. This workflow publishes stable releases; prerelease channels and development `edge` images are not created.
