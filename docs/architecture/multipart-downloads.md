# Multipart ROM downloads (RomM's cached ZIP)

Written for the Rust backend (`backend/src/adapters/romm/http.rs`, `services/downloads.rs`,
`domain/archive_transfer.rs`). Reference: RomM PR rommapp/romm#3167.

## What RomM does

| Request | Response |
| ------- | -------- |
| `GET …/content/<name>` without `Range` | a ZIP generated on the fly (mod_zip streaming): no `ETag`, no `Last-Modified` |
| `GET …/content/<name>` **with** `Range` | RomM builds (or reuses) one **complete cached ZIP** in a temp file, publishes it by rename, then serves it as a static file: `206`, `ETag`, `Last-Modified`, `Accept-Ranges` |

The two ZIPs are **not byte-identical** (different total lengths were measured), so a streamed partial must never
be continued from a cached response, or the reverse. Preparing a large archive can take several minutes, and RomM
answers neither `202` nor `Retry-After` for it: the request simply waits without response headers. (The `202` RomM
uses for *format conversion* is a different mechanism and is rejected as "not an archive" here.)

## What the plugin sends

* **First request** of every multipart ROM: `Range: bytes=0-` and `Accept-Encoding: identity`. It never starts with a
  plain `GET` and adds `Range` after an interruption; initial transfer and resumes always see the cached archive.
* **Resume** (only against a saved strong validator): `Range: bytes=<actual size of the partial>-`,
  `If-Range: <saved ETag, exact header value>`, `Accept-Encoding: identity`. The offset is the file's real size, never
  a stored counter. A weak `ETag` is never sent in `If-Range` and never saved.
* Single-file ROMs are unchanged (plain `GET`, optional `Range` resume).
* URL construction and encoding are unchanged (no extra percent-encoding pass).

## Three separate deadlines (`TransferTimeouts`)

| Phase | Default | Notes |
| ----- | ------- | ----- |
| connection setup (TCP + TLS) | 10 s | reqwest `connect_timeout`, applies to all requests |
| wait for response headers | 15 min (multipart) / 30 s (single file) | the whole wait; **only** download requests use the long one |
| body inactivity | 60 s | resets on every chunk; there is **no total-request deadline** (a healthy transfer may run for hours) |

Before this change the "connect" timeout (30 s) wrapped `req.send()`, which resolves when the headers arrive, so it was
really connect + header wait, and no real connect timeout existed. Heartbeat, metadata and artwork requests keep
their own short per-request timeouts.

## Archive identity (migration `013_download_archive_identity.sql`)

`download_jobs` stores `etag` (exact value), `last_modified`, `representation_length`, `cached_zip` and
`needs_recovery`. The identity is saved **before the first body byte** of a representation is written (and after a
replaced partial is truncated), so a crash cannot leave bytes the row vouches for wrongly.

## What happens to a response

| Situation | Behaviour |
| --------- | --------- |
| fresh `206` with strong `ETag` | accepted; identity saved; later resumes use `If-Range` |
| fresh `200` (stream) or weak/absent `ETag` | accepted; **no validator**, so no append resume is promised; an interruption restarts from byte 0 |
| resume `206` | appended only if `Content-Range` starts at the local size, the total equals the saved length, validators do not contradict and there is no content encoding. Otherwise **rejected** with the partial untouched and the job flagged `needs_recovery` |
| resume `200` | never appended. Once accepted it replaces the partial (truncate, then save the new identity, then write) |
| `416` | only "complete" if the local size equals the saved length, and then only after structure + CRC validation; otherwise `needs_recovery` |
| `202`, JSON/HTML body, other 2xx | rejected, nothing written (retried a bounded number of times, partial kept) |
| `401/403`, `404` | distinct auth / not-found handling (existing classification) |
| failure before headers | bounded retries (exponential back-off + jitter) with the same `Range: bytes=0-` |
| failure during the body | partial flushed and kept, retry from the real size with `If-Range` |

A complete partial (size equals the known length) is checked first (`verify_zip`: central directory + every CRC): a
valid ZIP is finished whichever representation produced it; an invalid one is discarded once and fetched again.

## Legacy partials

A multipart partial made before validators existed (migration flags such jobs `needs_recovery`) is **kept untouched**
and is never auto-resumed. The queue shows "restart needed"; the user's *Resume* (shown as *Restart*) replaces it
once RomM's response for the whole archive is accepted. No validator is attached to bytes that cannot be proven to
belong to it, and the migration discards no progress.

## One worker per job

Exactly one transfer owns a destination: the retry loop lives in `RommHttp::download` only (the service never
retries). Pause, cancel and shutdown cancel connection setup, the header wait, body reads and back-off sleeps, and a
job's slot is released only after its worker has stopped.

## UI

While the request is out and no headers have arrived the queue entry carries `phase: "waiting"` (or `"retrying"`),
`phase_since` (epoch seconds), `attempt`, `max_attempts`, `retry_delay_s`. The frontend counts elapsed time locally
and shows "Waiting for RomM to respond. A download archive may be preparing…" + "Elapsed: 2m 14s". No percentage is
invented, Pause and Cancel stay available, and the next frame without a phase switches to byte progress.

## Reverse-proxy prerequisite (not changed by the plugin)

A longer client wait cannot beat a shorter proxy timeout. Check the effective upstream response timeout in front of
RomM (for nginx `proxy_read_timeout`, default 60 s; Cloudflare closes a request that waits ~100 s for headers with a
524) and raise it to cover the longest archive preparation (the plugin waits up to 15 min). If a request dies
exactly at the proxy's limit the plugin reports `timeout`/`server_unreachable` and retries; it cannot succeed until the
proxy allows the wait (or the archive has been built once and is cached).
