# Fork differences

This document records implemented behavior maintained by `iugamlabs/agent-wechat` so upstream integration can preserve, adapt, or retire it deliberately. Follow [CONTRIBUTING.md](../CONTRIBUTING.md) for synchronization and [AGENTS.md](../AGENTS.md) for runtime constraints. Development proposals and investigation notes are not evidence that a feature is supported.

## Initial baseline

The local checkout used to establish this guide is based on upstream commit `96c2e90c497d999bf5189a134a9b3b293731f42e` (`Version Packages`, package version `0.15.1`). At this baseline, there are no recorded fork-specific runtime changes. This describes the inspected local history, not a claim about the latest upstream or deployed containers.

## Maintained changes

The fork is maintained as the WeChat provider foundation for `iugam-wechat`. Release tags follow `v<upstream-version>-iugam.<N>` (for example `v0.15.1-iugam.1`); see [the version policy](./release.md#fork-version-policy). This is a maintenance and release convention, not an implemented runtime feature or a claim that a fork release already exists.

No implemented runtime differences recorded yet. Add an entry in the same PR that introduces a fork change. Include reusable fixes retained locally while awaiting upstream integration as well as fork-specific features.

Use the following fields for each entry:

| Field | What to record |
| --- | --- |
| Change and status | Stable name; maintained, incorporated upstream, or retired. |
| Rationale | User need and the behavior the fork must preserve. |
| Source | Fork commit or PR; upstream source SHA or PR when applicable. |
| Affected paths | Implementation and integration points likely to overlap upstream changes. |
| Compatibility | API, configuration, schema, package, and architecture implications, including affected `iugam-wechat` provider behavior. |
| Release | First fork release containing the change, its upstream base version, and exact upstream SHA; use pending until it ships. |
| Regression checks | Test paths and commands, plus observable acceptance criteria for isolated container checks. |
| Upstream review | Last reviewed upstream SHA, overlap, and decisions to retain, adapt, or retire the change. |

Record each synchronization's full range, conflict decisions, and validation in its PR. Update entries when those decisions change maintained behavior. Keep retired entries with their reason and replacement so future merges do not reintroduce them. Include only sanitized evidence; do not commit credentials, account data, cache samples, or private infrastructure details.
