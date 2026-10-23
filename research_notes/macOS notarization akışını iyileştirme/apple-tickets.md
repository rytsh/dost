# Apple notarization tickets: multi-app submissions, containers, and universal versus thin code

Research date: 2026-10-23. Scope: Apple-documented semantics and a proposed Dost release workflow; no product code inspection, live notarization, macOS execution, or secrets handling was performed. Context (supplied by coordinator): a universal `dost.dmg` and two architecture-specific `.tar.gz` files, each holding a separately signed `Dost.app`.

## 1. Can a single submission cover multiple separately signed apps, and can the existing universal DMG cover the thin artifacts?

### Takeaway

**Yes, a submission ZIP can contain multiple separately signed bundles, and Apple documents issuing tickets for nested items and stapling each ZIP item. No, notarizing only a universal app's DMG does not automatically notarize separately built/signed thin apps merely because they have the same app name, bundle ID, version, or source.** Coverage is determined by the code-directory hashes actually included in the notarization result. Confidence: **high for documented semantics; untested for this specific Dost build**.

### Cited Findings

- Apple, *Customizing the notarization workflow*, “Upload your app to the notarization service”:
  > “The notary service generates a ticket for the top-level file that you specify, as well as for each nested file. For example, if you submit a disk image that contains a signed installer package with an app bundle inside, the notarization service generates tickets for the disk image, installer package, and app bundle.”
  — [Apple documentation](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow#Upload-your-app-to-the-notarization-service). Full text successfully fetched from the [Markdown endpoint](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow.md).

- The same document, “Export a package for notarization”:
  > “Alternatively, you can put apps, kernel extensions, and other software in a container, like a disk image, and notarize the container. The notary service accepts disk images (UDIF format), signed flat installer packages, and ZIP archives. It processes nested containers as well, like packages inside a disk image.”
  — [Apple documentation](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow#Export-a-package-for-notarization).

- Most importantly for **multi-item ZIPs**, the same document, “Staple the ticket to your distribution”:
  > “While you can notarize a ZIP archive, you can’t staple to it directly. Instead, run `stapler` against each item that you added to the archive. Then create a new ZIP file containing the stapled items for distribution.”
  — [Apple documentation](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow#Staple-the-ticket-to-your-distribution).
  This is affirmative multi-item wording, not a rule limiting submission ZIPs to one app.

- Apple, *TN3126: Inside Code Signing: Hashes*, “Code directory”:
  > “Hashing a code directory results in a code directory hash, or cdhash. This value uniquely identifies the code being signed. It crops up in a variety of places, most notably notarization. A notarized ticket is a set of cdhash values, signed by the notary service to confirm that it has checked that code for malicious components.”
  — [TN3126](https://developer.apple.com/documentation/technotes/tn3126-inside-code-signing-hashes#Code-directory). Full text fetched from [Markdown](https://developer.apple.com/documentation/technotes/tn3126-inside-code-signing-hashes.md).

- Apple DTS engineer Quinn, *Notarize the DMG and not the app?*:
  > “The ticket covers the outermost container and everything within it.”
  > “When Gatekeeper checks the outermost container, the system ingests the ticket so that it’s available to any future notarisation checks.”
  The answer also says to download the log and confirm that the ticket covers everything expected. — [Apple Developer Forums, thread 125512](https://developer.apple.com/forums/thread/125512). **Access limitation:** the search integration returned this answer text, but direct fetch hit Apple's security-verification page. Use the successfully fetched current Apple documentation and TN3126 as the primary evidence; this forum answer is corroboration.

- Apple, *Fetching the Notary Log*, says to review the log:
  > “If you encounter a trusted execution problem, to confirm that all your code was included in the notarised ticket”.
  It documents `xcrun notarytool log CREDENTIALS REQUEST_UUID`. — [Apple Developer Forums, thread 705839](https://developer.apple.com/forums/thread/705839). Search-retrieved source text; current main documentation independently documents `notarytool log` and says to check it even on success: [Apple workflow](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow#Check-the-status-of-your-request).

### Inferences

- A staging directory such as `submission/{arm64/Dost.app,x86_64/Dost.app,universal/Dost.app}`, archived with `ditto -c -k --keepParent`, is a supported application of the documented multi-item ZIP process. The directory names avoid collisions between three bundles all named `Dost.app`. Submit once, inspect `ticketContents` for every expected code hash/architecture, then staple each exact app. This is a **strong, direct application of Apple's documented behavior**, not a Dost-specific tested result. — [Apple workflow](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow); [Apple ZIP creation instructions](https://developer.apple.com/documentation/xcode/packaging-mac-software-for-distribution#Build-a-zip-archive).

- A DMG submission can cover an **unchanged copy of the exact signed app inside it** distributed separately. Staple that app before putting it into the separate archive, so its standalone distribution does not depend on a user first opening the DMG or on an online lookup. This does not extend coverage to a different app whose relevant cdhashes were not in that submission. — [Nested tickets and per-item stapling](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow); [cdhash semantics](https://developer.apple.com/documentation/technotes/tn3126-inside-code-signing-hashes#Code-directory).

- For the supplied Dost setup, **do not remove the two thin-app notarizations solely because the universal DMG succeeded**. Either explicitly include the final thin signed bundles in a common submission, or prove that the final thin code preserves already-notarized hashes and valid signatures throughout. Shared bundle identity/source is not proof of that. — [TN3126](https://developer.apple.com/documentation/technotes/tn3126-inside-code-signing-hashes).

- The supported upload-format list does not include `.tar.gz`. A temporary ZIP is a submission vehicle; the distributed tarballs should be assembled from the stapled thin apps. The archive round trip must preserve the actual bundle and stapled ticket. — [Apple accepted formats and ZIP workflow](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow).

### Gaps

- We have not inspected actual Dost hashes, notarization logs, bundle layouts, or signing commands. Whether the existing universal slices match the existing thin apps is **unknown**. Treat the context's “separately signed” outputs as distinct until measured.
- No end-to-end Dost multi-app submission was executed. Documentation supports it, but acceptance and complete ticket coverage must be checked on the actual build.
- No authoritative source specifically discussing Dost's tar creation/extraction tools was collected; metadata/ticket preservation must be verified experimentally rather than assumed.

## 2. Must a DMG assembled later be notarized, and what does “outermost container” actually mean?

### Takeaway

**A newly created DMG does not inherit its app's notarization ticket as a ticket for the DMG itself.** To retain Apple's normal signed-and-notarized DMG distribution workflow, the final signed DMG must be included in a successful submission, but it need not have its own separate submission: a temporary ZIP can contain the final signed DMG alongside the two thin apps. “Outermost” describes nested-container handling and the recommended shipped package, not a prohibition on batching. Confidence: **high for documentation and identity distinction; medium-high for the exact one-ZIP composition, which has not been run for Dost**.

### Cited Findings

- Apple, *Packaging Mac software for distribution*, “Notarize your product”:
  > “If you distribute your product directly, notarize the file you intend to distribute to your users.”
  > “If you distribute your product using nested containers, only notarize the outermost container. For example, if you have an app inside an installer package on a disk image, sign the app, sign the installer package, and sign the disk image, but only notarize the disk image.”
  — [Apple packaging documentation](https://developer.apple.com/documentation/xcode/packaging-mac-software-for-distribution#Notarize-your-product). Full text fetched from the [Markdown endpoint](https://developer.apple.com/documentation/xcode/packaging-mac-software-for-distribution.md).

- The same document, “Decide on a container format”:
  > “Sign your code, and sign each nested container that supports signing. For example, if you ship an app inside an installer package on a disk image, sign the app, create the installer package, sign that package, create the disk image, and then sign the disk image.”
  — [Apple packaging documentation](https://developer.apple.com/documentation/xcode/packaging-mac-software-for-distribution#Decide-on-a-container-format).

- The same document says a third-party-generated disk image should be signed with a **Developer ID Application** identity and be a **UDIF-format read-only zip-compressed (`UDZO`)** disk image. — [Build a disk image file](https://developer.apple.com/documentation/xcode/packaging-mac-software-for-distribution#Build-a-disk-image-file).

- The nested-ticket and ZIP-per-item-stapling passages quoted in section 1 expressly cover nested disk images and each staplable item in a ZIP. — [Apple custom workflow](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow).

- **Important exception preventing an overclaim of “every DMG must always be notarized”:** Apple's custom workflow, “Avoid long notarization response times and size limits”, explicitly says:
  > “For large uploads, try to exclude non-executable data from notarization. For example, if you use an installer package, you could move all your non-executable data into a folder next to a notarized installer package. Put both the data and the notarized installer package into a single disk image, and ship the disk image without notarization. For the best user experience, staple the notarization ticket to the installer package.”
  — [Apple documented exception](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow#Avoid-long-notarization-response-times-and-size-limits).
  This is specifically an example involving a stapled installer package and large non-executable data. It proves that outermost-DMG notarization is not a universal logical requirement, but does **not** establish an equivalent UX guarantee for Dost's app-only DMG.

- Apple DTS Quinn, *Electron and Notarization randomly failing*, proposed an experiment: sign app → ZIP → notarize ZIP → staple app → build DMG. He immediately qualified it:
  > “Now, I realise this runs counter to the general advice in Packaging Mac software for distribution, so I’m not gonna recommend that as an actual workaround, at least not yet.”
  — [Apple Developer Forums, thread 781883](https://developer.apple.com/forums/thread/781883). Search-retrieved text. The final recommendation in the retrieved answer was to fix the image format/filesystem issue, not to adopt ZIP-first/later-DMG as a general replacement for notarizing the DMG.

- Apple, *TN2206: macOS Code Signing In Depth*, “Signing Disk Images”:
  > “Disk images can be signed using the `codesign` tool on macOS 10.11.5 and later. This allows the entire disk image to be validated by Gatekeeper the first time it is mounted.”
  > “Gatekeeper will validate the contents of the disk image as well.”
  The section documents `spctl -a -t open --context context:primary-signature -v MyImage.dmg`. — [TN2206](https://developer.apple.com/library/archive/technotes/tn2206/_index.html). This archive predates notarization; use it for signature/assessment context, not as current notarization policy.

### Inferences

- **Preferred single-submission design, retaining signed/notarized final DMG:**
  1. Build and fully sign all three final bundles (including nested code), verifying the signatures before packaging.
  2. Assemble the final universal `dost.dmg`, then sign the DMG. Do this **before** notarization submission.
  3. Create a temporary staging ZIP containing `dost.dmg`, `arm64/Dost.app`, and `x86_64/Dost.app` — not the tarballs. The universal app already resides inside the DMG.
  4. Submit that ZIP once using `notarytool`, wait for **Accepted**, fetch the log, and confirm coverage of the DMG, both architectures in the universal bundle, and every relevant thin/nested code object.
  5. Staple and validate the final DMG and each thin app. Do not rebuild/re-sign the DMG afterward.
  6. Build architecture-specific tarballs from the stapled thin apps. Verify extracted copies and test real quarantined downloads on clean Intel and Apple silicon Macs, including offline use.
  This composes Apple's explicit nested-container support and per-ZIP-item stapling. It avoids any dependence on identical universal/thin cdhashes, and should reduce three submissions to **one** without leaving the final DMG uncovered. **It is a proposed documented composition, not an end-to-end verified Dost implementation.** — [Apple workflow](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow); [packaging and distribution testing](https://developer.apple.com/documentation/xcode/packaging-mac-software-for-distribution).

- **Conservative two-submission alternative:** submit one ZIP containing the three signed apps, staple apps, create and sign the DMG from the stapled universal app, then submit/staple the final DMG. It is structurally straightforward but repeats scanning of the universal app and uses two service jobs rather than one. The second job is about the newly created outer DMG, not a rule that each app requires its own job. — [Apple custom workflow](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow); [Apple packaging](https://developer.apple.com/documentation/xcode/packaging-mac-software-for-distribution).

- **Do not describe a three-app ZIP followed by a new DMG as “everything, including the DMG, notarized in one call.”** Only the apps existed in that upload; the later DMG is not among the checked container identities. It may be possible to distribute an unnotarized outer image in selected workflows, but that is a deliberate packaging tradeoff with separate validation needs, not ticket inheritance. — [Nested-ticket generation](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow#Upload-your-app-to-the-notarization-service); [documented unnotarized-DMG exception](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow#Avoid-long-notarization-response-times-and-size-limits).

- The 2019 forum advice warning against **distributing** several items in an outermost ZIP concerns choosing a safe/user-friendly distribution container. It does not invalidate the current official documentation's explicit “each item” ZIP notarization process, particularly when the batch ZIP is temporary and not shipped. — [2019 forum answer](https://developer.apple.com/forums/thread/125512); [current workflow](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow#Staple-the-ticket-to-your-distribution).

### Gaps

- The exact `ZIP(final signed DMG + two sibling thin apps)` layout has not been exercised here. No Apple passage was found naming this exact Dost-specific combination, but all constituent nesting and stapling behavior is documented.
- No clean-machine Gatekeeper experiment was run for an app-stapled-only, later-created, signed-but-unnotarized DMG. Do not claim that variant is guaranteed to fail, or guaranteed to have identical UX, on all macOS versions.
- The one-submission workflow might require CI artifact coordination (collect all signed outputs in one job); actual orchestration, latency, upload sizes, and failure tradeoffs were outside this research scope.

## 3. What does a universal ticket cover, and can thinning or re-signing safely reuse it?

### Takeaway

**Universal code has an independent signature and cdhash for each architecture. Pure removal of an architecture is expressly documented not to invalidate a signature, so claiming “lipo thinning always breaks signing/notarization” would be wrong.** However, separately built or re-signed thin bundles are not automatically the same signed code, and whole-bundle resources/nested-code layout can defeat ticket reuse. Use explicit batching of final signed artifacts unless signature- and hash-preserving thinning is deliberately implemented and validated. Confidence: **high for per-architecture signatures and permitted slice removal; conditional for whole-app notarization reuse**.

### Cited Findings

- Apple TN3126, “Code signature storage”:
  > “In a universal binary, each architecture is signed independently.”
  Its “Code directory” section adds:
  > “In a universal binary, each architecture is signed independently, each with its own code directory.”
  > “Each architecture in a universal binary is signed independently and so has different hash values. To get the hashes for a specific architecture, supply the `--arch` argument”.
  — [TN3126](https://developer.apple.com/documentation/technotes/tn3126-inside-code-signing-hashes).

- TN3126 provides these example diagnostic forms:
  ```sh
  codesign --display -vvv --arch arm64 "AppWithTool.app"
  codesign --display -vvv --arch x86_64 "AppWithTool.app"
  ```
  It notes that without `--arch`, `codesign` displays the signature for the architecture of the current machine. This makes inspecting only one default `CDHash` insufficient for a universal app. — [TN3126, Code directory](https://developer.apple.com/documentation/technotes/tn3126-inside-code-signing-hashes#Code-directory).

- Apple TN2206, “Changes That Don't Invalidate a Code Signature”:
  > “Removing the Mach-O slice for a particular architecture from a universal binary will also not invalidate the code signature.”
  — [TN2206, relevant anchor](https://developer.apple.com/library/archive/technotes/tn2206/_index.html#//apple_ref/doc/uid/DTS40007919-CH1-TNTAG402). Successfully fetched full text. This is explicit counterevidence to blanket warnings that any thinning necessarily breaks signing.

- TN3126, “Special slots” and “Resources”, explains that the code directory includes hashes of `Info.plist`, entitlements, and resources; `CodeResources` is itself hashed into the signature:
  > “So, if that file changes, the code directory hash changes and you break the seal on the code signature.”
  It distinguishes nested-code references from ordinary resource-file hashes. — [TN3126](https://developer.apple.com/documentation/technotes/tn3126-inside-code-signing-hashes#Resources).

- TN2206, “Nested Code”, warns that putting code outside standard code locations causes it to be sealed as resource data as well as signed code:
  > “Conversely, putting code into other places will cause it to be sealed as data (resource) files. This causes this code to be sealed twice; once in the nested code, and once in the outer signature.”
  — [TN2206](https://developer.apple.com/library/archive/technotes/tn2206/_index.html).

- Apple, *Resolving common notarization issues*, states:
  > “Before you can notarize an app, you must first code sign it. If you don’t, or if you make a modification to the bundle after signing, notarization fails with the following message: `The signature of the binary is invalid.`”
  — [Apple documentation](https://developer.apple.com/documentation/security/resolving-common-notarization-issues). Retrieved via search at [Markdown URL](https://developer.apple.com/documentation/security/resolving-common-notarization-issues.md). Interpret this as the general rule, with documented exceptions such as pure slice removal—not as disproving TN2206's explicit exception.

### Inferences

- If a thin artifact is produced **only** by removing other architectures while preserving the retained architecture's signed contents and required bundle metadata, its retained cdhash can remain one of the values already covered by the universal submission. On that condition, the code-identity model supports reusing the prior notarization; a standalone copy should carry a valid app-stapled ticket. **The notarization-reuse conclusion is a technical deduction from the ticket definition plus documented signature-preserving thinning, not an exact Apple promise covering every app-bundle thinning recipe.** — [TN2206 slice-removal exception](https://developer.apple.com/library/archive/technotes/tn2206/_index.html#//apple_ref/doc/uid/DTS40007919-CH1-TNTAG402); [TN3126 cdhash/ticket semantics](https://developer.apple.com/documentation/technotes/tn3126-inside-code-signing-hashes#Code-directory).

- Whole-app thinning can go beyond that safe case: removing architecture-specific files, editing `Info.plist`, changing resources, modifying code load commands, or thinning a Mach-O sealed as data can invalidate outer seals. Re-signing to repair such changes can produce new code directories/cdhashes; those newly changed hashes require notarization coverage. **Re-signing is not a logical guarantee that every cdhash changes**, because the cdhash hashes the CodeDirectory, not the entire CMS signature/timestamp; the safe rule is “do not assume reuse—compare final hashes and verify signatures.” — [TN3126 code directory and resources](https://developer.apple.com/documentation/technotes/tn3126-inside-code-signing-hashes); [TN2206 nested code and permitted changes](https://developer.apple.com/library/archive/technotes/tn2206/_index.html).

- Same source, compiler target, app version, signing certificate, or bundle identifier does not establish equal final CodeDirectory hashes. For the existing Dost outputs, collect architecture-specific hashes of **each relevant final code object**, not merely hashes of the raw executables before bundling/signing, and compare with notarization log `ticketContents`. — [TN3126](https://developer.apple.com/documentation/technotes/tn3126-inside-code-signing-hashes); [notary log coverage guidance](https://developer.apple.com/forums/thread/705839).

- Recommended validation gates for a future implementation:
  - On each final app: `codesign --verify --deep --strict --verbose=2` (use `--deep` for verification, not as a shortcut to sign).
  - Inspect `codesign --display -vvv --arch ...` for all present architectures and nested code, comparing cdhashes against the accepted log.
  - `xcrun stapler staple` and `xcrun stapler validate` on each final supported object that is to carry a ticket (DMG and thin apps).
  - Assess the final DMG with the image-specific `spctl --assess --type open --context context:primary-signature` form, and each app as an executable, but do not treat local assessment alone as an end-to-end distribution test.
  - Extract the real published tarballs, mount/download the real DMG, and test quarantined fresh copies on separate clean Intel/Apple silicon systems, including offline first use and running/copying from the DMG.
  These gates combine Apple's signing, stapling, and clean-distribution testing advice. — [TN2206 verification and disk-image assessment](https://developer.apple.com/library/archive/technotes/tn2206/_index.html); [Apple custom workflow](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow); [Apple packaging tests](https://developer.apple.com/documentation/xcode/packaging-mac-software-for-distribution#Test-your-distributed-product).

### Gaps

- No authoritative source was found explicitly guaranteeing notarization-ticket reuse after arbitrary whole-`.app` thinning across current macOS releases. The safe narrow slice-removal statement and per-architecture ticket model are documented; the complete app recipe remains conditional.
- No macOS host was used to prove Dost's final hashes before/after thinning, re-signing, tar extraction, or ticket stapling. Universal-only notarization should not replace explicit thin-output coverage without that evidence.
- Some Apple forum pages were blocked on direct fetch. Quotes from those pages are marked as search-retrieved; core conclusions rely on successfully fetched official current documentation and the full TN2206 archive, not solely search snippets.
