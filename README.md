# Feather Scan

A tiny, native replacement for ScanSnap Home with helpful AI filenames.

Feather Scan lives in the macOS menu bar and keeps a connection to your
Ricoh/Fujitsu ScanSnap over Wi-Fi. Put paper in, press **Scan** on the
scanner's touch panel, and a searchable PDF with a sensible file name lands in
a folder you choose. That's the whole job.

| | ScanSnap Home | Feather Scan |
|---|---|---|
| App size | 529 MB | 2.1 MB |
| Memory while idle | several helper processes | ~20–26 MB, one process |
| CPU while idle | — | ~0% |

## Features

- **Scan from the scanner.** Press Scan on the touch panel, just like with
  ScanSnap Home. You can also choose **Scan Now** in the menu.
- **Searchable PDFs.** Text is recognized with Apple's Vision framework and
  stored as an invisible text layer. It works in Preview, Spotlight, PDFKit and
  poppler, and any Unicode text survives copy and paste.
- **High quality, no re-compression.** Each page keeps the scanner's original
  JPEG. Cropping to the paper and straightening skewed pages are applied as PDF
  transforms, so no pixels are re-encoded.
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

1. **Pair once with ScanSnap Home** and select this Mac on the scanner's
   screen. Feather Scan reuses that pairing (see [Known gaps](#known-gaps)).
2. **Quit ScanSnap Home** and turn off its login item. Only one app can hold
   the scanner at a time.
3. **Build and open** Feather Scan (below). Allow **Local Network** and
   **Notifications** when macOS asks.
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

## Command line

The `feather` CLI drives the same core, which is handy for testing and for headless use:

```bash
cargo build --release -p feather-cli
target/release/feather discover                  # find scanners on the network
target/release/feather run --ip 192.168.1.50     # wait for the Scan button, print events as JSON
target/release/feather scan --ip 192.168.1.50    # scan what's in the feeder now
target/release/feather pdf page1.jpg page2.jpg   # offline: JPEGs → searchable PDF
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
blank backs → ask the LLM for a title → write the PDF.

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
  ocr.rs              Vision OCR (macOS)
  pdf/                PDF writer and the glyphless font behind the text layer
  title.rs            OpenAI-compatible titling
  ffi.rs              C API used by the app
crates/feather-cli    the `feather` command
macos/                SwiftUI menu bar app (Swift Package Manager)
scripts/build-app.sh  builds and signs the .app
docs/protocol.md      the Wi-Fi protocol
```

## Known gaps

- **No pairing of its own yet.** Feather Scan needs a pairing that ScanSnap
  Home made once.
- **Paper-edge detection needs contrast.** It looks for paper that is brighter
  than the scanner's light-gray background. Grayish paper keeps the full scan
  width.
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
