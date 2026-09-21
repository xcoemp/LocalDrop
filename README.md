# LocalDrop

**Zero-configuration LAN file and text transfer.** Two devices on the same
network see each other within seconds. Drag files onto a peer card and the bytes
go straight over TCP, disk to disk — no accounts, no cloud, no internet.

Built with Tauri v2 (Rust + tokio) and Vue 3, targeting **Windows 10/11** and
**Android 10+**.

Jump to **[Building from a fresh clone](#building-from-a-fresh-clone)**.

---

## What it does

- **Finds peers automatically.** A UDP heartbeat every 3 s on the subnet
  broadcast address; peers appear within ~3 s and are pruned after 12 s of
  silence. No IP addresses, no pairing codes, no QR scans.
- **Sends files and whole folders.** Directory structure is preserved, empty
  directories included. Symlinks are skipped, not followed, and reported.
- **Sends text.** Links, tokens, snippets — optionally landing straight on the
  other device's clipboard. Over 1 MB it converts to a `.txt` file.
- **Streams, never buffers.** 128 KB chunks to a `.part` file, renamed only on
  success. Memory use is flat regardless of file size.
- **Tells the truth about progress.** Rate is a rolling 1-second average of
  bytes actually committed to the receiver's disk, emitted at ~10 Hz.
- **Asks before receiving.** An accept prompt with a 30 s deadline, or
  auto-accept with a badge that cannot be hidden while it is on.
- **Survives being backgrounded on Android** via a foreground service, a
  partial wake lock, and a keep-screen-on hint during transfers.

### Security posture

**v1.0 transfers are unencrypted and unauthenticated.** Any device on the LAN
can discover any other and offer it files. This is a deliberate, documented
choice rather than an oversight: the backend exposes
`encrypted: false` and the UI reads that flag, so the header chip says
_"Unencrypted · trusted LAN only"_ and the accept dialog says so too.
Implementing a Noise handshake flips one value and the labels follow.

Hardening that _is_ in place: path-traversal is blocked and unit-tested, offer
headers and manifests are size-capped, declared sizes are enforced byte-for-byte,
received files are never auto-opened or made executable, malformed frames close
the connection without partial writes, auto-accept is off by default, and
sockets bind to LAN interfaces only.

---

## Status

The first three rows and the installer were re-run on 2026-09-17; the rest
record what has been observed on real hardware.

| Area              | State                                                                          |
| ----------------- | ------------------------------------------------------------------------------ |
| Rust backend      | **Clean** — `cargo check --all-targets`, 0 errors, 0 warnings                  |
| Rust tests        | **9/9 pass** — path sanitization, SAF URI mapping, reserved names (SEC-1)      |
| Vue frontend      | **Clean** — `vp check` (47 files formatted, 30 linted) and `vue-tsc` both pass |
| Desktop app       | **Runs** — launches, ~43 MB idle, binds UDP 57321 and TCP 57322                |
| Windows installer | **Builds** — 2.5 MB `.msi` + 1.8 MB NSIS `.exe`; not yet run on a clean VM     |
| Android APK       | **Builds, installs, and runs on a physical device** (arm64)                    |
| Android signing   | **Release signing configured** via Gradle + a keystore kept out of the repo    |
| Transfers         | **Android → Windows confirmed on real hardware**, including a 12 MB video      |

Integration testing is the main gap: the 9 passing tests cover path handling
only, so nothing exercises a real socket and none of the throughput, memory, or
backgrounding targets have been measured. Cancelling a _queued_ transfer is
unverified, as is a large transfer surviving 10 minutes backgrounded.

---

## Building from a fresh clone

There is no published installer yet, so building is the only way to run it.
The steps are in dependency order — **each one needs the previous one done**,
and the Android sequence in particular cannot be reordered.

Budget **30–40 GB of disk** if you build both platforms. Cargo keeps every
superseded artifact, and the two targets compile separate dependency trees.

### 1. Install the prerequisites

| Tool                                 | Version | Notes                                                       |
| ------------------------------------ | ------- | ----------------------------------------------------------- |
| [Node](https://nodejs.org)           | 22+     |                                                             |
| [pnpm](https://pnpm.io)              | 11+     | `npm install -g pnpm`                                       |
| [Vite+](https://viteplus.dev) (`vp`) | latest  | The build tool; `dev`/`build`/`check` are its built-ins     |
| [Rust](https://rustup.rs)            | 1.77+   | Install via rustup, then **reopen your terminal**           |
| MSVC Build Tools                     | —       | With the **Desktop C++** workload and the Windows 10/11 SDK |

Reopening the terminal after rustup matters: a shell opened beforehand has no
`~/.cargo/bin` on `PATH`, and the resulting failure names `cargo metadata`
rather than the PATH.

**For Android**, additionally: the Android SDK, a **stable** NDK, and JDK 17+,
with `ANDROID_HOME` and `JAVA_HOME` set. `NDK_HOME` does _not_ need setting —
[`scripts/msvc-env.ps1`](scripts/msvc-env.ps1) finds a usable NDK itself.

### 2. Clone and install dependencies

```bash
git clone <repo-url> localdrop
cd localdrop
pnpm install
```

#### What the clone does not contain

Four things are deliberately absent, and each has a documented way to recreate
it. Nothing here blocks running the app — only step 7 and release signing.

| Missing                           | Recreate with                      | Needed for                      |
| --------------------------------- | ---------------------------------- | ------------------------------- |
| `src-tauri/gen/android/`          | `pnpm tauri android init` (step 6) | Any Android build               |
| `src-tauri/keystore.properties`   | copy `keystore.properties.example` | A distributable Android release |
| `src-tauri/localdrop-release.jks` | `keytool -genkeypair` (see below)  | A distributable Android release |
| `deploy/`                         | `pnpm deploy`                      | Nothing; it is build output     |

Config files that are gitignored ship a committed `.example` alongside them.
Copy the template, drop the suffix, fill in your own values:

```bash
cp src-tauri/keystore.properties.example src-tauri/keystore.properties
```

The keystore and its passwords are the only secrets this project has. Windows
code signing is configured through environment variables instead, so there is
no file to copy for it — see the end of this section.

### 3. Run the desktop app

```bash
pnpm tauri:dev
```

That is the whole desktop path. If it fails at the link step with
`LNK1181: cannot open input file 'dbghelp.lib'`, the SDK is installed but
rustc cannot see it — the `pnpm` scripts route through
[`scripts/msvc-env.ps1`](scripts/msvc-env.ps1) to fix exactly this, so use
`pnpm tauri:dev` rather than `cargo` or `vp` directly.

### 4. Verify your checkout

```bash
pnpm cargo test           # 9 Rust unit tests (path sanitization, SEC-1)
vp check                  # format + lint
pnpm run typecheck        # vue-tsc; not optional, see below
```

> `vp check` does **not** type-check `.vue` script blocks. Verified by injecting
> a type error into a component: `vp lint` exited 0 while `vue-tsc` correctly
> reported it. `vue-tsc` is wired into `beforeBuildCommand`, so a release build
> cannot ship type errors — do not remove it assuming Vite+ covers it.

---

**The remaining steps are Android only**, and the order is not negotiable: the
Gradle project has to exist before the kit can be applied to it, and skipping
the kit produces an app that builds and launches but never finds a peer.

### 5. Add the Rust targets and SDK packages

```bash
# Rust standard library for the four Android ABIs (~250 MB)
rustup target add aarch64-linux-android armv7-linux-androideabi \
                  i686-linux-android x86_64-linux-android

# SDK platform, build-tools, and NDK.
# Redirect stdin (`echo y| sdkmanager ...`) or the installer can hang
# indefinitely on a licence prompt with no visible output.
sdkmanager "platforms;android-36" "build-tools;36.0.0" "ndk;28.2.13676358"
```

Use a **stable** NDK, not a release candidate. Tauri v2 is best tested against
r27/r28; this project uses `28.2.13676358`.

### 6. Generate the Gradle project

```bash
pnpm tauri android init
```

This creates `src-tauri/gen/android/`.

### 7. Apply the android-kit — required

`gen/android/` has just been created from Tauri's templates, so the
project-specific Kotlin is not in it yet. Copy each file from
[`src-tauri/android-kit/`](src-tauri/android-kit/) into place:

| File                              | Destination                                                     | Without it                                                                             |
| --------------------------------- | --------------------------------------------------------------- | -------------------------------------------------------------------------------------- |
| `BuildTask.kt`                    | over `buildSrc/src/main/java/app/localdrop/kotlin/BuildTask.kt` | **Gradle build fails** — `Cannot find module '…/src-tauri/tauri'` under pnpm           |
| `AndroidManifest.permissions.xml` | merge into `app/src/main/AndroidManifest.xml`                   | No permissions; discovery and the foreground service both fail                         |
| `MainActivity.kt`                 | overwrite `app/src/main/java/app/localdrop/MainActivity.kt`     | No multicast lock lifecycle, no all-files-access prompt, white nav bar over dark UI    |
| `LocalDropNetwork.kt`             | same directory                                                  | **The radar stays empty forever, with no error** — Android drops inbound UDP broadcast |
| `LocalDropService.kt`             | same directory                                                  | Transfers die when the app is backgrounded                                             |
| `LocalDropScreen.kt`              | same directory                                                  | Long transfers drop when the screen sleeps                                             |
| `LocalDropStorage.kt`             | same directory                                                  | Folder sends walk the tree and then transfer nothing                                   |
| `proguard-jni.pro`                | `app/proguard-jni.pro`                                          | R8 strips the JNI entry points — release builds crash where debug builds work          |

Two of the kit's entries are **patches to apply by hand** rather than files to
copy, because they edit generated Gradle files:

- [`BuildTask.patch.md`](src-tauri/android-kit/BuildTask.patch.md) — also
  covered by copying `BuildTask.kt` above, which carries both of its fixes.
- [`SigningConfig.patch.md`](src-tauri/android-kit/SigningConfig.patch.md) —
  needed only for a **distributable** release. Without it the release build
  succeeds and emits `app-universal-release-unsigned.apk`, which the device
  refuses to install with only "App not installed" and no mention of signing
  anywhere in the build output. `pnpm android:dev` is unaffected.

Repeat all of this after any future `tauri android init`, which regenerates the
directory and discards it. See
[`src-tauri/android-kit/README.md`](src-tauri/android-kit/README.md) for the
per-file detail.

### 8. Build and run on a device

```bash
adb devices                      # confirm the device is attached and authorized
pnpm android:dev                 # build, install, attach the dev server
adb logcat -s LocalDropNetwork LocalDropService RustStdoutStderr
```

Both devices must be on the **same subnet**. Many consumer APs enable _client
isolation_ on guest networks, which blocks peer-to-peer traffic entirely and
looks exactly like a broken app — check with `adb shell ping <desktop-ip>`
before debugging any code.

After touching anything behind `#[cfg(target_os = "android")]`, run
`pnpm cargo:android`: a desktop `cargo test` does not compile that code at all,
so a green desktop build says nothing about it.

---

### Packaging a release

```bash
pnpm tauri:build          # raw .msi + NSIS .exe, straight from Tauri
pnpm android:build        # raw release .apk + .aab, straight from Tauri
```

Prefer the `deploy` scripts over those two. They wrap the same builds but also
sign, verify, and collect the results into `deploy/` under versioned names with
a `sha256sum -c` compatible `SHA256SUMS.txt`:

| Script                         | What it does                                                                       |
| ------------------------------ | ---------------------------------------------------------------------------------- |
| `pnpm deploy`                  | Both platforms. Windows first, so a misconfiguration fails fast                    |
| `pnpm deploy:windows`          | `.msi` + NSIS `.exe` only                                                          |
| `pnpm deploy:android`          | `.apk` + `.aab` only. Needs `src-tauri/keystore.properties`, or it skips           |
| `pnpm deploy:resign`           | No rebuild — re-verify and re-collect what is already built, in seconds            |
| `pnpm deploy:android:resign`   | Same, Android only                                                                 |
| `pnpm deploy:android:debugkey` | Sideload-only build signed with the shared SDK debug key; output named `-debugkey` |

What the wrapper adds over a raw `tauri build`:

- **Runs `zipalign` before `apksigner`.** The other order signs cleanly and then
  fails at install, showing only "App not installed" on the device.
- **Verifies every signature** and refuses to publish an artifact that fails. A
  signing config that silently did nothing is otherwise first discovered on a
  phone — which is exactly how the `keystore.properties` path bug was caught.
- **Checks each build's exit code** before collecting its output, so a failed
  build cannot ship the previous run's APK.
- **Exit 2 on partial success**, so CI cannot read "one platform silently
  missing" as a clean release.

Runs are additive across platforms: `deploy:windows` leaves Android artifacts
alone, and `SHA256SUMS.txt` is rebuilt to cover everything currently in
`deploy/`.

> On Android, the **release** APK is signed by Gradle during the build — the
> script only verifies it. So `deploy:android:resign` cannot turn an unsigned
> APK into a signed one; it will refuse and tell you to fix the Gradle config
> and rebuild. `deploy:android:debugkey` is the only script that signs an APK
> itself.

Android release signing reads `src-tauri/keystore.properties`, which is **not
committed** — create a keystore first:

```bash
keytool -genkeypair -v -keystore src-tauri/localdrop-release.jks \
  -keyalg RSA -keysize 4096 -validity 10000 -alias localdrop
```

Then copy the committed template and fill in the two passwords you just chose:

```bash
cp src-tauri/keystore.properties.example src-tauri/keystore.properties
```

```properties
# src-tauri/keystore.properties
storeFile=localdrop-release.jks
storePassword=...
keyAlias=localdrop
keyPassword=...
```

Both files go in `src-tauri/`, not the repo root — Gradle resolves them two
levels up from `src-tauri/gen/android`, and a copy anywhere else is simply not
found. Check it in seconds, without a build:

```bash
cd src-tauri/gen/android
powershell -NoProfile -ExecutionPolicy Bypass \
  -File ../../../scripts/msvc-env.ps1 ./gradlew.bat -q :app:signingReport --console=plain
```

Every `*Release` variant must show your `.jks` and alias `localdrop`. If they
show the debug keystore instead, the `SigningConfig` patch from step 7 is
missing.

Keep both backed up elsewhere. Lose them and you cannot ship an update that
upgrades an installed copy: Android treats a differently-signed APK as a
different app. For sideloading without a release key,
`pnpm deploy:android:debugkey` signs with the shared SDK debug key and names the
output `-debugkey` so it cannot be mistaken for something distributable.

Windows code signing is off unless configured, and there are three ways to do
it. In order of preference:

| Mode                           | Variables                                                   |
| ------------------------------ | ----------------------------------------------------------- |
| **Azure Artifact Signing**     | `LOCALDROP_WIN_AZURE_DLIB` + `LOCALDROP_WIN_AZURE_METADATA` |
| Certificate in the store (HSM) | `LOCALDROP_WIN_CERT_THUMBPRINT`                             |
| Certificate file               | `LOCALDROP_WIN_CERT` + `LOCALDROP_WIN_CERT_PASSWORD`        |

Azure Artifact Signing (formerly Trusted Signing) is Microsoft's managed
service: no private key ever reaches the build machine, and certificates are
minted per signature. It needs a paid Azure subscription, and Public Trust
validation requires an organization with three or more years of history — or an
individual developer in the US or Canada.

Unsigned installers work; they just show a SmartScreen warning. Signing does not
remove that warning on day one either — it lets reputation accumulate against
your identity instead of each individual file.

Full setup for both platforms, including Android signature schemes, Play App
Signing, and key rotation, is in `docs/SIGNING.md`.

---

## Architecture

```text
LocalDrop/
├── src/                     Vue 3 frontend (Pinia, Tailwind v4, TypeScript)
│   ├── components/          Peer cards, transfer rows, dialogs, file browser, shell
│   ├── views/               Radar · Send · Settings (Transfers lives in TransferCenter.vue)
│   ├── stores/              usePeerStore · useTransferStore · useSettingsStore
│   ├── composables/         useTauriEvents (Rust event bridge) · useDragDrop · useFormat
│   ├── types/protocol.ts    TypeScript mirrors of the Rust wire types
│   ├── App.vue              Shell: header, nav, view switching, global dialogs
│   └── style.css            Design tokens
│
├── src-tauri/               Rust backend and native packaging
│   ├── src/                 Application code (see below)
│   ├── capabilities/        Tauri permission grants for the main window
│   ├── icons/               App icons, generated from the brand mark
│   ├── nsis/                Windows installer hooks — adds the firewall rules
│   ├── android-kit/         Kotlin + patches to reapply after `tauri android init`
│   ├── gen/android/         Generated Gradle project (not committed)
│   └── tauri.conf.json      Window, bundle, and build configuration
│
├── scripts/
│   ├── msvc-env.ps1         Sets up MSVC + NDK env so builds work from any shell
│   └── package-release.ps1  Build, sign, verify, and collect into deploy/
│
├── resources/               Design inputs, not shipped
└── .vite-hooks/             Git hook dispatcher installed by Vite+
```

### Backend modules

```text
src-tauri/src/
├── main.rs                  Binary entry point
├── lib.rs                   Plugin setup, command registration, tray, lifecycle
├── protocol.rs              Wire types + u32-length-prefixed JSON framing
├── discovery.rs             UDP 57321 — heartbeat, listener, prune, rebind
├── transfer.rs              TCP 57322 — send and receive, 128 KB chunks
├── paths.rs                 Path sanitization + collision policy (SEC-1)
├── registry.rs              Peer table with 12 s TTL
├── state.rs                 Queue, cancellation, progress aggregation
├── settings.rs              Settings schema + atomic JSON persistence
├── history.rs               Capped transfer history
├── error.rs                 Error taxonomy — one code per failure
├── commands.rs              Tauri command surface
└── android.rs               JNI bridge; compiled only for Android
```

Both ports fall back through `+1..+9` if occupied, and the actual bound port is
advertised in the heartbeat rather than assumed by the sender.

---

## Decisions worth knowing

**Transfers are unencrypted.** The original mockups advertised "AES-256-GCM"
and "E2EE Handshake OK"; the v1.0 protocol implements neither. Rather than ship
a false security claim the backend exposes `encrypted: false` and the UI reads
it, so the labels can never drift from what the code does.

**Protocol addendum: per-entry prelude byte.** The wire format as originally
specified had no per-entry framing, which makes "skip symlinks" and "source file
deleted mid-transfer" impossible to honour together — a sender that cannot
produce a declared entry would have to pad with garbage or drop the connection.
Each non-directory entry is now preceded by `0x01` (data follows) or `0x00`
(skipped).

**Settings and history are persisted by Rust, not `plugin-store`.** The backend
reads the alias every 3 s for the heartbeat; round-tripping that through the
webview would add a race for no benefit. Writes are atomic (temp + rename).

**Android uses an in-app file browser, not the system picker.** SAF returns
`content://` URIs whose media-provider ids carry no filename, so picked photos
could only be saved as `shared-<timestamp>`. With all-files access granted,
browsing real paths avoids the problem instead of working around it. Desktop
keeps its native dialogs.

**Three plugins beyond the original stack list:** `clipboard-manager`, `opener`,
and `single-instance`. Clipboard paste, auto-copy, and show-in-folder need the
first two; the third stops a second launch from fighting over the sockets.

**`build.minify` is `"oxc"`, not `"esbuild"`.** Vite+ builds on Rolldown, where
`transformWithEsbuild` is deprecated; requesting esbuild fails the build after
every module has already transformed.

---

## Feature coverage

Implemented and traceable in the source by requirement ID (grep for e.g.
`FR-2.10` or `SEC-1`):

- **Discovery** — TTL pruning, loopback suppression, incompatible-version chips,
  rescan, offline empty state, rebind on network change
- **Transfer** — files, folders with structure, symlink skip, 128 KB streaming to
  `.part`, per-peer sequential queue with a 3-peer cap, cancel, collision
  policy, free-space precheck, path sanitization
- **Text** — snippets, clipboard paste, auto-copy, >1 MB → `.txt` conversion
- **Consent** — accept prompt with 30 s deadline, auto-accept with a persistent
  indicator, organize-by-sender / by-type
- **Telemetry** — rolling 1 s rate, 10 Hz emit, aggregate throughput, capped
  persistent history, reveal-in-folder with an existence check, typed errors
- **Settings** — alias, download directory picker, all toggles, diagnostics,
  atomic persistence
- **Android** — multicast lock, foreground service with wake lock and
  battery-exemption prompt, runtime notification permission, scoped-storage
  download directory

## Design

Tokens in `src/style.css` come from the LocalDrop design system, and class names
match the original mockups so markup can be lifted from them directly. Icons are
Lucide. The app icons in `src-tauri/icons/` are generated from the brand mark in
`resources/localdrop_logo/`.
