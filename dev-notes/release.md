# Release Process

## Fork distribution

This checkout is maintained as `iugamlabs/agent-wechat`. The inherited release setup below describes upstream distribution; it is not evidence that the fork has published packages, images, or a documentation site. Source development and upstream synchronization follow [CONTRIBUTING.md](../CONTRIBUTING.md).

Current distribution settings in the checked-in implementation:

| Setting | Current behavior | Fork release preparation |
| --- | --- | --- |
| npm package identity | Public packages retain the `@agent-wechat` names. | Decide fork-owned package names and versions, update dependencies and installation instructions, and configure publishing access. Review OpenClaw package basename/plugin ID compatibility before renaming the extension. |
| CLI container image and update downloads | `packages/cli/src/cli.ts` uses `ghcr.io/thisnick/agent-wechat` and binary assets from `thisnick/agent-wechat` releases. | Update and verify both destinations before using the CLI to distribute fork builds. |
| Compose image | The inherited standalone Compose still points at upstream GHCR. | Downstream deployments explicitly pin `ccr.ccs.tencentyun.com/deali/agent-wechat:<version>`. |
| Release and rebuild workflows | GHCR names derive from `github.repository`, while npm names come from package manifests. | Review these destinations separately; changing the GitHub owner does not change npm package ownership. |
| Changesets and package metadata | `.changeset/config.json` and public package repository fields refer to `thisnick/agent-wechat`. | Update repository attribution for fork release notes and package source links while retaining upstream credit. |
| Documentation hosting | `docs/astro.config.mjs` points to `thisnick.github.io` and the upstream repository. | Configure a fork site and verify its deployment before replacing the upstream documentation link. |

The fork container registry is `ccr.ccs.tencentyun.com/deali/agent-wechat`. Publish from a machine already logged in with `docker login ccr.ccs.tencentyun.com`:

```bash
pnpm publish:image
```

`scripts/publish-image.sh` builds one architecture, defaults to `linux/amd64`, and pushes the newest local `v*-iugam.*` tag with the leading `v` removed. It also moves `:latest` to that image. The version tag is refused when it already exists, or when `docker/` or `packages/agent-server-rust` differ from the tag. This host publishes `amd64` only. An `arm64` image is a separate build and is not part of the `amd64` tag.

Do not enable `.github/workflows/release.yml` or `docker-rebuild.yml` for this registry. Those workflows push GHCR and, on `main`, also try to publish the `@agent-wechat/*` npm packages. Fork image publishing stays on the local script until a dedicated workflow has its own CCR credentials and does not publish npm. The CLI still pulls `ghcr.io/thisnick/agent-wechat` plus the package version (`0.16.0`), so `wx up` does not select this registry. Pin the CCR version tag, or `latest`, from Compose or `AGENT_IMAGE`.

Continue adding changesets for user-facing changes and follow the fork version policy below. Do not reuse an upstream artifact's identity for different fork content.

Local image builds stay `pnpm build:image:amd64` or `pnpm build:image:arm64`. Those commands create `agent-wechat:amd64` and `agent-wechat:arm64` and do not push. The CLI has no `wx up --image` override. For an isolated container, pass an explicit image and separate volumes, ports, and credentials, then point the local CLI at it with `AGENT_WECHAT_URL` and `AGENT_WECHAT_TOKEN`. A local build does not update a running container.

## Fork version policy

This fork is the WeChat provider foundation for `iugam-wechat`. Its release identifier combines the upstream base version with the current customization revision:

The current upstream base is `0.16.0` (`21fe1185a01ec7f90864a529d9bb94da648255d5`). The first fork release on this base is `v0.16.0-iugam.1`.

| Identifier | Example |
| --- | --- |
| Git tag / release title | `v0.16.0-iugam.1` |
| Package version / Rust crate version | `0.16.0-iugam.1` |
| Versioned Docker image tag | `0.16.0-iugam.1` |
| Server binary asset | `agent-server-0.16.0-iugam.1-linux-amd64` |
| Docker tools asset | `docker-tools-0.16.0-iugam.1.tar.gz` |

Version rules:

1. `<upstream-version>` identifies the upstream release used as the fork base. Record its exact upstream commit SHA and the fork commit in release notes; separately identify any additional upstream commits or backports.
2. `N` is a positive integer starting at `1`, without leading zeroes. Increment it for each new published fork revision on the same upstream base: `v0.16.0-iugam.1` → `v0.16.0-iugam.2`. Development commits do not each require a version increment.
3. After integrating and validating a new upstream release, use its version and restart the customization revision at `1`: `v0.16.0-iugam.2` → `v<new-upstream>-iugam.1`. Do not advance the upstream component merely to represent a fork feature or fix. Do not use `-alpha.N` or a bare upstream version. The release tag for this base is `v0.16.0-iugam.1` on `f5c7921afc4ff36a3ac96fd82ee44828452084cd`.
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

## 0.16.0-iugam.2 image release

This release adds optional message `appMsgType` metadata without changing the upstream 0.16.0 base. The fixed package group and Rust crate use the full fork version; the private shared package keeps its existing package version. Generate shared types before building.

For an amd64 CCR release, verify the source commit, local subtype regressions and a separate empty-container startup, then tag that commit `v0.16.0-iugam.2` and run:

```bash
uv run --script scripts/publish-image.py --tag v0.16.0-iugam.2 --arch amd64
```

The Python script also updates `latest`; neither npm nor the inherited GHCR workflows are part of this release. Downstream providers should use subtype 6 for files, 19 for forwarded histories and 57 for quoted replies. Older responses may omit this field. An API health check does not prove live-account message retrieval.

Published on 2026-10-20 from `a74ad1a329ed`: both the CCR version tag and `latest` resolve to
`sha256:a4057a341e7bad95e161b46c8a9651c516b757c152dfeb77b9f944f57e66b890`.
The verified runtime platform is linux/amd64. Rust regressions (104), CLI (5), Wechaty (24),
workspace build/typecheck, and a separate empty-container API check passed. Downstream
iugam-wechat also verified an isolated Manager `.1` to `.2` container replacement with
volume contents and credentials retained. Live-account sends were not part of validation.
