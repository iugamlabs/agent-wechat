# Fork differences

This document records implemented behavior maintained by `iugamlabs/agent-wechat` so upstream integration can preserve, adapt, or retire it deliberately. Follow [CONTRIBUTING.md](../CONTRIBUTING.md) for synchronization and [AGENTS.md](../AGENTS.md) for runtime constraints. Development proposals and investigation notes are not evidence that a feature is supported.

## Initial baseline

The local checkout used to establish this guide is based on upstream commit `96c2e90c497d999bf5189a134a9b3b293731f42e` (`Version Packages`, package version `0.15.1`). At this baseline, there are no recorded fork-specific runtime changes. This describes the inspected local history, not a claim about the latest upstream or deployed containers.

## Maintained changes

The fork is maintained as the WeChat provider foundation for `iugam-wechat`. Release tags follow `v<upstream-version>-iugam.<N>` (for example `v0.16.0-iugam.1`); see [the version policy](./release.md#fork-version-policy). This is a maintenance and release convention, not an implemented runtime feature or a claim that a fork release already exists.

The following reusable fixes are maintained locally pending upstream integration. The entries describe source changes; release and downstream compatibility verification are tracked separately.

## Upstream synchronization 2026-10-15

- **Range:** Previous base `96c2e90c497d999bf5189a134a9b3b293731f42e` (`v0.15.1`). Integrated `21fe1185a01ec7f90864a529d9bb94da648255d5` (`v0.16.0`). This also includes fork commits `ff013fd` and `5c734dc`, which were not on fork `main` yet.
- **Incoming behavior:** Nested Send composer selection, explicit media expiry, server-owned type-47 sticker cache, merged-forward text, OpenClaw 2026.10 loading and channel ingress, Node 24, and the npm publish CI fixes.
- **Conflict decision:** `wechat_media.rs` decode and missing-byte failures stay `pending()`. Upstream had only added `..Default::default()` to the old empty `type: "image"` result. The CLI test script runs both the upstream media-status tests and the fork contacts client test. `ia/types.rs`, `cli.ts`, and the CI workflows merged with both sides' additions.
- **Versions:** The tree carries upstream package version `0.16.0` from that integration. The fork release tag for this base is `v0.16.0-iugam.1` on `f5c7921`.
- **Downstream:** `iugam-wechat` must map `type: "expired"` to unavailable and keep polling `pending`. Confirmed expiry is not a retryable read.

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

## Contact local types (maintained)

- **Rationale:** Broad `contactType=individual` cannot distinguish friends from other stored contacts. Preserve raw `localType`, without replacing the existing categories or expanding contact queries to chatrooms/system accounts.
- **Source:** This change; see [contact investigation and implementation](./iugam-2026-10-05-contacts-local-type-fix.md). No upstream backport source.
- **Affected paths:** Rust `ia/types.rs`, `router/contacts.rs`, `tools/wechat_contacts.rs`; shared generated `Contact` and client; CLI contact commands; Wechaty type mapping and polling cache; API reference.
- **Compatibility:** Add numeric `localType` and optional `localType=1|3|5` query filters to both contact endpoints. Unsupported filters return HTTP 400 before session/database access. Existing client positional arguments and default scope remain. CLI supports `--local-type` and displays raw types. Wechaty uses type 1 excluding official accounts for `friend`, and polling preserves that result; chat-only fallback retains its existing behavior. `iugam-wechat` consumers must use raw type rather than broad account category to infer friendship, and refresh any incorrect cache. No friendship request API or database migration is added. A new Wechaty adapter expects the updated provider's `localType` field; older servers do not supply it.
- **Release:** Pending. Integrated on upstream `0.16.0` at `21fe1185a01ec7f90864a529d9bb94da648255d5`; fork release suffix remains governed by the release policy.
- **Regression checks:** Rust in-memory SQL fixtures, response serialization and query extraction/HTTP 400 tests; `tests/contacts-client.test.cjs`; Wechaty `type-map.test.ts` and `contact-cache.test.ts`; `pnpm generate-types`, build and typecheck. AMD64/Ubuntu 22.04 isolated account checks passed for list/find filters and raw types, with built CLI/shared client verification; human confirmation of friendship/group-only identities and downstream cache migration remain separate.
- **Upstream review:** Reviewed at `21fe1185a01ec7f90864a529d9bb94da648255d5`. Upstream did not change contact queries. `localType` stays. `ia/types.rs` also received the upstream `MediaResult` expiry fields and merged without a conflict.

## JPEG trailing data and XOR recovery (maintained)

- **Rationale:** JPEG EOI may precede trailing bytes, causing the legacy last-pair XOR inference and strict end-of-file validation to reject downloaded images.
- **Source:** This change; see [media investigation and implementation](./iugam-2026-10-05-jpeg-tail-xor-fix.md). No upstream backport source.
- **Affected paths:** Rust `tools/wechat_media.rs`; `docker/tools/media-convert.py`; synthetic fixtures and Rust/Python regressions; CI codec dependencies and path filters.
- **Compatibility:** Saved XOR retains priority. JPEG inference validates the legacy end-pair candidate first, then scans at most 4096 trailing bytes plus the EOI pair, accepts at most 32 distinct candidates, and requires a unique structurally valid, fully decoded fallback. Recovery shares a three-second decoder budget across the current image and at most two sibling thumbnails, prioritizing the corresponding thumbnail; directory inspection is limited to 64 entries. EOI must be inside the XOR suffix to establish its key. JPEG normalization skips metadata, entropy stuffing and restart markers, finds structural EOI and strips bounded trailing bytes in memory before strict FFmpeg decoding. No cache files are rewritten. Missing or invalid image bytes return `pending` instead of an empty successful image. A message whose own explicit expiry timestamp has elapsed, and which has no cached bytes, returns upstream `type: "expired"` with `retryable: false` before any native download. Quality selection, native transfer deduplication/wait policy and GUI behavior are unchanged. Candidate server binaries and the updated `media-convert.py` must deploy together. `iugam-wechat` should keep polling `pending` and treat `expired` as unavailable. Its exhausted-retry UI issue is outside this fix.
- **Release:** Pending, on upstream `0.16.0` at `21fe1185a01ec7f90864a529d9bb94da648255d5`.
- **Regression checks:** Rust `.dat` fixtures for plain/trailing JPEGs, saved/missing XOR, sibling recovery, malformed lengths/keys, wrong XOR and pending results; Python JPEG structure, candidate bounds/ambiguity/timeouts and strict codec tests; PNG/GIF/WebP/WXGF checks; `python3 -m unittest discover -s tests -v`, Rust tests, and `node --test tests/media-download.test.cjs`. CI installs FFmpeg so real decode tests run. AMD64/Ubuntu 22.04 isolated API checks returned fully decoded thumbnail/best/full bytes, and all 12 codec regressions passed in that container; standard returned pending. ARM64, full release image and downstream UI verification remain unverified, as recorded in the implementation note.
- **Upstream review:** Reviewed at `21fe1185a01ec7f90864a529d9bb94da648255d5`. The four overlapping failure returns in `decrypt_and_return` and `get_image_for_quality` stay `pending()`, including their stage logs. Upstream's side of those conflicts was the pre-existing empty `type: "image"` result plus `..Default::default()` for the new expiry fields. `pending()` carries that `Default`, so `reason`, `expiresAt`, and `retryable` stay unset. Explicit expiry, the sticker cache, and merged-forward parsing are kept.
