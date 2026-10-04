# Release Process

## Fork distribution

This checkout is maintained as `iugamlabs/agent-wechat`. The inherited release setup below describes upstream distribution; it is not evidence that the fork has published packages, images, or a documentation site. Source development and upstream synchronization follow [CONTRIBUTING.md](../CONTRIBUTING.md).

Current distribution settings in the checked-in implementation:

| Setting | Current behavior | Fork release preparation |
| --- | --- | --- |
| npm package identity | Public packages retain the `@agent-wechat` names. | Decide fork-owned package names and versions, update dependencies and installation instructions, and configure publishing access. Review OpenClaw package basename/plugin ID compatibility before renaming the extension. |
| CLI container image and update downloads | `packages/cli/src/cli.ts` uses `ghcr.io/thisnick/agent-wechat` and binary assets from `thisnick/agent-wechat` releases. | Update and verify both destinations before using the CLI to distribute fork builds. |
| Compose image | `docker-compose.yml` uses the upstream GHCR image. | Select a verified fork image for fork deployments. |
| Release and rebuild workflows | GHCR names derive from `github.repository`, while npm names come from package manifests. | Review these destinations separately; changing the GitHub owner does not change npm package ownership. |
| Changesets and package metadata | `.changeset/config.json` and public package repository fields refer to `thisnick/agent-wechat`. | Update repository attribution for fork release notes and package source links while retaining upstream credit. |
| Documentation hosting | `docs/astro.config.mjs` points to `thisnick.github.io` and the upstream repository. | Configure a fork site and verify its deployment before replacing the upstream documentation link. |

Before enabling fork publishing, review `.github/workflows/release.yml`, `docker-rebuild.yml`, and `deploy-docs.yml`, along with the fork's Actions settings, publishing access, and Pages configuration. The release workflow runs on pushes to `main`; the rebuild workflow is manually triggered. Keep publishing workflows disabled until their destinations and identities are configured. Actual remote Actions settings and artifact availability must be checked separately from the workflow files.

Continue adding changesets for user-facing changes and follow the fork version policy below. Do not reuse an upstream artifact's identity for different fork content.

Until fork distribution is configured, build from the checkout with `pnpm build` and the appropriate `pnpm build:image:amd64` or `pnpm build:image:arm64` command. The image script creates local tags `agent-wechat:amd64` and `agent-wechat:arm64`. The CLI currently has no `wx up --image` override; use an isolated container configuration with an explicit local image and separate volumes, ports, and credentials, then point the local CLI at it with `AGENT_WECHAT_URL` and `AGENT_WECHAT_TOKEN`. Build output alone does not update a running container.

## Fork version policy

This fork is the WeChat provider foundation for `iugam-wechat`. Its release identifier combines the upstream base version with the current customization revision:

| Identifier | Example |
| --- | --- |
| Git tag / release title | `v0.15.1-iugam.1` |
| Package version / Rust crate version | `0.15.1-iugam.1` |
| Versioned Docker image tag | `0.15.1-iugam.1` |
| Server binary asset | `agent-server-0.15.1-iugam.1-linux-amd64` |
| Docker tools asset | `docker-tools-0.15.1-iugam.1.tar.gz` |

Version rules:

1. `<upstream-version>` identifies the upstream release used as the fork base. Record its exact upstream commit SHA and the fork commit in release notes; separately identify any additional upstream commits or backports.
2. `N` is a positive integer starting at `1`, without leading zeroes. Increment it for each new published fork revision on the same upstream base: `v0.15.1-iugam.1` → `v0.15.1-iugam.2`. Development commits do not each require a version increment.
3. After integrating and validating a new upstream release, use its version and restart the customization revision at `1`: `v0.15.1-iugam.2` → `v0.15.2-iugam.1`. Do not advance the upstream component merely to represent a fork feature or fix.
4. Preserve the fork suffix when upstream changes version files. Keep the packages in the existing Changesets fixed release group and the Rust server's `Cargo.toml` / `Cargo.lock` aligned with the chosen fork version; package version strings omit `v`.
5. Published versions and their artifacts are immutable. Release changed content under a new customization revision.

Under [Semantic Versioning](https://semver.org/#spec-item-9), `-iugam.N` is a prerelease suffix and has lower precedence than the same upstream version without the suffix. The project uses it to identify customization revisions. Pin the complete fork version or image digest in downstream provider deployments, and verify package-manager selection and publishing channels explicitly rather than relying on version ordering to select a fork build.

For each fork release, include its upstream base, retained customizations, provider compatibility changes, validation results, and any required downstream upgrade steps. Verify affected `iugam-wechat` provider integration against the candidate artifacts; report verification that could not be performed. Record the published version in the fork differences document without adding private downstream paths or deployment details.

The current `pnpm version-packages` script runs `pnpm changeset version`, synchronizes the Cargo version, and refreshes the pnpm lockfile. It has no dedicated step for selecting an upstream base and incrementing the `iugam` revision. This policy does not itself configure that automation: adapt and verify the versioning and publishing flow before the first fork release, including generated versions, changelogs, tags, and artifact names. Changesets still record package changes and their compatibility impact; the fork release version follows the base-plus-revision policy above.

## Inherited upstream publishing setup

The historical setup notes below are retained for upstream context. Their account configuration is not a fork publishing configuration.

> **Package renamed (2026-08-31):** the OpenClaw extension moved from `@agent-wechat/wechat` to `@agent-wechat/agent-wechat` so the unscoped package basename matches the plugin id `agent-wechat` (OpenClaw validates plugin ids against the npm basename and hard-fails updates on mismatch; the official catalog also reserves the `wechat` id). The first version (0.11.15) was published manually with a one-off granular token — OIDC trusted publishing only covers packages that already exist — and the token was revoked afterward. The trusted publisher for the new package is configured at https://www.npmjs.com/package/@agent-wechat/agent-wechat/access (GitHub Actions: thisnick/agent-wechat, workflow release.yml), so CI publishes via OIDC as before. Once rollout is done, deprecate the old `@agent-wechat/wechat`: `npm deprecate @agent-wechat/wechat "Renamed to @agent-wechat/agent-wechat (plugin id agent-wechat); openclaw >=2026.8 reserves the wechat id for the official Tencent plugin."`


The current `.github/workflows/release.yml` defines these outputs when its npm publishing step reports newly published packages:

1. Public npm packages: `@agent-wechat/cli`, `@agent-wechat/agent-wechat`, and `@agent-wechat/wechaty-puppet` (the other workspace packages are private).
2. Rust server binary assets for `amd64` and `arm64` on GitHub Releases.
3. A `docker-tools-<version>.tar.gz` release asset.
4. A multi-architecture Docker image at `ghcr.io/<owner>/agent-wechat`.
5. Wechaty gateway images at `ghcr.io/<owner>/agent-wechat-wechaty-gateway`, tagged by digest and combined into a multi-architecture manifest.

`<owner>` follows the repository running the workflow. These are workflow intentions; verify each published artifact and job result before declaring a release complete.

## Prepare A Release

1. Add changelog entries:

```bash
pnpm changeset
```

2. Commit the generated changeset file with your code.

3. Merge to main. The changesets GitHub Action opens a "Version Packages" PR with bumped versions and changelogs.

4. Merge the Version Packages PR to trigger publishing in a repository with publishing configured.

## What CI/CD Does

On merge of the Version Packages PR:

- Attempts to publish non-private workspace packages to npm with provenance.
- Uploads server binaries and the Docker tools archive to GitHub Releases.
- Builds and pushes `amd64` and `arm64` Docker images.
- Publishes multi-arch manifest tags for the server and Wechaty gateway images.

The npm publishing step currently captures its output with `|| true` and derives `published` from `New tag:` lines. A completed workflow alone is not proof that every package published successfully; review the publishing output and confirm package versions and release assets.

Docker tags:

- `<version>` (e.g., `0.2.0`)
- `latest`

## Trusted Publishing (OIDC)

npm trusted publishing lets GitHub Actions publish without a long-lived token. Setup:

1. Go to https://www.npmjs.com/package/@agent-wechat/cli/access
2. Under "Trusted publishers", add GitHub Actions:
   - **Owner**: `thisnick`
   - **Repository**: `agent-wechat`
   - **Workflow**: `release.yml`
   - **Environment**: (leave blank)
3. Repeat for the other public packages, including `@agent-wechat/agent-wechat` and `@agent-wechat/wechaty-puppet`.

Once configured, delete the `NPM_TOKEN` secret from GitHub repo settings. The workflow uses OIDC automatically (requires npm >= 11.5.1, installed in CI).

**Note**: Trusted publishing can only be configured for packages that already exist on npm. For brand-new packages, the first publish must use a token.

The GHCR paths in the current release and rebuild workflows already derive from `github.repository`. Fork preparation also needs to address the CLI's upstream download addresses and Compose/documentation examples listed above.
