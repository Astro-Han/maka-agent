<!--
  Licensed to the Apache Software Foundation (ASF) under one
  or more contributor license agreements.  See the NOTICE file
  distributed with this work for additional information
  regarding copyright ownership.  The ASF licenses this file
  to you under the Apache License, Version 2.0 (the
  "License"); you may not use this file except in compliance
  with the License.  You may obtain a copy of the License at

      http://www.apache.org/licenses/LICENSE-2.0

  Unless required by applicable law or agreed to in writing,
  software distributed under the License is distributed on an
  "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
  KIND, either express or implied.  See the License for the
  specific language governing permissions and limitations
  under the License.
-->

# Desktop Nightly

Desktop Nightly is an ephemeral developer snapshot, not an Apache release. It builds the current `main` commit every day so contributors can try recent Desktop changes and report problems without waiting for an ASF source-release vote.

The npm publication workflow gives each snapshot an immutable version such as `0.2.0-dev.42.20260829`. The run number is the sole ordering authority. After that exact npm version is public, it triggers Desktop Nightly with a version-only artifact; the authenticated workflow event supplies the exact source commit and upstream run. Each fresh Desktop Nightly creates an immutable `v<version>` tag and one GitHub draft prerelease containing the macOS and Windows packages, blockmaps, `dev-mac.yml`, `dev.yml`, and one offline Sigstore bundle. The workflow verifies every remote asset before it publishes the prerelease as non-Latest. Packaged Nightlies use the GitHub `dev` channel and verify that downloaded bytes were attested by `.github/workflows/desktop-nightly.yml` on `main`. A formal Desktop build uses the separate stable GitHub Release channel and formal product-release attestation identity.

Nightly currently uses the same application identity as the formal Desktop. Installing it replaces the existing Maka installation rather than creating a second side-by-side app. Its user data remains in the same location. Testers who need the formal build should reinstall that build before returning to the formal channel.

## One-time setup

1. Ask Apache Infra to allow `apache/maka` to publish the one-time compatibility bridge to `nightlies.apache.org` and whitelist the repository for the standard `NIGHTLIES_RSYNC_HOST`, `NIGHTLIES_RSYNC_KEY`, `NIGHTLIES_RSYNC_PATH`, `NIGHTLIES_RSYNC_PORT`, and `NIGHTLIES_RSYNC_USER` secrets.
2. After the checked-in `.asf.yaml` reaches `main`, verify that ASF reconciliation created the `nightly` GitHub Environment with only `main` permitted and no approval gate. Do not maintain that policy manually in GitHub. Verify that its jobs can read the five Infra-provided Nightlies secrets. Configure its macOS signing and notarization secrets: `CSC_LINK`, `CSC_KEY_PASSWORD`, `APPLE_API_KEY`, `APPLE_API_KEY_ID`, and `APPLE_API_ISSUER`. Do not expose these secrets to repository-wide or pull-request workflows.
3. Configure npm Trusted Publishing for `apache/maka` and `.github/workflows/npm-publication.yml`, restricted to the `npm-publication` Environment and with both `npm publish` and `npm stage publish` allowed. Do not create or store a long-lived npm token.
4. After npm Trusted Publishing is ready, set `NPM_NIGHTLY_ENABLED` to `true`, run `npm publication` from `main` with `channel=nightly`, and verify the exact npm version and `nightly` dist-tag. This does not depend on Desktop Infra.
5. For the one-time Nightlies-to-GitHub cutover, choose the exact `main` commit that the next manually dispatched npm Nightly will build. Set the repository variable `DESKTOP_NIGHTLY_GITHUB_CUTOVER_SOURCE_SHA` to that full 40-character commit SHA before dispatching. The Desktop workflow fails closed unless its authenticated source SHA matches this value while the public Nightlies feed still points at Nightlies payloads.
6. Set `DESKTOP_NIGHTLY_ENABLED` to `true` and manually dispatch a fresh npm Nightly from that exact `main` commit. Confirm that its successful run triggers `Desktop Nightly`. Do not rerun a failed attempt in place.
7. Verify that `v<version>` points to the exact source SHA and that its GitHub Release is published with Draft off, Prerelease on, Latest off, and exactly the nine expected assets. Verify that the public Nightlies `latest-mac.yml` and `latest.yml` now contain absolute URLs for those GitHub assets, that `versions/<version>/Maka-<version>-attestation.sigstore.json` serves the same bundle, that the Nightlies index links to GitHub downloads, and that `github-cutover.json` records the same version. This final small file is the bridge completion marker and is written only after every preceding cutover operation succeeds. Test an update from a pre-cutover Nightly on both platforms.
8. Remove `DESKTOP_NIGHTLY_GITHUB_CUTOVER_SOURCE_SHA` after the public Nightlies feed has advanced. Install the cutover build, publish one later fresh Nightly, and confirm a GitHub-to-GitHub differential update on both platforms before sharing the channel with testers.

The npm schedule starts at 18:17 UTC. Before changing the npm tag, the workflow requires its run number to exceed the current `nightly` version. Desktop separately requires its version to advance the highest valid GitHub `dev` prerelease across retained product versions. It assembles and verifies a draft before one publish mutation; a packaging, attestation, tag, upload, or digest failure leaves no partially published GitHub Release. During the one-time bridge it stages only the compatibility Sigstore bundle on Nightlies, publishes the complete GitHub prerelease, advances the legacy feed and index, and writes the completion marker last. Until that marker exists, any legacy, partial, or otherwise interrupted bridge state remains recoverable by updating the cutover variable to the next fresh source SHA and dispatching a new npm Nightly. Never rerun a failed workflow attempt in place. Once the marker exists, steady-state runs do not use Nightlies SSH or write packages, blockmaps, metadata, attestations, or bridge files there.

GitHub Release retention is intentionally outside this workflow. Do not delete an old Nightly prerelease or its tag while any installed client may need its payload or blockmap; in particular, retain the successful bridge release. Disabling `DESKTOP_NIGHTLY_ENABLED` stops new Desktop publication without mutating tags, releases, or the legacy bridge.

Remote Runtime Host setup uses the exact `maka-agent@<nightly-version>` package embedded in the Desktop manifest. The npm package is verified before Desktop artifacts become visible, so clean remote setup never depends on an unpublished Runtime Host version.
