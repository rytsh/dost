# Apple notarization queue handling and resumable GitHub CI

Research date: 2026-10-23. Scope: operational queue handling, submission concurrency, same-UUID recovery, artifact durability, and duplicate release submissions. Read-only investigation of workflow/code; no running job was queried, canceled, retried, or modified. The 49-minute DMG acceptance and 3+ hour app waits are observations supplied in the assignment, not independently verified run telemetry.

## 1. What do official sources establish about delays, concurrency, and duplicate submissions?

### Takeaway

Apple documents minutes as the normal case, but explicitly allows longer additional analysis without a maximum completion guarantee. Three distinct release artifacts can be submitted without sequentially waiting for each result; however, no published simultaneous-submission limit or promise of parallel server processing was found. Treat a long-running UUID as pending, not as justification to upload the same file again.

### Cited Findings

- **Official typical timing:** Apple's current customization guide says most software completes within **5 minutes**, **98 percent within 15 minutes**. The same page's older/general status section also says “typically takes less than an hour.” These are typical-time statements, not a per-submission SLA or upper bound. The 49-minute result exceeds the 15-minute percentile but is still within the page's broad less-than-an-hour wording. — [Apple workflow guide](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow); [full official DocC content fetched during research](https://developer.apple.com/tutorials/data/documentation/security/customizing-the-notarization-workflow.json)
- **Official reason for some long waits:** Apple's Mac notary service team says some uploads require “additional evaluation”; Apple has received such files and will complete analysis, but it may take longer than usual. Apple explicitly permits uploading a **new build with changes** while a prior upload is delayed. This is not advice to repeatedly re-upload unchanged bytes. — [Apple notary team Q&A, 2024-03-07](https://developer.apple.com/news/?id=s8eigiyj)
- **Official avoidable performance factors:** minimize file count; put non-executable resources in appropriate resource directories rather than code directories; verify disk images with `hdiutil verify`; avoid heavily compressed disk images and huge frequently changing non-executable files; **“Limit notarizations to 75 per day.”** The page does not characterize 75 as a documented concurrent ceiling, define a reset time, or describe exact enforcement. — [Apple guide, “Avoid long notarization response times and size limits”](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow#Avoid-long-notarization-response-times-and-size-limits)
- **Submission is naturally asynchronous:** `submit` defaults to `--no-wait`; `info` and `wait` accept a prior submission UUID. The tool interface does not require one submitted artifact to finish before another is uploaded. That establishes an asynchronous client workflow, **not** a guarantee about Apple worker concurrency. — [Apple-supplied notarytool manual reproduced by an independent Xcode man-page mirror](https://keith.github.io/xcode-man-pages/notarytool.1.html)
- **Avoid needless CI uploads:** Apple says notarize all versions you might distribute, including betas, not nonfunctional apps. WWDC19 also says “You probably don't need to upload every CI build.” — [Apple Q&A](https://developer.apple.com/news/?id=s8eigiyj); [WWDC19, All About Notarization](https://developer.apple.com/videos/play/wwdc2019/703/)
- **Container coverage:** Apple generates tickets for the top-level submitted file and nested software, e.g. a DMG and its embedded app. Thus the universal app inside the submitted DMG does not require an additional separate submission merely because it is also an app. This does not establish that independently built or modified thin architecture-specific apps are covered by that DMG's ticket. — [Apple workflow guide](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow)
- **Current local snapshot already avoids main+tag notarization:** `.github/workflows/ci.yml` triggers main pushes, version tags, and PRs, but certificate import and notarization are tag-only at lines 121 and 145. Its comments explicitly keep branch/PR artifacts ad-hoc signed. A main and tag push can therefore still cause two CI builds, but only the tag run is configured to contact Apple's notary service. — [Inspected local workflow](file:///home/ray/repo/dost/.github/workflows/ci.yml)
- **Current local snapshot already submits concurrently:** `scripts/notarize_macos.py` lines 47–58 construct one DMG submission plus two app ZIP submissions and use `ThreadPoolExecutor(max_workers=len(submissions))`. The DMG success staples both the DMG and universal app; each separate ZIP success staples its thin app. This differs from the assignment's description of an earlier sequential run; do not assume the running run uses the current working-tree version. — [Inspected local helper](file:///home/ray/repo/dost/scripts/notarize_macos.py)

### Inferences

- A 3+ hour `In Progress` result is unusually slow against Apple's published normal distribution, but **does not by itself diagnose rejection, an invalid Rust app, a broken signature, account configuration trouble, an outage, or a team-level hold**. Additional evaluation is an officially established possibility, not a proven cause for these submissions.
- For the three genuinely distinct deliverables, a small bounded concurrent fan-out is reasonable. It removes the client's cumulative sequential waits; it cannot guarantee that Apple finishes faster or avoid additional evaluation. Do not extrapolate the 75/day recommendation into “75 concurrent requests are supported.”
- Do not hammer `submit` to seek a better queue position. Persist and reuse the existing UUIDs. A real changed build may be submitted when needed, as Apple's Q&A explicitly permits.
- Keep notarization release-tag-only unless a particular beta build is intentionally distributed. If adding release deduplication, use release identity plus source SHA/artifact hashes and a durable submission manifest; a shared SHA alone is not proof that independently signed artifacts are identical.
- If adding GitHub concurrency controls, separate branch/PR groups from release groups and avoid auto-canceling a release solely because a later main commit arrives. Client cancellation is not a service-side cancellation mechanism; durable UUID recovery still matters.

### Gaps

- No reliable official numerical **simultaneous submissions** cap, per-team parallel-worker guarantee, queue ordering rule, or rule that first submissions always wait hours was found. Do not present folklore such as “new account manual review,” “all later requests blocked behind the first,” or “parallel uploads cause throttling” as established fact.
- Apple forum search results contained first-time/account-delay anecdotes, but several thread URLs resolved to generic forum pages; the alternate origin host failed browser verification. Consequently no inaccessible forum answer is used as authoritative evidence here.
- Actual UUIDs, current status, and the workflow revision of the observed run were outside this read-only file investigation. A green system-status page alone would not establish whether a particular submission is delayed; no live Apple outage claim is made.

## 2. Which supported commands resume the same UUID, and what output can CI rely on?

### Takeaway

Split **upload** from **wait**. Save the UUID immediately after a successful non-waiting submission; later use `info`/`wait`/`log` for that exact UUID on an authenticated machine for the same development team. `--timeout` ends client waiting, **not** Apple's processing.

### Cited Findings

- `submit` returns a UUID identifying the submission; it is needed by `info`, `wait`, and `log`, and by Apple support. IDs are scoped to the developer team; credentials for another team cannot inspect them. — [notarytool manual mirror, Submission ID and Authentication Options](https://keith.github.io/xcode-man-pages/notarytool.1.html)
- Supported status and recovery operations are `info submission-id`, `wait submission-id`, `log submission-id [output-path]`, and `history`. `history` lists the development team's earlier submissions made using notarytool. — [notarytool manual mirror](https://keith.github.io/xcode-man-pages/notarytool.1.html)
- `wait --timeout` accepts an integer plus optional `s`, `m`, or `h`, e.g. `20m`. On timeout Apple's service continues processing. `submit --wait --timeout` likewise bounds waiting, not the remote job. — [notarytool manual mirror, submit and wait](https://keith.github.io/xcode-man-pages/notarytool.1.html)
- `--output-format json` and `plist` suppress output until the subcommand completes and cannot stream progress. The normal format can display progress. This explains why wrapping a long `submit --wait --output-format json` in a capture-output subprocess can hide the UUID for hours; it need not mean the upload itself is stuck. — [notarytool manual mirror, Common Options](https://keith.github.io/xcode-man-pages/notarytool.1.html); [current helper lines 12–29](file:///home/ray/repo/dost/scripts/notarize_macos.py)
- `wait` completes on `Accepted`, `Invalid`, `Rejected`, or fatal error according to the manual. A completed command/upload must not substitute for checking that the returned status is exactly `Accepted`. `log` retrieves a JSON analysis log for a **completed** submission. Apple says always inspect the log, even for accepted uploads, because there can be warnings. — [notarytool manual mirror](https://keith.github.io/xcode-man-pages/notarytool.1.html); [Apple guide](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow)
- Staple to apps/DMGs/packages after acceptance. A ZIP cannot itself be stapled: staple its contained app, then create the distribution archive again. — [Apple guide, Staple the ticket](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow#Staple-the-ticket-to-your-distribution)

### Inferences

#### Supported command reference, without secrets

These commands are documentation, **not executed during research**. `notarytool-password` denotes an already configured keychain credential profile for the original Apple developer team, not a literal password. A new hosted runner needs secure credentials provisioned again; recovering a UUID does not require the original signing private key merely to query or staple already-signed software. Do not archive that credential profile/keychain with the build.

```sh
# Check exact interface on the selected runner's installed Xcode.
xcrun notarytool --version
xcrun notarytool help submit
xcrun notarytool help wait

# Upload ONCE. Default is no-wait; explicit --no-wait documents the intent.
xcrun notarytool submit "Dost.dmg" \
  --keychain-profile "notarytool-password" \
  --no-wait --output-format json

# Resume an existing submission; these do NOT upload the artifact again.
xcrun notarytool info "SUBMISSION_UUID" \
  --keychain-profile "notarytool-password" --output-format json

xcrun notarytool wait "SUBMISSION_UUID" \
  --keychain-profile "notarytool-password" --timeout 20m --output-format json

# Recover/disambiguate a UUID if the original terminal log/manifest was lost.
xcrun notarytool history \
  --keychain-profile "notarytool-password" --output-format json

# Retrieve analysis after completion, including accepted submissions.
xcrun notarytool log "SUBMISSION_UUID" \
  --keychain-profile "notarytool-password" "submission-log.json"

# Only after Accepted, against the preserved original signed artifact.
xcrun stapler staple "Dost.dmg"
xcrun stapler validate "Dost.dmg"
xcrun stapler staple "Dost.app"
xcrun stapler validate "Dost.app"
spctl --assess --type execute -v "Dost.app"
```

Command syntax is supported by the [notarytool manual](https://keith.github.io/xcode-man-pages/notarytool.1.html), [Apple's notarization/stapling guide](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow), and the [existing project's assessment step](file:///home/ray/repo/dost/.github/workflows/ci.yml). `SUBMISSION_UUID` is a placeholder; the actual value is the original returned `id`.

#### Exact documented output, versus illustrative JSON

Apple's workflow guide prints this **normal-format successful upload** output (the UUID/path belong to Apple's example, not Dost):

```text
Successfully uploaded file.
 id: 2efe2717-52ef-43a5-96dc-0797e4ca1041
 path: /Users/janea/Desktop/OvernightTextEditor_11.6.8.zip
```

Apple's documented **completed status** example is:

```text
createdDate: 2021-04-29T01:38:09.498Z
id: 2efe2717-52ef-43a5-96dc-0797e4ca1041
name: OvernightTextEditor_11.6.8.zip
status: Accepted
```

Apple's documented **log download** output is:

```text
Successfully downloaded submission log
 id: 2efe2717-52ef-43a5-96dc-0797e4ca1041
 location: /Users/ravipatel/Desktop/developer_log.json
```

Source for the three verbatim output examples: [Apple guide](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow), read from its [official DocC JSON](https://developer.apple.com/tutorials/data/documentation/security/customizing-the-notarization-workflow.json).

For automation choose JSON rather than scraping progress text. Expect to obtain the UUID as `id` and status as `status` when querying/waiting; the current project helper already parses these names. **Do not claim a complete frozen JSON schema** from the above normal-text examples. Specifically, preserve the raw submit/status response, validate that `id` is present, and only require fields relevant to the command. A submission/upload response need not contain a final `status`. `history` should aid human recovery, not guess “the latest UUID” in a team with concurrent submissions. — [manual's JSON output support](https://keith.github.io/xcode-man-pages/notarytool.1.html); [local parser](file:///home/ray/repo/dost/scripts/notarize_macos.py)

#### Safe recovery state machine

1. Existing UUID + `Accepted`: obtain the log, restore the associated original signed files, staple/validate/package; **no new `submit`**.
2. Existing UUID + `In Progress`: retain pending state and use a bounded `wait` or later `info`; do not turn a timeout into an automatic upload retry.
3. Existing UUID + `Invalid`/`Rejected`: download the log and investigate; do not publish. Submit a genuinely corrected artifact as a new submission when appropriate.
4. Network/authentication failure or unknown status: preserve UUID/artifacts; diagnose/retry the query. A query failure is not proof that Apple rejected or lost the upload.
5. UUID missing after an ambiguous submit failure: inspect `history` and available logs, correlate name/time/team and preserved manifest/hash evidence before deciding to upload again. Human confirmation is safer than blindly retrying an ambiguous successful upload.

This is a recommended operational policy inferred from the supported asynchronous operations and timeout semantics above, not a claim that Apple provides an atomic CI checkpoint transaction.

### Gaps

- The Linux research environment cannot run the actual runner's `xcrun`. Verify its installed help and capture real JSON fixture outputs before implementing a strict parser. The cited manual is Apple's manual content on a **third-party mirror**, not a live Apple-hosted reference; installed Xcode help takes precedence if options differ.
- No specific numeric exit code for timeout is asserted. Handle nonzero exit/error output and then query the saved UUID; do not depend on an undocumented magic code.
- No documented notarytool cancel/delete-submission command appears in this manual. Do not suggest killing the CLI cancels Apple's analysis. No analysis log being available yet is not, by itself, evidence of rejection.

## 3. How should GitHub CI preserve and resume work without re-uploading or rebuilding?

### Takeaway

The highest-value change is durable checkpoints **before waiting**, not simply parallelizing `--wait`. Preserve signed inputs first, then the UUID manifest; run bounded status/finalization attempts that consume those same inputs. A GitHub job retry is not a resume operation unless the workflow explicitly implements that behavior.

### Cited Findings

- Every GitHub-hosted job is capped at **6 hours**, after which it is terminated and fails. This includes build/sign/upload/wait/package time in the same job; increasing `timeout-minutes` cannot exceed the hosted execution limit. — [GitHub Actions limits](https://docs.github.com/en/actions/reference/limits)
- Workflow artifacts support sharing files between jobs and retaining them after jobs finish. Default artifact/log retention is 90 days, configurable subject to repository/organization limits. Cross-workflow/run downloads require the original run identifier and a suitable token. — [GitHub artifact guide](https://docs.github.com/en/actions/how-tos/writing-workflows/choosing-what-your-workflow-does/storing-and-sharing-data-from-a-workflow); [Downloading workflow artifacts](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/download-workflow-artifacts)
- `upload-artifact` v4 artifacts are immutable; do not have several jobs append to one named artifact. Direct upload/download of file trees can lose file permissions, and GitHub recommends wrapping files in tar to preserve them. Hidden files are excluded by default. — [GitHub-owned upload-artifact README, immutable artifacts / permission loss / hidden files](https://github.com/actions/upload-artifact)
- GitHub supports rerunning failed jobs or individual jobs, but a rerun uses the original `GITHUB_SHA` and `GITHUB_REF`; it does not inherently resume a stopped subprocess. Reruns are available up to 30 days after the initial run. — [GitHub rerunning workflows/jobs](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/re-run-workflows-and-jobs)
- Current workflow `macos` job has **no pre-wait signed-input artifact upload**. Its only notarization-result upload is lines 172–178, after the blocking notarization and packaging steps; final `dist/*` upload is later still. `if: always()` is present for the results upload but is not a substitute for uploading recovery data before a hard job termination. — [Local workflow lines 144–183](file:///home/ray/repo/dost/.github/workflows/ci.yml); [hard execution limit](https://docs.github.com/en/actions/reference/limits)
- Current helper captures `notarytool submit --wait --output-format json`, writes a result file only after the subprocess succeeds, and has no submit timeout or resume mode. If one future fails, the executor still waits for all other submissions. Therefore completed workers can write results, but still-pending worker UUIDs and raw inputs are not durably checkpointed by this design. — [Local helper lines 12–34, 53–66](file:///home/ray/repo/dost/scripts/notarize_macos.py)
- The current release job depends on all platform jobs succeeding; macOS packaging checks stapled thin apps when release credentials are present. This is a useful existing fail-closed release gate, not something to remove merely to make the workflow green. — [Local workflow lines 153–187](file:///home/ray/repo/dost/.github/workflows/ci.yml)

### Inferences

#### Recommended future architecture (design only; no changes made)

1. **Build/sign/package once.** Produce the final signed DMG, universal app, and independently signed architecture apps/ZIP inputs. Before any long wait, upload a dedicated recovery archive containing the signed artifacts and a manifest with source SHA, release tag, original run ID/attempt, artifact names, sizes, and SHA-256 hashes. Archive app trees with appropriate macOS-aware tooling and verify restoration; do not rely on direct upload of `.app` trees to preserve executable bits, symlinks, or metadata. Never include `.p12`, a signing keychain, passwords, environment dumps, or API private keys.
2. **Submit without waiting.** For each distinct required input, run `submit --no-wait --output-format json`, immediately save/print the returned UUID and map it to its input hash and staple target(s). Upload each checkpoint promptly, or complete a short submission job that uploads the full manifest before a separate waiting job. Preserve partial successes even when a different submission fails. Unique per-artifact/per-attempt checkpoint names avoid immutable-artifact collisions.
3. **Wait with a budget well below six hours.** Example policy, not an Apple requirement: 20–30 minute waits in a dedicated attempt, leaving explicit time for log/checkpoint upload. Stop with a clearly described pending state if still in progress; never publish pending artifacts. A 20-minute wait timeout is operational scheduling, not a rejection threshold.
4. **Resume/finalize independently.** A trusted manual `workflow_dispatch` recovery workflow can accept the original run ID and validated manifest reference, download preserved inputs, query/wait on the original UUIDs, then staple/package once all required inputs are `Accepted`. It should not rerun signing/build steps. Authenticate for the same Apple team again; do not transfer old signing credentials through artifacts. If automatic continuation is later desired, keep it bounded and secure, rather than consuming a macOS runner for hours of sleep.
5. **Keep provenance and release gating.** Restrict recovery to the intended trusted repository/tag/SHA and matching artifact hashes. Use `actions: read` for cross-run artifact reads and narrowly scoped release-write permissions only where publishing is required. Distinguish draft/pending from a published successful release. Retain separate submission-state, signed-input, analysis-log, and final-distribution artifacts so they cannot be mistaken for one another.
6. **Retry only the unfinished phase.** A failed stapling/download/query should retry that phase. A CI timeout with still-pending UUIDs should resume those UUIDs. A rerun of the current `macos` job would execute the build/sign/submit sequence again; it is not the desired no-reupload recovery path.

The reasoning for this design combines [GitHub artifact durability/limits](https://docs.github.com/en/actions/how-tos/writing-workflows/choosing-what-your-workflow-does/storing-and-sharing-data-from-a-workflow), [notarytool asynchronous UUID operations](https://keith.github.io/xcode-man-pages/notarytool.1.html), and [Apple's requirement to staple the accepted software rather than the ZIP](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow).

#### Read-only operator commands for preserved state

If the required artifacts already exist, GitHub documents these downloads; they do not rerun or mutate jobs:

```sh
gh run download RUN_ID -n notarization-results
gh run download RUN_ID -n SIGNED_INPUT_RECOVERY_ARTIFACT
```

The second artifact name is a proposed placeholder, **not an artifact currently uploaded by this workflow**. Download in a fresh local directory to avoid accidental file collisions. Then use the same-UUID commands in section 2. — [GitHub artifact download syntax](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/download-workflow-artifacts)

If the original signed inputs were never uploaded and the hosted runner has disappeared, `history` may recover UUIDs/status, **but it cannot reconstruct the original DMG/app from Apple's service using the commands researched here**. Do not promise “no reupload recovery” when only an ID survives and the original signed deliverable is gone. A later rebuild/re-sign is not assumed to reproduce the original notarization identity.

### Gaps

- Preservation cannot be retroactively added to an already-running job by editing the repository workflow. The current live job's local signed files might still exist, but accessing/mutating that runner was explicitly outside scope.
- A fully atomic “upload to Apple plus durable GitHub checkpoint” is not provided by these tools. There remains a small crash window after Apple accepts an upload but before its UUID manifest is durably uploaded; history and human correlation are recovery fallbacks, not a guarantee.
- This research does not prove that artifacts from the described run already include usable signed inputs; the checked-in workflow does not upload them before notarization. Nor does it establish that an already-accepted universal DMG covers separately built thin apps.
