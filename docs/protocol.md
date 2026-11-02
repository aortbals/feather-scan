# ScanSnap iX1600 Wi-Fi protocol

Notes from live captures of ScanSnap Home talking to an iX1600 (firmware 0V00),
taken by injecting a socket logger into ScanSnap Home's `SshResident` helper.
The iX1500 work in [scansnack](https://github.com/elte0/scansnack) and
[panelbeater](https://github.com/appositeit/panelbeater) matches closely; the
differences we saw on the iX1600 are called out below.

All integers are big-endian unless noted. There is no TLS or cryptography.

## Ports

| Port | Proto | Direction | Purpose |
|---|---|---|---|
| 52217 | UDP | host → scanner | discovery probe and heartbeat |
| 53218 | TCP | host → scanner | SCSI tunnel (status polling, scanning); kept open |
| 53219 | TCP | host → scanner | control channel; one request per connection |
| 55265 | UDP | scanner → host | button notice (the port the host registered) |
| 53220 | UDP | scanner → broadcast | power-on notice |

ScanSnap Home sends discovery from UDP 55264 and listens for the button on 55265.

## Identity

A host is identified by:

- **MAC** (6 bytes): sent in every TCP request header. The scanner drops
  frames from a MAC that does not own the session.
- **IP**: sent in SCSI request headers, the register body and the 0x0210 announce.
- **host ID** (8 bytes): made up by the host when it pairs. The touch panel's
  "selected computer" is a host ID (`conn_user.select.id`).
- **user ID** (32 hex chars): owns the profiles shown on the panel.

The panel only sends the button notice to the selected host.

macOS hides link-layer addresses from unprivileged apps (`getifaddrs` returns
`02:00:00:00:00:00`). The scanner does not check the MAC against the pairing
(ScanSnap Home itself sends a randomized private Wi-Fi address), so Feather Scan
derives a stable locally administered MAC from the host ID.

## Discovery (UDP 52217)

Probe, 32 bytes:

| Offset | Content |
|---|---|
| 0 | tag: `VENS`, `ssNR` or `V2ss` |
| 4 | u32 seq: 0 = discovery, 1 = heartbeat |
| 8 | host IP |
| 12 | host MAC |
| 18 | u32 0 |
| 22 | u16 reply port (55264) |
| 24 | u16 flag: `VENS` 0x0010, `ssNR` 0x0100, `V2ss` 0x1000 |
| 26 | zero |

ScanSnap Home sends all three tags, unicast and broadcast, three times.

Reply, 144 bytes (`ssNR`):

| Offset | Content |
|---|---|
| 0 | `ssNR` |
| 16 | scanner IP |
| 20 | u32 SCSI port (53218) |
| 24 | u32 control port (53219) |
| 28 | scanner MAC (6) |
| 40 | device name, 64 bytes, NUL-padded (`iX1600-<serial>`) |
| 104 | model, 16 bytes (`ScanSnap iX1600 `) |
| 120 | host IP the scanner last saw |
| 124 | selected host ID (8) |
| 132 | serial (12) |

Heartbeat: `ssNR`, seq 1, flag 0x1000, sent as a burst of 3 packets 0.5 s apart
about every 31 s. The reply is 12 bytes `ssNR 80 01 00 00 00 00 00 00`.

## TCP framing (53218 and 53219)

The scanner speaks first: every connection starts with a 16-byte greeting
`u32 16 | "ssNR" | 0 | 0`. Sending before reading it resets the connection.

Request header, 32 bytes:

| Offset | Content |
|---|---|
| 0 | u32 total length |
| 4 | `ssNR` |
| 8 | u32 command |
| 12 | u32 0 |
| 16 | host MAC (6) + 2 zero |
| 24 | host IP on 53218, 0 on 53219 |
| 28 | u32 0 |

Response: `u32 total length | "ssNR" | i32 code | u32 status`, then the body.
On the control channel `code` is the result (negative = refused, e.g. -4 busy);
on the SCSI tunnel `status` is the SCSI status.

## Control channel (53219)

| Command | Body | Meaning |
|---|---|---|
| 0x13 | none | device name: 64-byte name, then flags |
| 0x30 | none | state, u32 at response offset 16 (ScanSnap Home sees 3 on the iX1600) |
| 0x11 | 352 bytes | register (below) |
| 0x12 | MAC, `0x01` at body 11 | unregister |
| 0x41 | JSON call | see below |

### 0x11 register

| Body offset | Content |
|---|---|
| 0 | `01 00 1e 00` |
| 4 | u32 intent: 5 = pair, 0 = connect |
| 12 | host IP |
| 16 | u32 notify port (55265) |
| 68 | local time: u16 year, u8 month, day, hour, minute, second, 0 |
| 76 | host ID (8) |
| 84 | i32 time-zone bias in seconds (UTC − local; PST = +28800) |

Response is 20 bytes; the result code is at header offset 8. ScanSnap Home
registers once with intent 0 per session. Right after another session ends the
scanner answers -4 (busy) for a few seconds; retrying works.

### 0x41 JSON calls

Request body: `u32 length of rest | u16 sub | 10 zero | u32 LE JSON length | JSON`.
Response body (right after the 16-byte header): `u32 length | u16 sub | 10 zero | u32 LE JSON length | JSON`.

| Sub | Meaning |
|---|---|
| 0x0100 | read `conn_user` (users + `select`) and `profiles` |
| 0x0110 | write users and profiles (replaces the list) |
| 0x0200 | read the profile the user tapped on the panel |
| 0x0210 | announce host: `host_address`, `function_level` 2, `current_user_id`, `host_port`, `version` 1 |
| 0x0610 | select profile |

## SCSI tunnel (53218)

Request: header with command 1 and the host IP, then:

| Body offset | Content |
|---|---|
| 0 | u32 CDB length |
| 4 | u32 bytes expected back |
| 8 | u32 bytes sent to the scanner (required for E9/D4) |
| 12 | zero |
| 16 | CDB, zero-padded to 16 bytes |
| 32 | data to the scanner |

Response: 16-byte header, 24 zero bytes, data from byte 40. Status 2 is CHECK
CONDITION; follow it with `03` REQUEST SENSE.

### Session start (ScanSnap Home)

1. 53218: INQUIRY `12 00 00 00 60 00`, VPD `12 01 f0 00 90 00`, `e7 00 01 00 00 00 00 0c 00 00`.
2. 53219: 0x13, 0x41/0x0100, 0x41/0x0210, 0x11 (intent 0), 0x30.
3. 53218: `c2` GET HW STATUS every ~250 ms for as long as the session lives.
4. 53219: 0x41/0x0210 every 10 s. UDP heartbeat burst every ~31 s.

### C2 status (48 bytes)

| Byte | Meaning |
|---|---|
| 3, bit 7 | feeder empty |
| 4, bit 0 | scan button pressed (short pulse) |
| 16 | 0x80 unregistered, 0xe0 connected, 0xe1 pairing |
| 32–39 | host ID selected on the panel |

### Button

The scanner sends a 48-byte UDP datagram to `host:55265`, three times:
`u32 0x30 | "ssNR" | u32 1 | u32 0 | u8 counter | 3 zero | scanner IP | scanner MAC`.
Only command 1 is a scan press. Repeats carry the same counter.

ScanSnap Home then reads 0x41/0x0200 (the tapped profile) and 0x41/0x0100,
and starts the scan. No 0x0610 is sent for a panel scan.

A scan started from the computer must select one of the host's profiles first
(0x41/0x0610 `{"version":1,"prof_id":…}`); without it the scanner drops the
connection at D5.

### Scan sequence (captured, 300 dpi color duplex)

```
d5 00 00 01 08 08            W 8 zero, R 8         open
d8 00 00 00 00 00                                  start
e9 00 00 00 00 00 00 20 00 00  W 32               window
   01 2c 01 2c | 00 00 28 d0 | 00 00 44 dc | 05    dpi x, dpi y, width, height (1/1200 in)
d4 00 00 00 50 00            W 80                  mode
03 00 00 00 12 00            R 18                  request sense
per sheet:
  e0 00 00 00 00 00                                feed; CHECK CONDITION + 3/80/03 = feeder empty
  28 00 00 02 00 00 <len24> 00 <n> <chunk>         front image
  03 …                                             sense: ILI, info = residual = end of image
  28 00 00 02 00 80 <len24> 00 <n+1> <chunk>       back image
  03 …
  28 00 80 00 00 80 00 00 20 00 <n+1> 00           image info, 32 bytes
d6 00 00 00 00 00                                  close (always, even after errors)
```

On the iX1600 the byte after the length is a running image number for the
whole batch (0, 1, 2, …), not the side as on the iX1500. Every READ reply starts
with two bytes `<image number> <chunk>`, then JPEG data. At 300 dpi with a 3 MiB
read length each image arrives in one chunk.

Image info: `u32 width | u32 max height | u32 width | u32 height | u16 dpi x | u16 dpi y | …`.

Images are baseline JPEG, 2608 px wide at 300 dpi (full scan width), upright,
with dark background on both sides of the paper. Length is already cropped by
the scanner.

D4 block used by ScanSnap Home for color/300 ("auto" profile):

```
00 03 01 01 d0 00 c1 80 80 80 90 80 80 00 00 00
00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 30
00 10 01 2c 01 2c 05 82 0d 00 00 00 28 d0 00 00
44 dc 04 00 …                                    (80 bytes)
```

Bytes 34–37 are dpi x/y. scansnack's iX1500 template differs at bytes 10, 13
and 40 and is also accepted.

### Sense codes

| Sense | Meaning |
|---|---|
| ILI, info = residual | end of image |
| 3/80/01 | paper jam |
| 3/80/02 | cover open |
| 3/80/03 | feeder empty |
| 3/80/07 | double feed |
| 3/80/33 | D5 without a prior profile select (host-started scan) |
| 5/26/00 | E9/D4 sent without the out-length field |
