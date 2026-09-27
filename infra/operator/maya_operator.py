#!/usr/bin/env python3
"""The Maya2C Kubernetes operator (Master Prompt 19 §1).

One custom resource, `MayaNetwork` (crd.yaml), and three jobs:

- **install** — a headless Service and a StatefulSet running the node binary
  with the resource's image and genesis. Replica 0 serves snapshots; every
  other replica is started with `--bootstrap-from` replica 0, which the node
  ignores while its volume already holds a chain and uses when it is empty.
- **snapshot restore** — `spec.restore: {replica, generation}` deletes that
  replica's volume and pod; the StatefulSet recreates both, and the empty
  volume bootstraps from replica 0's snapshot. The operator then waits until
  the restored replica's height is within `healthGateBlocks` of replica 0.
- **rolling upgrade** — a changed `spec.image` is rolled one replica at a
  time from the highest ordinal down, using the StatefulSet's partition. A
  replica must be Ready **and its chain height must advance** before the next
  one is touched; a replica that does not, within `healthTimeoutSeconds`,
  stops the rollout and leaves the rest on the old image.

Run out of cluster against the current kube context:

    kopf run --standalone infra/operator/maya_operator.py

Health is read over each pod's JSON-RPC through `kubectl exec`, so the
operator needs no network path into the pod network — and the node image
needs curl, which `infra/docker/Dockerfile.local` has.
"""

from __future__ import annotations

import json
import subprocess
import time

import kopf
import kubernetes

GROUP, VERSION, PLURAL = "maya2c.io", "v1alpha1", "mayanetworks"
HEALTH_GATE_BLOCKS = 3


def _api():
    kubernetes.config.load_kube_config()
    return kubernetes.client.AppsV1Api(), kubernetes.client.CoreV1Api()


def _args(name: str, namespace: str, spec: dict, ordinal_expr: bool = True) -> list[str]:
    """Node arguments. Replica-specific flags are chosen in the shell wrapper,
    because a StatefulSet has one template for every replica."""
    return [
        "--genesis", "/config/genesis.json", "--data-dir", "/data",
        "--rpc-addr", "0.0.0.0:8545", "--p2p-port", "30333",
        "--snapshot-interval", str(spec.get("snapshotInterval", 10)),
    ]


def _statefulset(name: str, namespace: str, spec: dict) -> dict:
    svc = f"{name}-p2p"
    first = f"{name}-0.{svc}.{namespace}.svc.cluster.local"
    miner = spec.get("minerOrdinal")
    base = " ".join(_args(name, namespace, spec))
    # The ordinal is the pod name's suffix. Replica 0 is the bootnode and
    # snapshot source; the miner (proof-of-work devnets) mines; the rest dial
    # replica 0 and bootstrap from it when their volume is empty.
    script = f"""set -eu
ord="${{POD_NAME##*-}}"
extra=""
if [ "$ord" != "0" ]; then
  extra="--bootnode /dns4/{first}/tcp/30333 --bootstrap-from http://{first}:8545 --prune-depth {spec.get('pruneDepth', 10)}"
fi
if [ "{'' if miner is None else miner}" = "$ord" ]; then extra="$extra --mine --threads 1"; fi
exec /usr/local/bin/maya2c-node {base} $extra
"""
    return {
        "apiVersion": "apps/v1",
        "kind": "StatefulSet",
        "metadata": {"name": name, "namespace": namespace, "labels": {"maya2c.io/network": name}},
        "spec": {
            "serviceName": svc,
            "replicas": spec["replicas"],
            "podManagementPolicy": "Parallel",
            "selector": {"matchLabels": {"maya2c.io/network": name}},
            "updateStrategy": {"type": "RollingUpdate", "rollingUpdate": {"partition": 0}},
            "template": {
                "metadata": {"labels": {"maya2c.io/network": name}},
                "spec": {
                    "securityContext": {"runAsNonRoot": True, "runAsUser": 10001, "fsGroup": 10001},
                    "containers": [{
                        "name": "node",
                        "image": spec["image"],
                        "imagePullPolicy": "IfNotPresent",
                        "command": ["/bin/sh", "-c", script],
                        "env": [{"name": "POD_NAME", "valueFrom": {"fieldRef": {"fieldPath": "metadata.name"}}}],
                        "ports": [{"name": "p2p", "containerPort": 30333}, {"name": "rpc", "containerPort": 8545}],
                        "readinessProbe": {"tcpSocket": {"port": 8545}, "periodSeconds": 3},
                        "volumeMounts": [
                            {"name": "data", "mountPath": "/data"},
                            {"name": "genesis", "mountPath": "/config", "readOnly": True},
                        ],
                        "resources": {"requests": {"cpu": "50m", "memory": "96Mi"}, "limits": {"memory": "512Mi"}},
                    }],
                    "volumes": [{"name": "genesis", "configMap": {"name": spec["genesisConfigMap"]}}],
                },
            },
            "volumeClaimTemplates": [{
                "metadata": {"name": "data"},
                "spec": {"accessModes": ["ReadWriteOnce"], "resources": {"requests": {"storage": "1Gi"}}},
            }],
        },
    }


def _service(name: str, namespace: str) -> dict:
    return {
        "apiVersion": "v1",
        "kind": "Service",
        "metadata": {"name": f"{name}-p2p", "namespace": namespace},
        "spec": {
            "clusterIP": "None",
            "publishNotReadyAddresses": True,
            "selector": {"maya2c.io/network": name},
            "ports": [{"name": "p2p", "port": 30333}, {"name": "rpc", "port": 8545}],
        },
    }


def height(namespace: str, pod: str) -> int | None:
    """The pod's chain height over its own JSON-RPC, or None if unreachable."""
    req = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "get_tip_height", "params": []})
    try:
        out = subprocess.run(
            ["kubectl", "-n", namespace, "exec", pod, "--", "curl", "-sf", "-m", "3",
             "-H", "content-type: application/json", "-d", req, "http://127.0.0.1:8545"],
            capture_output=True, text=True, timeout=10,
        ).stdout
        return int(json.loads(out)["result"])
    except Exception:  # noqa: BLE001 - a pod that is not up answers nothing
        return None


def wait_healthy(namespace: str, pod: str, timeout: int, reference: str | None, logger) -> None:
    """Ready, answering, advancing, and (with a reference) within the gate."""
    deadline = time.time() + timeout
    first = None
    while time.time() < deadline:
        h = height(namespace, pod)
        if h is not None:
            first = h if first is None else first
            ref = height(namespace, reference) if reference else None
            close = ref is None or h + HEALTH_GATE_BLOCKS >= ref
            if h > first and close:
                logger.info(f"{pod}: healthy at height {h} (reference {ref})")
                return
        time.sleep(3)
    raise kopf.PermanentError(f"{pod} not healthy within {timeout}s (height {height(namespace, pod)})")


@kopf.on.create(GROUP, VERSION, PLURAL)
def install(spec, name, namespace, patch, logger, **_):
    apps, core = _api()
    core.create_namespaced_service(namespace, _service(name, namespace))
    apps.create_namespaced_stateful_set(namespace, _statefulset(name, namespace, spec))
    patch.status["phase"] = "Installed"
    patch.status["image"] = spec["image"]
    logger.info(f"installed {spec['replicas']} replicas of {spec['image']}")


@kopf.on.field(GROUP, VERSION, PLURAL, field="spec.image")
def upgrade(old, new, spec, name, namespace, patch, logger, **_):
    if old is None or old == new:
        return
    apps, core = _api()
    replicas = spec["replicas"]
    timeout = spec.get("healthTimeoutSeconds", 300)
    # Freeze every replica, change the template, then release one at a time.
    apps.patch_namespaced_stateful_set(name, namespace, {"spec": {"updateStrategy": {
        "type": "RollingUpdate", "rollingUpdate": {"partition": replicas}}}})
    sts = _statefulset(name, namespace, spec)
    apps.patch_namespaced_stateful_set(name, namespace, {"spec": {"template": sts["spec"]["template"]}})
    log = []
    for ordinal in range(replicas - 1, -1, -1):
        started = time.time()
        apps.patch_namespaced_stateful_set(name, namespace, {"spec": {"updateStrategy": {
            "rollingUpdate": {"partition": ordinal}}}})
        pod = f"{name}-{ordinal}"
        # Wait for the old pod to go before judging the new one.
        for _ in range(60):
            p = core.read_namespaced_pod(pod, namespace)
            if p.spec.containers[0].image == new:
                break
            time.sleep(2)
        reference = f"{name}-0" if ordinal != 0 else f"{name}-1" if replicas > 1 else None
        wait_healthy(namespace, pod, timeout, reference, logger)
        log.append({"replica": ordinal, "seconds": round(time.time() - started, 1)})
        logger.info(f"upgraded {pod} to {new}")
    patch.status["phase"] = "Upgraded"
    patch.status["image"] = new
    patch.status["lastUpgrade"] = log


@kopf.on.field(GROUP, VERSION, PLURAL, field="spec.restore")
def restore(new, spec, name, namespace, patch, logger, **_):
    if not new or "replica" not in new:
        return
    apps, core = _api()
    ordinal = int(new["replica"])
    pod, pvc = f"{name}-{ordinal}", f"data-{name}-{ordinal}"
    started = time.time()
    # PVC first (it waits on the pod's finalizer), then the pod: the
    # StatefulSet recreates both, and the empty volume bootstraps.
    core.delete_namespaced_persistent_volume_claim(pvc, namespace)
    core.delete_namespaced_pod(pod, namespace)
    for _ in range(60):
        try:
            if core.read_namespaced_pod(pod, namespace).metadata.deletion_timestamp is None:
                break
        except kubernetes.client.exceptions.ApiException:
            pass
        time.sleep(2)
    wait_healthy(namespace, pod, spec.get("healthTimeoutSeconds", 300), f"{name}-0", logger)
    patch.status["phase"] = "Restored"
    patch.status["lastRestore"] = {
        "replica": ordinal,
        "generation": new.get("generation"),
        "seconds": round(time.time() - started, 1),
    }
    logger.info(f"restored {pod} from replica 0's snapshot")
