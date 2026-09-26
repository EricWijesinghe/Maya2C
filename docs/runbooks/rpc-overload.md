# Runbook: RPC overload

**Symptom.** RPC latency p99 rising, timeouts, 5xx at the gateway.

**Check.** Request mix at the gateway; one node saturates at ~21k simple reads/s on 4 vCPU (`reports/14-scale.md`).

**Act.** Add RPC nodes behind the gateway; move history queries to the indexer; rate-limit per key.

**Rehearsal.** measured: `maya2c-rpc-load` curve, `reports/14-scale.md`
