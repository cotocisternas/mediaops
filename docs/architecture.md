# Architecture

Home is a Kubernetes-shaped control plane. The seedbox is still a dumb disk plus a pipe.

```
  mediaops-home.service
        │
        ├── mediaops-api          sqlite (api.db), Home RPCs, in-process reconcilers
        ├── mediaops-scheduler    binds Pending Pull Jobs (albums first)
        ├── mediaops-gateway      Secret watch → mTLS pool → mediaops.v1
        ├── mediaops-inventory    List/HoldList → RemoteFile / Hold
        └── mediaops-pull         Range pull, sidecar, BLAKE3, PathSchema install

  mediaops CLI      ──UDS──►  mediaops-api
  mediaops-tui      ──UDS──►  mediaops-api
  inventory/pull         ──UDS──►  mediaops-gateway  ──mTLS──►  mediaopsd --role seedbox
```

The CLI never dials `seedbox_address` for media traffic; it goes through the gateway. SSH is exec-only maintenance for seedbox bootstrap, upgrade, and confirmed edge repair, never bulk copy. There is no rsync-ssh fallback, no FTP, no rclone pipe.

## Roles

**`mediaops`** — Home API client. `get` / `apply` / `delete` / `watch` / `reconcile` / `import-legacy`. Pretty verbs (`status`, `why`, `hold`) still exist; declarative copying runs in the worker. The low-level manual `pull` remains an operator maintenance command: it pauses scheduling, refuses active bound Jobs, and records verified installation proof through the API.

**`mediaops-home`** — supervisor. Execs the five role binaries (next to `argv[0]`, then `PATH`), restarts a dead child, forwards SIGTERM. Links neither store nor transfer.

**`mediaops-api`** — the only process that opens `api.db`. Serves `mediaops.home.v1` on `$XDG_RUNTIME_DIR/mediaops-api.sock`, with a state-directory fallback when no absolute runtime directory is set. Want / Hold / Title-drift / Job-create run as tokio tasks here.

**`mediaops-scheduler`** — binds Pending Pulls to the `pull` Node. Leaves budget-blocked work Pending. Music first. A per-Job admission refusal or conflict does not block later candidates; transport failures remain errors. Revoked Want/Hold authorization refuses unbound work, while a temporarily absent source can remain Pending until it returns.

**`mediaops-gateway`** — watches Secret, owns the mTLS pool, proxies Transfer/Control on the existing home UDS (`mediaopsd.sock`).

**`mediaops-inventory`** — List/HoldList through the gateway. Writes RemoteFile and Hold. Heartbeats; `NotReady` after 30s silence. A failed list is not “box empty.” After a committed listing, inventory also runs best-effort Servarr unmonitor for proved movie/album Titles (ControlPort `wanted_missing` / `unmonitor`). It does not write Jobs.

**`mediaops-pull`** — bound Pull Jobs only: Range pull, sidecar resume, BLAKE3, PathSchema install. Phases `pulling → verifying → installed`. Recheck `statfs` before write.

**`mediaopsd --role seedbox`** — exposes the WAN mTLS listener and is the only role that performs grabber HTTP on localhost. The home gateway connects to it using `mediaops.v1` Transfer/Control.

## Home API

Package `mediaops.home.v1` (`proto/mediaops/home/v1/home.proto`). The API socket and database are private to the Unix account. `x-mediaops-actor` enforces cooperating roles' write rules; it is not authentication against malicious code already running as that same account. WAN access still requires mTLS through the gateway.

Apply replaces spec. Status is a subresource (`Patch`). Creation requires resourceVersion zero; updates require the exact current version. Bind and worker status writes validate the stored Job lifecycle. `-o json` is the raw object (no `{ok,data,error}` envelope).

Admission: CLI/import write Cluster, Secret, Want, Title.spec, Hold decision. Only inventory writes RemoteFile. Only controllers create Jobs. Only the scheduler sets `Job.spec.nodeName`. The bound worker advances its Job status; controllers can refuse revoked unbound work. Title observations require a verifying Job, or a maintenance import whose file digests are checked by the API.

Watches start with a consistent snapshot and replay a bounded durable event history. A cursor older than retained history fails explicitly; relist and watch from zero. Consumers must not treat a disconnected or expired watch as current state.

For the Home control plane, `config.toml` is import/export. Runtime settings are
the Cluster object; gateway credentials and endpoint data live in Secret. The
seedbox daemon and explicit `seedbox apply` / edge-maintenance commands still use
`config.toml`. Applying a Cluster is not a deployment of seedbox configuration.

## Pull

One-way. Remote → `_incoming/…/*.partial` → per-range BLAKE3 in the sidecar → whole-file BLAKE3 → atomic install onto a schema path.

- A range is at most 64 MiB (`range_len` on the snapshotted Job).
- Worker retries ranges in-process. Job goes `Failed` after 3 attempts or 30 minutes (`PULL_DEADLINE_SECS`). That deadline applies to transfer and to install *before* dest publication. Once dest bytes match the saved `verified_b3`, proof and staging cleanup still run even if the deadline has elapsed. API failures after publication leave the Job `Verifying`; retry is retryable recovery, not a new copy.
- Kill at 90% and run again: completed ranges stay; resume reads the sidecar's `range_len`.
- Want + a completed inventory listing → controller creates a snapshotted Pull Job for each missing file. Identity is TitleId plus the schema file key. No silent replacement of an installed or drifted episode, track, or movie.
- Title status retains both installation and current digests per file. The verified digest is persisted before installation, so a restart can recover an interrupted install and finish recording its proof.
- Once the destination matches that saved digest, recovery removes only the owned completed staging source and sidecar before recording Title proof and Installed. Destination-verification or cleanup I/O errors leave the Job Verifying for retry; a proven mismatch refuses without overwriting either file.

Range proofs verify the bytes received and retained locally; the current Range protocol does not provide an immutable remote-file snapshot. Sources must remain unchanged during a copy. With `grabber = "none"`, finish writing outside the allowlisted tree and move the completed file into place, rather than writing directly to a visible media filename.

## Library of record

Local FS wins. `grabber = "none"` is a first-class path: schema folder on the box, disk at home, no *arr HTTP. A Want is still required.

Path grammar and `TitleId` are pure functions in `crates/core`. That crate is the only renderer of library paths. See [library layout](config.md#library-layout).

## Crate graph

Cargo workspace. Edges are allowlisted and tested in `crates/arch-tests` (`make test-arch`). Adding a workspace dependency means adding the edge there first.

| Crate | Role |
| ----- | ---- |
| `bins/mediaops` | CLI client |
| `bins/mediaops-tui` | Additive Home TUI. Depends on core + home-client only |
| `bins/mediaops-home` | supervisor |
| `bins/mediaops-api` | apiserver binary |
| `bins/mediaops-scheduler` | bind |
| `bins/mediaops-gateway` | mTLS attach |
| `bins/mediaops-inventory` | box listing |
| `bins/mediaops-pull` | Range pull worker |
| `bins/mediaopsd` | seedbox daemon |
| `crates/core` | `TitleId`, PathSchema, Home objects, `config.toml`. No tokio, no tonic, no rusqlite. Only `walker` and `install` touch the filesystem |
| `crates/proto` | gRPC stubs for `mediaops.v1` and `mediaops.home.v1`; the only wire↔domain conversions |
| `crates/store` | sqlite. `state.db` (legacy) and `ApiStore` (`api.db`). The only crate that may depend on `rusqlite`. Only `mediaops-api` opens `api.db` |
| `crates/home-client` | typed Home API client |
| `crates/api` | serve, admission, watch bus, reconcilers |
| `crates/net` | mTLS, channel pool, seedbox + gateway serve |
| `crates/ssh` | Exec-only bootstrap, upgrade, and edge maintenance. No bulk media copy |
| `crates/transfer` | Range pull, `.partial` resume, BLAKE3 |
| `crates/sync` | leftover planner helpers + unit text |
| `crates/encode` | EncodePolicy. Not in this slice’s workers |
| `crates/arr` | Grabber HTTP. Linked only into `mediaopsd` |
| `crates/arch-tests` | Dependency-graph and I/O-boundary law |

Banned as direct deps: `rsync`, `rclone`, `ftp`, `ssh2`, `russh`, `ffmpeg-next`, `native-tls`. `mediaopsd`, `mediaops-home`, `mediaops-gateway`, `mediaops-scheduler`, `mediaops-inventory`, and `mediaops-pull` must not reach `store` or `encode`.

## Bind, not flock

Unattended copy concurrency is Job bind + Job status, not the legacy
`mediaops.lock`. Explicit CLI maintenance, including manual pull, still takes its
flock and coordinates scheduling through Cluster maintenance state where needed.
Lock conflict is exit 3. Role liveness uses heartbeats and becomes NotReady after
30s of silence; inventory readiness also requires a completed fresh listing.

One always-on `mediaops-home.service` replaces `mediaopsd-home.service` and `mediaops-run.timer`.

## Documentation boundary

This page describes the current system. The old two-binary, plan/run design and
its generated BMAD work queues are [historical](documentation-status.md). Their
presence in the repository does not make those commands or queues active again.

## What this is not

- A Jellyfin/Plex installer or plugin
- Two-way sync, a third-cloud archive, or a push path
- A public WebUI or a LAN bind of *arr
- Encode on the seedbox
- Auto-approve holds, auto-upgrade HD → UHD
- An in-process LLM
- mTLS-to-API (UDS only in this slice)
