// three.js renderer for the propagation scene. Loaded with a dynamic import, so
// phones and reduced-motion visitors never download three. Every frame is drawn
// from the pure model in propagation-model.ts; this file only places pixels.
import * as THREE from "three";
import {
  buildModel, beamAt, edgeLevel, finalityAt, nodeLevel, ringAt, yawAt, PERIOD, phaseAt,
} from "./propagation-model.ts";

const DPR_CAP = 1.5;
const FRAME_MS = 1000 / 30;
const CLOCK_STEP = 0.1; // a backgrounded tab advances the clock at most this much per frame
const COL = {
  dim: new THREE.Color("#1d3c44"),
  peer: new THREE.Color("#7eeaf0"),
  mlDsa: new THREE.Color("#2fd9e6"),
  slhDsa: new THREE.Color("#e6b260"),
  core: new THREE.Color("#fff3d6"),
};

function glowTexture(): THREE.CanvasTexture {
  const c = document.createElement("canvas");
  c.width = c.height = 128;
  const g = c.getContext("2d");
  if (!g) throw new Error("2d context unavailable");
  const grad = g.createRadialGradient(64, 64, 0, 64, 64, 64);
  grad.addColorStop(0, "rgba(255,243,214,1)");
  grad.addColorStop(0.35, "rgba(126,234,240,0.45)");
  grad.addColorStop(1, "rgba(126,234,240,0)");
  g.fillStyle = grad;
  g.fillRect(0, 0, 128, 128);
  return new THREE.CanvasTexture(c);
}

export function startPropagation(canvas: HTMLCanvasElement, onPhase: (i: number) => void): void {
  const m = buildModel();
  const renderer = new THREE.WebGLRenderer({ canvas, antialias: true, alpha: true });
  renderer.setPixelRatio(Math.min(window.devicePixelRatio, DPR_CAP));
  const camera = new THREE.PerspectiveCamera(36, 1, 0.1, 50);
  camera.position.set(0, 0, 8.6);
  const scene = new THREE.Scene();
  const world = new THREE.Group();
  scene.add(world);

  // Peers and validators share one instanced mesh; validators are larger.
  const nodeGeo = new THREE.SphereGeometry(1, 10, 8);
  const nodeMat = new THREE.MeshBasicMaterial({ toneMapped: false });
  const nodes = new THREE.InstancedMesh(nodeGeo, nodeMat, m.points.length);
  const isValidator = new Set(m.validators);
  const base = m.points.map((_, i) => (isValidator.has(i) ? 0.07 : i === m.core ? 0.11 : 0.036));
  world.add(nodes);

  // Edges: one vertex-coloured line set, additive so overlaps glow.
  const edgeGeo = new THREE.BufferGeometry();
  const edgePos = new Float32Array(m.edges.length * 6);
  m.edges.forEach(([a, b], e) => {
    edgePos.set([...m.points[a], ...m.points[b]], e * 6);
  });
  edgeGeo.setAttribute("position", new THREE.BufferAttribute(edgePos, 3));
  edgeGeo.setAttribute("color", new THREE.BufferAttribute(new Float32Array(m.edges.length * 6), 3));
  const edges = new THREE.LineSegments(edgeGeo, new THREE.LineBasicMaterial({
    vertexColors: true, transparent: true, blending: THREE.AdditiveBlending, depthWrite: false,
  }));
  world.add(edges);

  // Signature beams: two per validator, one ML-DSA (teal) and one SLH-DSA (bronze).
  const beamCount = m.validators.length * 2;
  const beamGeo = new THREE.BufferGeometry();
  beamGeo.setAttribute("position", new THREE.BufferAttribute(new Float32Array(beamCount * 6), 3));
  beamGeo.setAttribute("color", new THREE.BufferAttribute(new Float32Array(beamCount * 6), 3));
  const beams = new THREE.LineSegments(beamGeo, new THREE.LineBasicMaterial({
    vertexColors: true, transparent: true, blending: THREE.AdditiveBlending, depthWrite: false,
  }));
  world.add(beams);

  const coreMat = new THREE.MeshBasicMaterial({ color: COL.core });
  const core = new THREE.Mesh(new THREE.SphereGeometry(1, 24, 16), coreMat);
  core.position.set(...m.points[m.core]);
  world.add(core);

  const halo = new THREE.Sprite(new THREE.SpriteMaterial({
    map: glowTexture(), transparent: true, blending: THREE.AdditiveBlending, depthWrite: false,
  }));
  halo.position.copy(core.position);
  world.add(halo);

  const ring = new THREE.Mesh(new THREE.RingGeometry(0.97, 1, 96), new THREE.MeshBasicMaterial({
    color: COL.peer, transparent: true, blending: THREE.AdditiveBlending, depthWrite: false,
  }));
  ring.position.copy(core.position);
  world.add(ring);

  const tmp = new THREE.Object3D();
  const from = new THREE.Vector3();
  const head = new THREE.Vector3();
  const tail = new THREE.Vector3();
  const cc = new THREE.Color();
  const corePos = new THREE.Vector3(...m.points[m.core]);
  let sim = 0;
  let last = performance.now();
  let lastDraw = 0;
  let raf = 0;
  let visible = true;
  let lastPhase = -1;
  let yaw = 0;

  function resize() {
    const w = canvas.clientWidth, h = canvas.clientHeight;
    if (!w || !h) return;
    renderer.setSize(w, h, false);
    camera.aspect = w / h;
    camera.updateProjectionMatrix();
  }

  function frame(now: number) {
    raf = 0;
    if (!visible || document.hidden) return;
    // 30 fps is plenty for a slow, heavy motion, and halves the GPU and CPU cost.
    if (now - lastDraw < FRAME_MS) { raf = requestAnimationFrame(frame); return; }
    lastDraw = now;
    const dt = Math.min((now - last) / 1000, CLOCK_STEP);
    last = now;
    sim += dt;
    const t = sim % PERIOD;
    const fin = finalityAt(m, t);
    const phase = phaseAt(m, t);
    if (phase !== lastPhase) { lastPhase = phase; onPhase(phase); }

    // Heavy camera: first-order ease toward the target yaw, never a snap.
    yaw += (yawAt(t) - yaw) * (1 - Math.exp(-dt * 2.2));
    world.rotation.y = yaw;
    camera.position.x = fin.shake * Math.sin(t * 61);
    camera.position.y = fin.shake * 0.6 * Math.cos(t * 47);
    camera.position.z = 8.6;
    camera.lookAt(0, 0, 0);

    m.points.forEach((p, i) => {
      const lv = nodeLevel(m, i, t);
      const locked = i === m.core || isValidator.has(i);
      const pulse = locked && t >= m.finalAt ? fin.pulse : 1;
      const s = base[i] * (1 + 0.55 * lv * (i === m.core ? 1 + fin.flash : 1));
      tmp.position.set(p[0], p[1], p[2]);
      tmp.scale.setScalar(s);
      tmp.updateMatrix();
      nodes.setMatrixAt(i, tmp.matrix);
      const target = i === m.core ? COL.core : COL.peer;
      cc.copy(COL.dim).lerp(target, lv).multiplyScalar(pulse);
      nodes.setColorAt(i, cc);
    });
    nodes.instanceMatrix.needsUpdate = true;
    if (nodes.instanceColor) nodes.instanceColor.needsUpdate = true;

    const ec = edgeGeo.getAttribute("color") as THREE.BufferAttribute;
    m.edges.forEach((_, e) => {
      const lv = edgeLevel(m, e, t);
      cc.copy(COL.dim).lerp(COL.peer, Math.min(1, lv));
      for (let v = 0; v < 2; v++) ec.setXYZ(e * 2 + v, cc.r, cc.g, cc.b);
    });
    ec.needsUpdate = true;

    const bp = beamGeo.getAttribute("position") as THREE.BufferAttribute;
    const bc = beamGeo.getAttribute("color") as THREE.BufferAttribute;
    m.validators.forEach((v, k) => {
      for (const kind of [0, 1] as const) {
        const b = beamAt(m, k, kind, t);
        const idx = k * 2 + kind;
        from.set(...m.points[v]);
        head.copy(from).lerp(corePos, b.p);
        tail.copy(from).lerp(corePos, Math.max(0, b.p - 0.22));
        bp.setXYZ(idx * 2, tail.x, tail.y, tail.z);
        bp.setXYZ(idx * 2 + 1, head.x, head.y, head.z);
        cc.copy(kind === 0 ? COL.mlDsa : COL.slhDsa).multiplyScalar(b.alpha);
        bc.setXYZ(idx * 2, 0, 0, 0);
        bc.setXYZ(idx * 2 + 1, cc.r, cc.g, cc.b);
      }
    });
    bp.needsUpdate = true;
    bc.needsUpdate = true;

    const coreLevel = nodeLevel(m, m.core, t);
    core.scale.setScalar(0.1 + 0.05 * coreLevel + 0.03 * fin.flash);
    halo.scale.setScalar(0.9 + 1.6 * fin.flash + 0.25 * nodeLevel(m, m.core, t));
    (halo.material as THREE.SpriteMaterial).opacity = 0.35 + 0.65 * fin.flash;
    const rg = ringAt(m, t);
    ring.scale.setScalar(rg.r);
    (ring.material as THREE.MeshBasicMaterial).opacity = rg.a;

    renderer.render(scene, camera);
    raf = requestAnimationFrame(frame);
  }

  function run() {
    if (!raf && visible && !document.hidden) { last = performance.now(); raf = requestAnimationFrame(frame); }
  }

  window.addEventListener("resize", resize, { passive: true });
  document.addEventListener("visibilitychange", run);
  new IntersectionObserver(([e]) => { visible = e.isIntersecting; run(); }).observe(canvas);

  resize();
  renderer.render(scene, camera);
  canvas.parentElement?.classList.add("gl");
  run();
}
