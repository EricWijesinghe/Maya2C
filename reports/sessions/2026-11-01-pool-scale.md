# Session 2026-11-01: pool scale (B13, branch perf/pool-million-miners)

Goal: find what limits `bins/pool-service` as concurrent miner connections grow, measure it on this PC, fix the measured top bottlenecks, and re-measure.

Scope: `crates/stratum-v2` holds framing and messages. It has no connection, lock or queue code, so every finding is in `bins/pool-service`.

## 1. Analysis by reading the code (file:line at origin/master b5364dc3)

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

### Hardware and limits
- CPU: Intel Core Ultra 9 275HX, 24 logical cores. RAM 31.4 GiB, about 10 GiB free during the run. OS Windows 11 Pro 10.0.29680.
- Build: `cargo build -p maya-pool-service --profile perf` (`lto = false`, `codegen-units = 16`, opt-level 3, debuginfo). Not the `release` profile (fat LTO); absolute numbers are for `perf`. CARGO_BUILD_JOBS=1, the worktree's own `target/`. Dependency build took 46 min 54 s.
- Ephemeral ports: 1024-65535 (64,511), per `netsh int ipv4 show dynamicport tcp`. Each simulated miner uses one client port on 127.0.0.1, so the ceiling is about 64k. Not reached.
- Windows client listen backlog: refusals with `os error 10061` at 10,000 unpaced connects, both before and after the change. The cause (kernel backlog on a client SKU) is not isolated. Pacing the connects removed the refusals in the after run (paced baseline: 273 refused; paced after: 0). Run-to-run variance is large, so this is not a fixed claim.
- Server memory: working set of the child process (`GetProcessMemoryInfo`), so the miners' sockets and buffers are not included.
- Miners: real PQ handshake, real SV2 messages, shares solved before the timed phase (solve time reported as setup). Valid shares are real solutions at the pinned channel target (3 bits, vardiff pinned). Invalid shares are real nonces that miss the target. Each miner's last share is always invalid, so every accepted share is acknowledged by the time the run ends.

### Single-thread share cost (`pool-load hash-cost`)
```
== pool-load hash-cost ==
share verification (pow_hash_at, DagConfig::MAINNET, height 30000): 5591.1 us per hash, single thread, 2000 samples after warm-up
single-thread shares/s: 179
```
This is the per-share CPU cost. The design estimate (25.4 ms, Argon2id) is for heights below the DAG fork. The mainnet pool verifies DAG shares at 5.6 ms.

### Saturation (1000 miners, 5 offered shares/s each, 10 s, job push at end)

Before (`validators = 8`, the fixed default at origin/master):
```
accepted: 19000  refused: 1000 (of which valid refused: 0)
accepted shares/s over the send window (11.0 s): 1727.3
refused-share ack latency (immediate): p50 8816316 us  p99 9030996 us  n=1000
accepted-share ack latency (batched, ACK_BATCH=8): p50 7329715 us  p99 9547364 us  n=19000
```
After (`validators = 24`, one per core):
```
accepted: 47000  refused: 3000 (of which valid refused: 0)
accepted shares/s over the send window (11.0 s): 4272.7
refused-share ack latency (immediate): p50 1369067 us  p99 1874338 us  n=3000
accepted-share ack latency (batched, ACK_BATCH=8): p50 1744247 us  p99 3185621 us  n=47000
```
Offered 5,000/s; the 8-worker pool fell behind (queue backlog) and the 24-worker pool nearly kept up. Note: at 8 workers the pool verified 1,727 shares/s against a single-thread 179/s, so the 8-worker pool used about 10 cores' worth; the 24-worker pool used about 24. Memory bandwidth on the 64 MiB cache is a plausible cause of the gap, but it is not isolated.

### Connections at production share rate (1 share/min per miner, 2 shares each, 60 s)

Before (origin/master, paced connects, 16 per 20 ms):
```
miners requested: 10000  channels open: 9727  failed: 273
accepted: 9727  refused: 9727
server RSS (working set): baseline 75.0 MiB, idle with 9727 channels 184.0 MiB, peak under load 189.1 MiB
server memory per open channel (idle delta / channels): 11.5 KiB
job fan-out (push to NewMiningJob received): p50 70.1 ms  p99 164.1 ms  max 164.9 ms  miners 9727
```
After (paced connects, same harness):
```
miners requested: 10000  channels open: 10000  failed: 0
accepted: 10000  refused: 10000
server RSS (working set): baseline 75.8 MiB, idle with 10000 channels 190.1 MiB, peak under load 195.6 MiB
server memory per open channel (idle delta / channels): 11.7 KiB
job fan-out (push to NewMiningJob received): p50 105.6 ms  p99 222.5 ms  max 223.0 ms  miners 10000
```
Fan-out rose with N in both binaries (p99 9.6 ms at 1,000 miners, 61 ms at 5,000, 164-223 ms at 10,000). The after run's p99 is higher than the before run's, and that difference is within the run-to-run noise seen across repeats, so the fan-out change is not claimed as an improvement.

Unpaced 10,000 connects on the after binary: 3,623 refused, same kind of refusal as before (`os error 10061`). Refused connects are a client-pacing limit on this PC, not a pool-capacity number.

The per-channel memory (about 12 KiB) includes the per-connection tasks, the PQ stream buffers, the 64-slot outbound queue, and the channel state. It is measured, not split into parts.

### Ack latency at production rate (1,000 miners, 1 share/min)
```
accepted-share ack latency (batched, ACK_BATCH=8): p50 60004666 us  p99 60018550 us  n=1000
```
Accepted shares wait for a batch of eight, or for the next refusal. The harness's last share per miner is a refusal, so the flush happens at the end of the window, and the 60 s figure is the send-window length. At 1 share/min per miner, a real miner waits about 8 minutes for its initial acknowledgement. This is what the design intends for bulk acks, and it is a product problem (miners cannot see progress). Not fixed here.

### Projection (labeled as projection, not measurement)
- Measured on this PC: 4,273 verified shares/s with 24 workers. 
- At one share per minute per miner: 4,273 x 60 = about 256,000 miners per 24-core PC, if throughput stays linear to 24 cores. Linear scaling was not measured beyond 24 workers, and the 8-to-24 step was already sublinear in per-core terms.
- One million miners at one share per minute need 16,667 verifications per second, which is about 94 cores at 5.59 ms each, or about four 24-core machines. This is a projection from the single-thread cost.

## 3. Fixes and what was tested

| Bottleneck | Change | Test | Result |
|---|---|---|---|
| Validator workers fixed at 8 on a 24-core machine | `pool.rs` default = `available_parallelism` | measured above | 1,727 -> 4,273 shares/s at 5/s offered |
| Connection leak on a failed share or session error (`daemon.rs` `?` paths) | early returns replaced by the cleanup path | not covered by an automated test (NOT VERIFIED: the error path needs a failing ledger or DAG, and no test double exists for that in this crate) | checked by reading the code |
| Silent socket held forever | handshake timeout 10 s | `tests/transport_limits.rs::a_silent_socket_is_closed_after_the_handshake_timeout` (passes, 10.0 s) | closed |
| Idle channel held forever | 5-minute idle timeout | not tested (timing path is the same as the handshake deadline) | NOT VERIFIED |
| No connection cap | `MAX_CONNECTIONS = 50,000`, excess refused at accept | not tested | NOT VERIFIED |
| Listen backlog | `TcpSocket::listen(4096)` | paced and unpaced runs above | not a demonstrated fix |

Tests run: `cargo test -p maya-pool-service --profile perf --lib`: `test result: ok. 160 passed; 0 failed`. `--test worker_simulation --test ledger_tests`: `23 passed` and `5 passed; 1 ignored`. `--test transport_limits`: `1 passed`, finished in 10.01 s.

## 4. Bottlenecks left

1. **Verification CPU** is the ceiling (5.6 ms per share per core). A GPU or batched verifier is the lever; not attempted.
2. **Batched acks** hide progress for up to 8 share intervals. Needs a time-based flush.
3. **Global locks.** `state.channels` and `state.telemetry` are single mutexes taken by every share and by `job_loop` (O(channels) under the lock per job, `daemon.rs:457-461`). Not measured to be the bottleneck at 10k channels, but the cost grows with N.
4. **Per-IP limit** is not implemented. The total cap is.
5. **Ledger:** two RocksDB writes per share (`ledger/rocks.rs:135-156`, `236-255`) and no single-batch write. Not measured; the validator hash dominates at 5.6 ms.
6. **PPLNS window** at a million miners is about 500,000 JSON records per block (estimate; not measured).
7. **Accept refusal on client-SKU Windows** at unpaced connect bursts: not isolated.

## 5. Commands

- Smoke: `pool-load run --miners 20 --per-miner-rate 1.0 --duration 10 --job-push`
- Production rate: `pool-load run --miners 10000 --per-miner-rate 0.016667 --duration 60 --job-push --connect-batch 16`
- Saturation: `pool-load run --miners 1000 --per-miner-rate 5.0 --duration 10 --validators {8|24} --job-push`
- Cost: `pool-load hash-cost`
