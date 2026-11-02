<p align="center"><img src="docs/icon.png" width="128" height="128" alt="Feather Scan icon"></p>

# Feather Scan

A tiny, native replacement for ScanSnap Home with helpful AI filenames.

Feather Scan lives in the macOS menu bar and keeps a connection to your
Ricoh/Fujitsu ScanSnap over Wi-Fi. Put paper in, press **Scan** on the
scanner's touch panel, and a searchable PDF with a sensible file name lands in
a folder you choose. That's the whole job.

| | ScanSnap Home | Feather Scan |
|---|---|---|
| App size | 529 MB | 3.8 MB |
| Memory while idle | several helper processes | ~20–26 MB, one process |
| CPU while idle | — | ~0% |

## Features

- **Scan from the scanner.** Press Scan on the touch panel, just like with
  ScanSnap Home. You can also choose **Scan Now** in the menu.
- **Searchable PDFs.** Text is recognized with Apple's Vision framework and
  stored as an invisible text layer. It works in Preview, Spotlight, PDFKit and
  poppler, and any Unicode text survives copy and paste.
- **Clean, straight pages.** Each page is cropped to the paper and
  straightened, the paper is turned white and the text darker and crisper,
  all at the scanner's full resolution. Turn off "Clean up page images" in
  Settings to keep the scanner's original JPEGs untouched instead (crop and
  deskew are then applied as PDF transforms, without re-encoding).
- **Blank pages removed.** Empty back sides of double-sided scans are dropped.
- **Smart file names.** Any OpenAI-compatible endpoint (host + key, OpenAI by
  default) names each scan, for example
  `2026-08-17 - Utility Bill.pdf`. Without a key, scans get a
  timestamp name.
- **Uses the profile you tap.** Resolution, color mode and single- or
  double-sided come from the profile chosen on the touch panel.
- **Menu bar app.** The menu shows status, recent scans and the scans folder.
  You get a notification when a PDF is saved, and the app can open at login.
  While a scan runs, the feather icon rocks.

## Status

Early, but in daily use. Tested on a **ScanSnap iX1600** (firmware 0V00).

- **iX1500:** speaks the same protocol and should work.
- **iX500, iX100, iX1300:** use an older protocol variant that isn't supported yet.
- **Platforms:** macOS 14 or later on Apple silicon.

## Getting started

1. **Quit ScanSnap Home** if you have it, and turn off its login item. Only
   one app can hold the scanner at a time.
2. **Build and open** Feather Scan (below). Allow **Local Network** and
   **Notifications** when macOS asks.
3. **Pair.** If ScanSnap Home already paired this Mac, Feather Scan reuses
   that pairing. Otherwise open **Settings → Scanner**, click
   **Pair with Scanner**. Then, on the scanner's screen, tap the person icon
   in the corner and pick "Feather Scan@<this Mac>". Pairing adds this Mac to the scanner's list of computers
   and keeps everything already there; a copy of the old list is saved in
   `~/Library/Application Support/Feather Scan/backups`.
4. **Optional:** in **Settings → Smart Titles**, add an API key. It's stored in
   your Keychain. The recognized text of each scan is sent to that endpoint.
5. Put paper in the scanner and press **Scan**.

## Building

Requirements:

- Rust. The toolchain is pinned in `rust-toolchain.toml`.
- Xcode 16 or later.

```bash
./scripts/build-app.sh          # → build/Feather Scan.app
```

The app icon is drawn in code from the same feather as the menu bar icon;
`scripts/make-icon.sh` regenerates `macos/Resources/AppIcon.icns`,
`docs/icon.png` and `docs/feather.svg`.

Signing is configured in `scripts/signing.env` (gitignored). Copy it from
`scripts/signing.env.example` and set your `TEAM_ID`. The script then signs with
the best identity it finds for that team:

1. **Developer ID Application:** distributable. It's also notarized when
   `NOTARY_PROFILE` is set.
2. **Apple Development:** runs on the build machine only.
3. **Ad-hoc:** used when neither certificate is available.

To force a specific identity, set `SIGN_IDENTITY`. For notarization, create a
keychain profile once with
`xcrun notarytool store-credentials <name> --apple-id … --team-id …`.

### Releases and updates

The app checks this repo's GitHub Releases about once a day. By default a new
version installs itself, and the app relaunches, once the scanner has been idle
for a couple of minutes. With **Install updates automatically** off in
Settings → General, the update waits in the menu as **Install Update X.Y.Z**.
Updates never post notifications. **Check for Updates…** is in the menu. A
release needs:

- a tag `vX.Y.Z` matching `CFBundleShortVersionString` in the app it ships, and
- an asset named `Feather-Scan-X.Y.Z.zip` holding the notarized `Feather Scan.app`
  (the `build/Feather Scan.zip` that `build-app.sh` makes).

Installing downloads the zip, checks that the new app is signed with a
Developer ID from the same team as the running app, replaces the app in place
and relaunches it. Copies signed any other way (development, ad-hoc, a fork's
own certificate) never install someone else's build; they only link to the
download page. Forks can point the check at their own repo with the
`FeatherUpdateRepo` key in `macos/Resources/Info.plist`.

To test against a local feed:
`defaults write app.featherscan.FeatherScan updateFeedURL http://127.0.0.1:8765/latest.json`
(remove it with `defaults delete app.featherscan.FeatherScan updateFeedURL`).

## Command line

The `feather` CLI drives the same core, which is handy for testing and for headless use:

```bash
cargo build --release -p feather-cli
target/release/feather discover                  # find scanners on the network
target/release/feather run --ip 192.168.1.50     # wait for the Scan button, print events as JSON
target/release/feather scan --ip 192.168.1.50    # scan what's in the feeder now
target/release/feather pdf page1.jpg page2.jpg   # offline: JPEGs → searchable PDF
target/release/feather pair --ip 192.168.1.50 --dry-run  # show the list pairing would write
target/release/feather pair --ip 192.168.1.50    # pair without ScanSnap Home (quit the app first)
```

- **Config:** read from `~/Library/Application Support/Feather Scan/config.json`.
- **LLM key:** taken from `FEATHER_LLM_KEY` or `OPENAI_API_KEY`.
- **Debugging:** `FEATHER_TRACE=1` dumps the network traffic.

## How it works

```
┌────────────────────────┐   C API (JSON events)   ┌────────────────────────────────┐
│ SwiftUI menu bar app   │ ◀─────────────────────▶ │ Rust core (static library)     │
│ settings, Keychain,    │                         │ protocol · scan · OCR · PDF ·  │
│ notifications          │                         │ LLM titles                     │
└────────────────────────┘                         └──────────────┬─────────────────┘
                                                                  │ Wi-Fi
                                                         ┌────────┴────────┐
                                                         │ ScanSnap iX1600 │
                                                         └─────────────────┘
```

The core holds a session the way ScanSnap Home does:

- It registers with the scanner.
- It polls the scanner's status a few times a second.
- It re-announces itself every 10 seconds.
- It listens for the UDP notice the scanner sends when you press Scan.

On a press, it runs the scan sequence, collects one JPEG per side, and passes
the pages to the pipeline: find the paper and skew → recognize text → drop
blank backs → crop, straighten and clean each page → ask the LLM for a title
→ write the PDF.

### The protocol

ScanSnap scanners don't speak AirScan/eSCL. SANE only supports them over USB.
The Wi-Fi protocol was reverse-engineered for this project:

- **Captures:** we recorded ScanSnap Home's own traffic with the scanner by
  injecting a small socket logger into its background helper. The helper's
  code signature allows injected libraries.
- **What it turned out to be:** SCSI scanner commands, the same family SANE's
  `fujitsu` backend sends over USB, wrapped in a small TCP framing. There's
  also a JSON control channel and a UDP discovery/notification side.
- **What was new:** earlier public work hadn't managed to get the iX1600's
  touch-panel button to reach a third-party app. Feather Scan does.

[`docs/protocol.md`](docs/protocol.md) documents the wire format, the session
and scan sequences, and the quirks.

### Repository layout

```
crates/feather-core   Rust core (staticlib + rlib)
  proto/              discovery, control channel, SCSI tunnel, framing
  scanner.rs          scan sequence
  engine.rs           background session: keep-alive, Scan button, jobs
  imaging.rs          paper edges, skew, blank detection
  enhance.rs          crop, deskew and clean-up of page pixels
  ocr.rs              Vision OCR (macOS)
  pdf/                PDF writer and the glyphless font behind the text layer
  title.rs            OpenAI-compatible titling
  ffi.rs              C API used by the app
crates/feather-cli    the `feather` command
macos/                SwiftUI menu bar app (Swift Package Manager)
scripts/build-app.sh  builds and signs the .app
scripts/make-icon.sh  draws the app icon and SVG from the menu bar feather
docs/protocol.md      the Wi-Fi protocol
```

## Known gaps

- **Pairing is new.** It follows the steps ScanSnap Home uses, and keeps
  every computer on the scanner, but it has only been tested on one iX1600.
  The panel shows the profiles of the selected computer only; each computer
  sends its own when it's picked, so switching back to a ScanSnap Home
  computer brings its profiles back.
- **Unusual paper may not crop.** Edges are found from the thin shadow the
  scanner sees along the paper; if none is found, the page keeps the full
  scan width.
- **Rotation is limited.** Pages are turned upright or upside down, but not
  from sideways.
- **macOS only for now.** The core is portable Rust, but OCR and the UI are
  macOS-specific.

## Acknowledgements

The iX1500 reverse-engineering work in
[scansnack](https://github.com/elte0/scansnack) and
[panelbeater](https://github.com/appositeit/panelbeater) was a great head
start. SANE's `fujitsu` backend documents the underlying command set.

## Disclaimer

Feather Scan is an independent project. It is not affiliated with, endorsed
by, or supported by Ricoh, PFU or Fujitsu. ScanSnap is a trademark of PFU
Limited. Use at your own risk.
