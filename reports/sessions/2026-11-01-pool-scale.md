# Session 2026-11-01: pool scale (B13, branch perf/pool-million-miners)

Goal: find what limits `bins/pool-service` as concurrent miner connections grow, measure it on this PC, fix the measured top bottlenecks, and re-measure.

Scope: `crates/stratum-v2` is framing and messages only. It has no connection, lock or queue code, so every finding is in `bins/pool-service`.

## 1. Read-only analysis (file:line at origin/master b5364dc3)

### Connection model
- One accept loop, one Tokio task per TCP connection: `bins/pool-service/src/daemon.rs:116-141`. No admission control.
- Per connection: a write task (`daemon.rs:170`), a job-push task (`daemon.rs:192`), and the read loop in `serve_connection` (`daemon.rs:225`). One `mpsc` of capacity 64 (`OUTBOUND_CAPACITY`, `daemon.rs:60`). Each connection also owns a `Session`, an `AckBook`, and a `Vec<u32>` of channels.
- Shares are verified inline on the read loop: `validator.submit(request).await` (`daemon.rs:250`). One share in flight per connection is the intended backpressure (`daemon.rs:26-34`).
- Shared state is three std locks taken per share: `channels: Mutex<ChannelRegistry>` (`state.rs:108`), `telemetry: Mutex<TelemetryBook>` (`state.rs:110`), `jobs: RwLock<JobRegistry>` (`state.rs:106`). Every share takes the channel lock at least twice (`session.rs:330-344`, `validator.rs:273`) and the telemetry lock once (`validator.rs:287`). One global channel lock serves every connection.

### Share validation cost per share
- PoW: `share::verify` calls `BlockHeader::pow_hash_at` (`share.rs:88`, `crates/node/src/core/block.rs:202`). Below `DAG_ACTIVATION_HEIGHT` (30,000) that is ArgonBlake (Argon2id, 32 MiB). At or above it, hashimoto_light over a 64 MiB epoch cache. `docs/stratum-v2.md` §4 cites 25.4 ms per Argon2id share. That is the design's estimate; section 2 is the measured figure.
- Concurrency: `DEFAULT_WORKERS = 8` (`validator.rs:68`), a semaphore over `spawn_blocking` (`validator.rs:145-190`). The hash cost sets the ceiling: `workers / cost_per_share` shares per second per machine.
- Per share, besides the hash: `job.clone()` under the jobs read lock (`validator.rs:234`, a header copy), `ledger.append_share` (`validator.rs:263`, see ledger), two channel-lock sections (`validator.rs:273`, `validator.rs:338`), one telemetry-lock section (`validator.rs:287`), and a `PoolEvent::Share` with two allocations (`validator.rs:300-304`).
- Cheap checks run before any hash (`channel.rs:235-251`), and a repeated nonce is refused before hashing (`channel.rs:258`, `session.rs:340-350`).

### Job broadcast fan-out
- `job_loop` builds one `Arc<Job>` per template (`daemon.rs:464`) and sends it on a `broadcast` channel of capacity 16 (`bins/pool-service/src/bin/pool.rs:60`). One allocation per job.
- Per connection, the push task copies the channel-id list (`daemon.rs:202`) and calls `job_messages(channel_id, &job)` per channel (`daemon.rs:206-209`, `session.rs:489-507`). That builds two `Message`s per channel, and each connection encodes its own frames. Work per job is O(channels) encodes with no sharing, because `channel_id` appears in both messages.
- `job_loop` takes the global channel lock and calls `push_job` on every channel (`daemon.rs:457-461`). That is O(channels) under the lock every share needs.
- `maintenance_loop` takes the channel and telemetry locks every 5 s (`state.rs:211-223`, `daemon.rs:575`). `total_hashrate` walks every worker under the telemetry lock.

### Memory per connection
- Outbound queue: up to 64 `Message`s (`daemon.rs:60`). Each frame read allocates its payload (`daemon.rs:330-337`), then copies header and payload into a second `Vec` before decoding (`daemon.rs:343-345`).
- Per channel: `ChannelState` with `HashMap<u32, HashSet<u64>>` of submitted nonces, released when a job retires (`channel.rs:149`, `channel.rs:207-217`). Bounded by shares per job.
- Per worker: `TelemetryBook` `HashMap<WorkerKey, WorkerEntry>` (`telemetry.rs:124`), evicted after 10 x `WORKER_RETENTION` (`telemetry.rs:407-420`). Bounded by distinct workers, not live connections.
- Measured per-connection cost: section 2.

### Backpressure and slow clients
- A slow reader fills the 64-slot outbound queue. Then `outbound.send(..).await` blocks the read loop (`daemon.rs:263-300`), so the pool stops reading that miner. No timeout, no drop. Memory stays bounded at 64 messages, but the task and channel live until the peer closes (see DoS).
- Job pushes: a slow connection's push task blocks on `push_outbound.send` (`daemon.rs:213`). The broadcast receiver lags, and `Lagged` is skipped (`daemon.rs:198`). Right policy for jobs.
- Validator queue: 4,096 submissions (`validator.rs:49`). A full queue waits rather than rejects (`validator.rs:198-210`). Correct for accounting; a share flood slows every connection.

### Limits and DoS
- **No connection limit.** `accept` spawns unconditionally (`daemon.rs:118-134`). No semaphore, no per-IP cap. `docs/stratum-v2.md` sizes for 50,000 connections; nothing enforces it.
- **No handshake timeout.** `handshake::respond` is awaited with no deadline (`daemon.rs:158`). A TCP peer that connects and sends nothing holds a task and a socket forever.
- **No idle timeout.** `read_frame` has no deadline (`daemon.rs:316`). A connected miner that never sends keeps its channel and nonce range forever.
- **Resource leak on error (bug).** `serve_connection` returns early with `?` at `daemon.rs:233` (`session.handle`) and `daemon.rs:250` (`validator.submit`). Both skip `session.disconnect` (`daemon.rs:308`) and skip aborting `push_task`. Effects: the channel stays registered and its nonce range is never released; the job-push task holds a clone of `outbound` and a broadcast receiver, so the write task never ends. Reachable: `validator.submit` returns `Err` on a ledger write failure (`validator.rs:263`), on a DAG failure (`share.rs:88`), or after validator shutdown (`validator.rs:204`); `session.handle` returns `Err` on a poisoned lock (`session.rs:204`, `session.rs:217`). Each occurrence leaks two tasks and one channel.
- **Pressure is O(channels) under the lock.** Vardiff pressure raises every channel's target (`validator.rs:367-391`), every 30 s while the queue is above a quarter full. It holds the global channel lock for the whole loop (`validator.rs:372`).
- **Frame size** is bounded before allocation (`daemon.rs:329-336`). Good.

### Ledger write path
- `RocksLedger::open` uses default options (`ledger/rocks.rs:122-130`). Default WAL, default write options, no batching on the share path.
- `append_share` does two RocksDB writes per share: `bump(NEXT_SEQUENCE)` a `get` plus a `put` (`rocks.rs:135-156`), then the share record `put` (`rocks.rs:236-255`). The counter mutex (`rocks.rs:84`) serialises all validators on it. The two writes are not atomic: a crash between them leaves the counter advanced with no record (a sequence hole, harmless to PPLNS but not clean).
- Records are JSON (`rocks.rs:107-114`): one serialization per share.
- No group commit. Each share is durable before the ack path returns, which is the right property, at one WAL append per share.

### PPLNS window
- `window_size(network_target, factor)` converts an `f64` factor to `U256` (`pplns.rs:65-80`). Operator input, off-chain, deterministic on one machine. The split itself is exact `U256` (`pplns.rs:98`).
- The walk reads backwards from the found share and decodes each record until the window weight is reached, capped at `MAX_WINDOW_SHARES = 5,000,000` (`ledger/mod.rs:55`, `rocks.rs:257-277`).
- Estimate (derived, not measured): `pplns_factor = 2`, 15 s blocks, and N miners at one share per 60 s give a window of about N/2 shares. That is 500,000 JSON decodes per block at a million miners, on the payout path.

### Design-level findings
- **Verification, not connections, is the ceiling.** A million miners at one share per 60 s need 16,667 shares/s. At the documented 25.4 ms per share over 8 workers the ceiling is about 315 shares/s, or about 19,000 miners. Section 2 replaces the estimate with a measurement.
- **Batched acks.** `ACK_BATCH = 8` (`daemon.rs:68`) acknowledges accepted shares after 8 per channel, or on the next refusal. At one share per minute an accepted share waits about 8 minutes for its acknowledgement. The comment says "within a few minutes". Section 2 measures it.

## 2. Measurements

(filled in below after the baseline run)
