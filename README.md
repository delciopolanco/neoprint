# neoprint — silent-print sidecar

Local HTTP bridge that lets neocrm (browser) hand jobs to the OS printer
spooler without opening the browser print dialog. Same UX as QZ Tray;
zero license fee, zero per-page authorization prompt, no auth token to
paste.

## Build

```bash
# from repo root
npm run neoprint:build
# OR directly:
cd apps/neoprint
cargo build --release
```

The binary lands at `target/release/neoprint` (`neoprint.exe` on Windows). End users
install the signed installers instead (see _Installers_ below); CI builds them on
every push (see _Where this code lives_).

Download URLs (used by neocrm's "Descargar neoprint" button), overwritten on every
publish, Cache-Control 5 min:

- Windows: `https://<AWS_S3_BUCKET_NAME>.s3.<AWS_REGION>.amazonaws.com/neoprint/latest/neoprint-setup.exe`
- macOS: `https://<AWS_S3_BUCKET_NAME>.s3.<AWS_REGION>.amazonaws.com/neoprint/latest/neoprint-macos.pkg`

CI artifacts (per run, 30-day retention) are also available from the Actions tab.

Required GitHub Actions secrets for the S3 upload:
`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_REGION`,
`AWS_S3_BUCKET_NAME`. IAM user needs `s3:PutObject` on
`neoprint/latest/*` (ACLs are disabled on modern buckets — no
`s3:PutObjectAcl` needed).

**Bucket policy** — attach to the bucket to allow anonymous GET on the
`neoprint/latest/` prefix only. Everything else stays private:

```json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Sid": "PublicReadNeoprintLatest",
      "Effect": "Allow",
      "Principal": "*",
      "Action": "s3:GetObject",
      "Resource": "arn:aws:s3:::neogeniux-downloads/neoprint/latest/*"
    }
  ]
}
```

**Block Public Access** — the bucket-level setting must be relaxed
enough for the policy above to take effect. In the S3 console
(Permissions tab → Block public access → Edit), uncheck at least:

- "Block public access to buckets and objects granted through _new_
  public bucket or access point policies"
- "Block public and cross-account access to buckets and objects
  through _any_ public bucket or access point policies"

Leave the two ACL-related toggles enabled — this repo does not rely on
per-object ACLs.

## Where this code lives

The source of truth is `apps/neoprint` in the neogeniux monorepo. On every push to
`main` that changes it, `.github/workflows/neoprint-sync.yml` copies it (one
snapshot commit, no history) to the standalone repo `delciopolanco/neoprint`.

Builds are split by platform, and each publishes its own update manifest, so the
two never race:

| Platform | Built + signed in                                                                        | Publishes                                    |
| -------- | ---------------------------------------------------------------------------------------- | -------------------------------------------- |
| macOS    | the monorepo (`.github/workflows/neoprint-build.yml`, Apple certs live there)            | `neoprint-macos.pkg`, `version-macos.json`   |
| Windows  | the standalone repo (`mirror-ci/build.yml`, copied to its `.github/workflows/build.yml`) | `neoprint-setup.exe`, `version-windows.json` |

Make changes in the monorepo; edits made directly in the standalone repo are
overwritten by the next sync. The standalone repo only needs the four `AWS_*` secrets
(plus the optional Windows certificate ones); setup is documented at the top of the
sync workflow.

## Installers (what cashiers download)

| OS      | File                 | Built by                         | Notes                                                                                                                                        |
| ------- | -------------------- | -------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| Windows | `neoprint-setup.exe` | Inno Setup (`installer/windows`) | Per-user, no admin. Installs to `%LOCALAPPDATA%\Programs\Neoprint`, adds the `HKCU\...\Run` autostart, launches neoprint, tray icon appears. |
| macOS   | `neoprint-macos.pkg` | `pkgbuild` (`installer/macos`)   | Postinstall runs `neoprint --install --autostart` as the logged-in user (per-user LaunchAgent). Menu-bar icon, no Dock icon.                 |

S3 keys: `neoprint/latest/neoprint-setup.exe` and `neoprint/latest/neoprint-macos.pkg`
(the raw `neoprint.exe` / `neoprint-macos` stay published for one release).
**Deployment config:** point neocrm's `VITE_NEOPRINT_DOWNLOAD_URL` at the `.exe`
installer and `VITE_NEOPRINT_DOWNLOAD_URL_MAC` at the `.pkg`.

Publishing needs only the four `AWS_*` secrets in the standalone repo. Signing secrets
are all optional; unsigned builds still work, with OS warnings (without
`NEOPRINT_APPLE_CERT_P12_BASE64` the macOS binary and `.pkg` ship unsigned):
`NEOPRINT_WINDOWS_CERT_PFX_BASE64` + `NEOPRINT_WINDOWS_CERT_PASSWORD` (signtool),
and for the pkg a **Developer ID Installer** certificate:
`NEOPRINT_APPLE_INSTALLER_CERT_P12_BASE64`, `NEOPRINT_APPLE_INSTALLER_CERT_PASSWORD`,
`NEOPRINT_APPLE_INSTALLER_IDENTITY` (this is a different cert from the Developer ID
_Application_ cert used for the binary).

## Updates

CI bakes `NEOPRINT_UPDATE_URL` (the platform's S3 `version-windows.json` or
`version-macos.json`: `{version, url, sha256}`) into each build and publishes that
manifest last. The tray polls it
every 6 h; when a newer version exists the menu shows **Actualizar a vX**. Windows runs
the downloaded installer silently (`/VERYSILENT`), which stops, replaces and relaunches
neoprint; macOS opens the `.pkg`. The download must be HTTPS and match the manifest's
SHA-256. Builds without the URL (local `cargo build`) never check.

## Security

The API is bound to loopback AND checks the browser `Origin` header: allowed are
`localhost`/`127.0.0.1` (any port), `https://neogeniux.com` + `https://*.neogeniux.com`,
and exact origins listed in `NEOPRINT_ALLOWED_ORIGINS` (comma separated). Any other
origin gets 403, Chrome's Private Network Access preflight is answered, request bodies
are capped at 25 MB, and a printer name must match an OS printer before it reaches
`lp` / PowerShell. Requests without an `Origin` (curl, native tools) are allowed.

## Code signing policy

Free code signing for the Windows installer is provided by
[SignPath.io](https://signpath.io), with a certificate from the
[SignPath Foundation](https://signpath.org). The macOS package is signed and notarized
with Neogeniux's own Apple Developer ID.

**What is signed.** The Windows `neoprint.exe` and `neoprint-setup.exe`, built from this
repository's source by the CI workflow in `.github/workflows/build.yml` on
GitHub-hosted runners. Nothing is signed from a developer's machine, and no binary
that was not produced from this repository is ever submitted for signing.
SumatraPDF, which the installer bundles for silent PDF printing, is third-party
software (GPLv3, <https://github.com/sumatrapdfreader/sumatrapdf>) pinned by SHA-256
in the build; it is not re-signed or modified.

**Team roles.**

| Role                                 | Who                                                |
| ------------------------------------ | -------------------------------------------------- |
| Authors / committers                 | [Delcio Polanco](https://github.com/delciopolanco) |
| Reviewers                            | [Delcio Polanco](https://github.com/delciopolanco) |
| Approvers (release signing requests) | [Delcio Polanco](https://github.com/delciopolanco) |

Every signing request is approved manually by an approver. This list will be updated if
more maintainers join.

**Privacy policy.** neoprint does not collect, store or transmit personal data, and it
has no telemetry or analytics.

- It listens only on `127.0.0.1:9787` (the local machine, never the network) and only
  answers requests from the allowed web origins listed under _Security_ above.
- Print jobs go from your browser to this local service and then to the printer you
  selected; they are not sent anywhere else.
- Logs stay on your computer (`%LOCALAPPDATA%\Neoprint\logs`, or
  `~/Library/Logs/Neoprint` on macOS).
- The one outgoing connection is the **update check**: about every six hours the tray
  app downloads a small `version-<platform>.json` file from the project's download
  server to see whether a newer installer exists. It sends no information about you or
  your machine beyond what any HTTPS request carries (your IP address). If a newer
  version exists it is downloaded only when you choose "Actualizar", over HTTPS, and
  verified against a SHA-256 hash. Set the environment variable
  `NEOPRINT_DISABLE_UPDATE_CHECK=1` to turn the update check off completely.

This program will not transfer any other information to other networked systems
unless specifically requested by the user or the person installing or operating it.

## Run

```bash
npm run neoprint:dev
# OR directly:
./target/release/neoprint
```

**Windows runtime:** the exe is built with a statically linked C runtime
(`.cargo/config.toml`), so it runs on a fresh Windows install without the
Visual C++ Redistributable. CI fails the build if `VCRUNTIME140`/`MSVCP140`
ever appear in the import table. It is also a _windowless_ background app (no
console): a double-click shows a message box on first install, and logs go to
`%LOCALAPPDATA%\Neoprint\logs` (macOS: `~/Library/Logs/Neoprint`, last 7 days).
Launching it a second time just tells the user it is already running.

Listens on `127.0.0.1:9787`. Loopback-only bind = only local processes
on this machine can reach it. No token, no config paste. neocrm hits
the URLs directly.

## API

No auth headers required. Loopback bind IS the trust boundary.

| Method | Path       | Body                                | Purpose                 |
| ------ | ---------- | ----------------------------------- | ----------------------- |
| GET    | /health    | —                                   | Readiness probe         |
| GET    | /printers  | —                                   | List OS printers        |
| POST   | /print/raw | `{"printer":"…","bytesBase64":"…"}` | Send raw ESC/POS bytes  |
| POST   | /print/pdf | `{"printer":"…","pdfBase64":"…"}`   | Print PDF via OS driver |

## Platform notes

- **macOS**: uses `lp` under the hood for PDF. Ensure CUPS is running
  (default on macOS). Printer picker uses the CUPS queue name (spaces
  underscored) resolved from the display name automatically.
- **Windows**: PDFs print through a bundled SumatraPDF (`-print-to`, silent, no
  Adobe needed; the installer ships it, pinned by SHA-256 in CI, GPLv3). There is
  no fallback to the `PrintTo` verb (it opens Adobe Reader when a printer is
  unavailable): if SumatraPDF is missing or fails, the request errors and neocrm
  opens the browser print dialog. Raw ESC/POS uses the `printers` crate directly.
  Temp PDFs older than 1 h are swept on the next PDF print.
- **Linux**: same as macOS (CUPS).

## Autostart

Windows: the binary self-installs on first run. Just **double-click
`neoprint.exe`** from the Downloads folder. On the first launch it:

1. Copies itself to `%LOCALAPPDATA%\Neoprint\neoprint.exe`.
2. Drops a `Neoprint.lnk` shortcut in the current user's Startup
   folder (`shell:startup`) so the sidecar boots on every login.
3. Drops through to serve, so the sidecar is ALSO usable in the
   current session (keep the console window open or minimize it —
   closing it kills the process until next login).

Subsequent runs from the install directory just serve; the installer
is a no-op. Advanced usage:

```powershell
neoprint.exe                        # first-run auto-install + serve; subsequent runs just serve
neoprint.exe --serve                # skip the first-run installer, serve in-place
neoprint.exe --install               # explicit install: prompts for autostart
neoprint.exe --install --autostart   # explicit install + Startup shortcut
neoprint.exe --install --no-autostart# explicit install without autostart
neoprint.exe --uninstall             # remove shortcut + installed binary
```

Set `NEOPRINT_SKIP_INSTALL=1` in the environment to disable the
first-run auto-installer without passing `--serve`. The Startup
shortcut launches the window minimized; console still exists but does
not steal focus. Fully hidden launcher = signed-installer follow-up.

macOS: the binary also self-installs on first run. Double-click **won't
work** out of the box because Gatekeeper blocks unsigned binaries — the
first launch has to happen from Terminal:

```bash
cd ~/Downloads
chmod +x neoprint-macos
xattr -d com.apple.quarantine neoprint-macos
./neoprint-macos
```

First run installs to `~/Library/Application Support/Neoprint/neoprint`,
registers a LaunchAgent at
`~/Library/LaunchAgents/com.neogeniux.neoprint.plist`, then drops through
to serve. Subsequent logins boot the sidecar automatically via
`launchctl`. Logs land in `~/Library/Logs/neoprint.log` +
`~/Library/Logs/neoprint.err.log`.

Same CLI flags apply (`--install`, `--install --autostart`,
`--install --no-autostart`, `--uninstall`, `--serve`,
`NEOPRINT_SKIP_INSTALL=1`).

Linux: no installer yet. Run the binary manually or wire a systemd user
unit.

macOS / Linux: no installer yet. Run the binary manually or wire a
`LaunchAgent` plist / systemd unit.

## Signing

See _Code signing policy_ above. Unsigned or not-yet-reputable builds show
SmartScreen's "Windows protected your PC" (click _More info_ → _Run anyway_); the
unsigned macOS package needs right-click → _Open_ → _Open_.

## Fallback to the browser dialog

There is no manual switch. Whenever the silent path cannot print (neoprint not
running, printer renamed/offline, PDF render or sidecar error) neocrm falls back
to the browser print dialog on its own and warns the operator, so a job is never
lost. A slot with no printer chosen always uses the dialog.
