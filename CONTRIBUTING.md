# Contributing to the iugamlabs fork

This repository is maintained at [iugamlabs/agent-wechat](https://github.com/iugamlabs/agent-wechat), with [thisnick/agent-wechat](https://github.com/thisnick/agent-wechat) as upstream. Fork features and reviewed upstream changes meet on fork `main`. Read [AGENTS.md](./AGENTS.md) for runtime boundaries and [fork differences](./dev-notes/fork-differences.md) for behavior that upstream integration must preserve. `CLAUDE.md` is a symlink to `AGENTS.md`; keep that single source of agent instructions.

## Provider role and compatibility

This fork serves as the WeChat provider foundation for the downstream `iugam-wechat` project. Maintain WeChat client control, local data access, and media capabilities here, and coordinate downstream provider integration and application behavior with `iugam-wechat`.

When a change affects behavior used by that provider, record its API, configuration, media, or deployment implications in the PR and fork differences document. Verify affected provider integration against the candidate fork build before declaring it compatible. If the downstream project is unavailable for verification, report that limit explicitly. Public documentation refers to the downstream project by name; local checkout paths, private repository links, and deployment details stay outside this repository.

Fork releases use `v<upstream-version>-iugam.<N>`, for example `v0.15.1-iugam.1`. Custom releases on the same upstream base increment `N`; moving to a new integrated upstream release resets `N` to `1`. Follow [the version policy](./dev-notes/release.md#fork-version-policy) for package versions, artifact names, and release tooling requirements. Keep the upstream base and fork revision explicit when resolving version conflicts.

## Remotes and branches

Use `origin` for the fork and `upstream` to inspect and fetch upstream changes. Check each clone before working:

```bash
git remote -v
git branch -vv
git status --short
```

Expected repository destinations:

| Remote | Repository | Purpose |
| --- | --- | --- |
| `origin` | `iugamlabs/agent-wechat` | Fork branches and PRs |
| `upstream` | `thisnick/agent-wechat` | Upstream changes for review |

If `upstream` is absent, add it with `git remote add upstream https://github.com/thisnick/agent-wechat.git`. If a remote points elsewhere, resolve that discrepancy before using the commands below.

An existing checkout may still have `main` tracking `upstream/main`. After verifying the remotes and fetching fork `main`, set local tracking and the default push destination:

```bash
git fetch --prune --no-tags origin
git branch --set-upstream-to=origin/main main
git config remote.pushDefault origin
```

These are local Git settings, not repository policy enforcement. Use explicit destinations such as `git pull --ff-only origin main` while on local `main`, and `git push -u origin <topic-branch>`. If local `main` has diverged, inspect it and preserve its commits before reconciling it; do not reset it to upstream. Upstream publishing tags are not automatically fork release tags, so the synchronization examples fetch without tags.

## Daily development

1. Start a focused `feat/*`, `fix/*`, or `docs/*` branch from current fork `main`.
2. Keep changes that could be contributed upstream separate from fork-specific policy, integration, and deployment changes. Prefer small commits and existing extension points; avoid broad file moves or formatting changes mixed with behavior changes.
3. Preserve existing API behavior where possible. Document intentional compatibility changes and update Rust-exported types with `pnpm generate-types`.
4. Add meaningful regression checks for changed behavior and an appropriate changeset for user-facing or package changes. Documentation-only edits do not need a changeset.
5. Update [fork differences](./dev-notes/fork-differences.md) when an implemented change introduces or alters fork behavior. Record why it exists, affected paths, compatibility implications, and how to verify it. Keep proposals separate from implemented features.
6. Open a PR against `iugamlabs/agent-wechat:main` describing resulting behavior and validation. An upstream contribution is a separate PR against `thisnick/agent-wechat`; prepare it from upstream's branch so it does not include unrelated fork changes.

## Choosing when to synchronize

Review upstream changes periodically and before a fork release. Prioritize relevant security fixes, correctness fixes, and compatibility updates. Larger refactors or changes to shared interfaces need time for regression checks and conflict review; record the reason when deferring them. Synchronization is a reviewed maintenance task rather than an automatic overwrite of fork `main`.

Fetch and inspect the candidate range:

```bash
git fetch --prune --no-tags origin
git fetch --prune --no-tags upstream
upstream_commit=$(git rev-parse upstream/main)
git merge-base origin/main "$upstream_commit"
git log --oneline origin/main.."$upstream_commit"
git diff --stat origin/main..."$upstream_commit"
git log --oneline "$upstream_commit"..origin/main
```

Record the merge-base SHA as the previous upstream base. The first log lists incoming commits; the three-dot diff shows upstream changes since the common ancestor. The second log includes fork commits and previous integration commits, so consult the differences document and individual patches for behavioral overlap. Review upstream release notes and changesets as well as code. If there are no incoming commits, no upstream merge is needed.

## Integrating upstream

Use a dedicated branch based on current fork `main`. A separate worktree lets unfinished work and local notes stay in the original checkout. Replace the date placeholder and choose an unused worktree path:

```bash
git worktree add -b sync/upstream-YYYY-MM-DD ../agent-wechat-sync origin/main
cd ../agent-wechat-sync
git status --short
git merge --no-ff --no-commit "$upstream_commit"
```

Run these in the same shell as the fetch commands so `upstream_commit` retains the reviewed SHA, or set it explicitly to that SHA. Begin the merge only in a clean worktree. The merge preserves upstream ancestry; `--no-commit` leaves the result available for inspection and verification.

Resolve each conflict using the intended behavior on both branches. Do not apply repository-wide `ours` or `theirs` resolution. Inspect these integration points even if Git reports no textual conflicts:

| Area | Required review |
| --- | --- |
| API and generated types | Resolve Rust source first, regenerate TypeScript exports, then check clients and adapters. |
| Dependencies and lockfiles | Resolve dependency manifests, regenerate affected lockfiles with the repository's tool versions, and review the resulting diff. |
| Database migrations | Check version collisions and schema compatibility against both branches' migration histories. Applied migrations are immutable; do not renumber or rewrite them to make Git merge cleanly. Renumbering is only appropriate for migrations confirmed never applied or shipped. Conflicting deployed histories require an explicit compatibility plan and tests using database copies before release. |
| UI and media operations | Preserve the GUI guard, destination checks, media quality and pending semantics, and voice idempotency and partial-success reporting. |
| Fork behavior | Run the regression checks recorded in the differences document, including when upstream implements a similar feature differently. |
| Fork versions and provider compatibility | Preserve `-iugam.<N>` and the reviewed upstream base; coordinate any affected `iugam-wechat` provider contract and validation. |
| CI, releases, and docs | Review publishing destinations, permissions, package identities, changeset configuration, and fork guidance. Keep upstream attribution and distinguish upstream artifacts from fork builds. |

Preserve incoming changesets. Add a new changeset for additional fork behavior introduced during conflict resolution when it is not already covered; avoid duplicating upstream release entries.

After resolving conflicts, stage only reviewed paths with `git add <resolved-path>`. Review `git diff --cached`, inspect unstaged changes, and run the checks below. Stage any reviewed generated files or documentation updates from that verification, then commit the merge with a message identifying the upstream SHA and push explicitly to the fork:

```bash
git diff --cached --check
git diff --check
git status --short
git commit -m "Merge upstream main at $upstream_commit"
git merge-base --is-ancestor "$upstream_commit" HEAD
git push -u origin sync/upstream-YYYY-MM-DD
```

If integration should be abandoned, `git merge --abort` cancels the uncommitted merge in the dedicated worktree. Preserve any useful conflict-resolution work first.

Open the synchronization PR against fork `main` and use **Create a merge commit** when integrating it. Squash or rebase merging this PR discards the upstream ancestry needed for future synchronization. Configure the fork's merge options to permit merge commits and ensure branch rules do not require linear history. If fork `main` advances before integration, merge the new `origin/main` into the synchronization branch and rerun affected checks.

Include the previous upstream base and new SHA, reasons for integrating now, overlapping fork changes, conflict decisions, tests and architecture/container coverage, and any deferred verification. Update the differences document when upstream supersedes a fork change; mark it as incorporated or retired and explain any adaptation still required. Do not assume matching feature names mean matching behavior.

For an urgent isolated fix, use `git cherry-pick -x <upstream-sha>` on a focused branch instead of a full merge. Review its dependencies and record the source SHA and tests. Later full synchronization still needs to check this patch for conflicts or duplicated behavior. Preserve published fork history throughout.

## Verification

For a full upstream integration, run the repository checks:

```bash
pnpm install --frozen-lockfile
pnpm build
pnpm typecheck
pnpm -r --if-present test
cargo check --manifest-path packages/agent-server-rust/Cargo.toml
cargo test --manifest-path packages/agent-server-rust/Cargo.toml
```

For container tool changes, also run the checks used by `.github/workflows/python-tools.yml`:

```bash
python3 -m unittest discover -s tests -v
node --test tests/media-download.test.cjs
```

Build documentation with `pnpm --filter @agent-wechat/docs build` when `docs/` changes. Run additional checks from the fork differences document. If dependency manifests were reconciled, regenerate the lockfile before the frozen install. Regenerate exported Rust types when relevant and review generated changes before committing.

For UI, media, voice, or other container behavior changes, test the built image and CLI using an isolated container and test account on the affected architecture. Use separate names, ports, volumes, and credentials; identify the destination before a send. `pnpm dev:deploy` can autodetect a container, so use `bash scripts/dev-deploy.sh --container <isolated-container>` when deploying a test binary. Local unit tests and upstream CI do not establish end-to-end behavior or cross-architecture compatibility. Report checks that could not be run and why.

## Distribution

Source integration and publishing are separate tasks. Existing npm names and CLI image/update addresses still refer to upstream; a fork merge does not create fork distribution channels. Follow [dev-notes/release.md](./dev-notes/release.md) before enabling publishing or advertising fork artifacts.
