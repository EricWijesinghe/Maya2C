// The homepage hero in 3D: the validator committee around the Maya2C core,
// and every real finalized block as a crystal on a spiral chain.
//
// Data is real, choreography is illustrative: each crystal is a block the
// explorer streamed, and the validator count comes from get_bft_status. The
// pulses show the order DAG-BFT works in (proposal to the committee, votes to
// commit) but not which validator proposed or how long each hop took: that
// would need per-vote telemetry the node does not publish.
import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { EffectComposer } from "three/examples/jsm/postprocessing/EffectComposer.js";
import { RenderPass } from "three/examples/jsm/postprocessing/RenderPass.js";
import { UnrealBloomPass } from "three/examples/jsm/postprocessing/UnrealBloomPass.js";
import { OutputPass } from "three/examples/jsm/postprocessing/OutputPass.js";

export type Pick =
  | { kind: "chain"; x: number; y: number }
  | { kind: "block"; height: number; txs: number; x: number; y: number }
  | { kind: "validator"; index: number; of: number; x: number; y: number };

export interface Hero3D {
  addBlock(height: number, txs: number, animate: boolean): void;
  setValidators(count: number): void;
}

const TEAL = new THREE.Color(0x3fd2e0);
const GOLD = new THREE.Color(0xe0b46a);
const ICE = new THREE.Color(0xbff6fa);
const MAX_BLOCKS = 48;
const RING = 2.5;
const PHONE = 760;

type Pulse = { mesh: THREE.Mesh; from: THREE.Vector3; to: THREE.Vector3; t0: number; dur: number; done?: () => void };
type Block = { mesh: THREE.Mesh; height: number; txs: number; target: THREE.Vector3; born: number };

/** Where the k-th newest block sits: a slow galaxy arm around the core. */
function spiral(k: number, out: THREE.Vector3): THREE.Vector3 {
  const a = 0.9 + k * 0.32;
  const r = 3.1 + k * 0.03;
  return out.set(Math.cos(a) * r, -0.35 - k * 0.045 + Math.sin(k * 0.5) * 0.08, Math.sin(a) * r);
}

export function mountHero3D(
  canvas: HTMLCanvasElement,
  stage: HTMLElement,
  onPick: (p: Pick | null) => void,
  emblem?: HTMLElement,
): Hero3D {
  const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const renderer = new THREE.WebGLRenderer({ canvas, antialias: true, powerPreference: "high-performance" });
  const scene = new THREE.Scene();
  const camera = new THREE.PerspectiveCamera(42, 1, 0.1, 200);
  camera.position.set(0, 2.2, 10.5);

  const controls = new OrbitControls(camera, canvas);
  Object.assign(controls, { enableZoom: false, enablePan: false, enableDamping: true, dampingFactor: 0.06, autoRotate: !reduce, autoRotateSpeed: 0.55 });
  controls.minPolarAngle = Math.PI * 0.25;
  controls.maxPolarAngle = Math.PI * 0.62;

  const world = new THREE.Group();
  scene.add(world);

  // Starfield: depth cue only.
  const starGeo = new THREE.BufferGeometry();
  const stars = new Float32Array(1800 * 3);
  for (let i = 0; i < stars.length; i += 3) {
    const v = new THREE.Vector3().randomDirection().multiplyScalar(30 + Math.random() * 60);
    stars.set([v.x, v.y, v.z], i);
  }
  starGeo.setAttribute("position", new THREE.BufferAttribute(stars, 3));
  const starMat = new THREE.PointsMaterial({ color: 0x9fdfe6, size: 0.12, transparent: true, opacity: 0.55, depthWrite: false });
  const starfield = new THREE.Points(starGeo, starMat);
  scene.add(starfield);

  // Soft light for glows: one radial texture shared by every halo.
  const glowTex = (() => {
    const c = document.createElement("canvas"); c.width = c.height = 64;
    const g = c.getContext("2d")!, r = g.createRadialGradient(32, 32, 0, 32, 32, 32);
    r.addColorStop(0, "rgba(255,255,255,1)"); r.addColorStop(0.2, "rgba(191,246,250,0.55)"); r.addColorStop(1, "rgba(0,0,0,0)");
    g.fillStyle = r; g.fillRect(0, 0, 64, 64);
    return new THREE.CanvasTexture(c);
  })();
  const halo = (color: THREE.Color, size: number, opacity: number) => {
    const s = new THREE.Sprite(new THREE.SpriteMaterial({ map: glowTex, color, transparent: true, opacity, depthWrite: false, blending: THREE.AdditiveBlending }));
    s.scale.setScalar(size);
    return s;
  };

  // The core: a breathing light. The emblem itself is an HTML image pinned
  // over it (crisp at any size, never washed out by the glow).
  const core = new THREE.Group();
  const heart = halo(TEAL, 2.6, 0.5);
  const shell = new THREE.Mesh(new THREE.SphereGeometry(0.75, 16, 16), new THREE.MeshBasicMaterial({ visible: false }));
  // Tapping the core opens the chain's own numbers.
  shell.userData = { kind: "chain" };
  core.add(heart, shell);
  world.add(core);

  // Validators and the links between them.
  const nodes: THREE.Mesh[] = [];
  const links = new THREE.LineSegments(new THREE.BufferGeometry(), new THREE.LineBasicMaterial({ color: TEAL, transparent: true, opacity: 0.22 }));
  world.add(links);
  const nodeGeo = new THREE.SphereGeometry(0.075, 20, 20);
  const nodePos = (i: number, n: number, out = new THREE.Vector3()) => {
    const a = (i / n) * Math.PI * 2;
    return out.set(Math.cos(a) * RING, Math.sin(a * 2) * 0.35, Math.sin(a) * RING);
  };
  function setValidators(count: number) {
    const n = Math.max(1, Math.min(64, count | 0));
    if (n === nodes.length) return;
    nodes.forEach((m) => world.remove(m));
    nodes.length = 0;
    const pts: number[] = [];
    for (let i = 0; i < n; i++) {
      const m = new THREE.Mesh(nodeGeo, new THREE.MeshBasicMaterial({ color: ICE }));
      m.add(halo(TEAL, 0.9, 0.55));
      nodePos(i, n, m.position);
      m.userData = { kind: "validator", index: i };
      nodes.push(m); world.add(m);
      pts.push(0, 0, 0, m.position.x, m.position.y, m.position.z);
      for (let j = i + 1; j < n; j++) { const p = nodePos(j, n); pts.push(m.position.x, m.position.y, m.position.z, p.x, p.y, p.z); }
    }
    links.geometry.dispose();
    links.geometry = new THREE.BufferGeometry().setAttribute("position", new THREE.Float32BufferAttribute(pts, 3));
  }
  setValidators(4);

  // The chain: crystals on the spiral, joined in height order.
  const blocks: Block[] = [];
  const crystalGeo = new THREE.OctahedronGeometry(0.1, 0);
  const crystalEdges = new THREE.EdgesGeometry(crystalGeo);
  const chain = new THREE.Line(new THREE.BufferGeometry(), new THREE.LineBasicMaterial({ color: TEAL, transparent: true, opacity: 0.35 }));
  world.add(chain);
  function relayout() {
    const pts: number[] = [];
    // History can land after a live block: order by height, not arrival.
    blocks.sort((a, b) => a.height - b.height);
    blocks.forEach((b, i) => { spiral(blocks.length - 1 - i, b.target); pts.push(b.target.x, b.target.y, b.target.z); });
    chain.geometry.dispose();
    chain.geometry = new THREE.BufferGeometry().setAttribute("position", new THREE.Float32BufferAttribute(pts, 3));
  }

  const pulses: Pulse[] = [];
  const pulseGeo = new THREE.SphereGeometry(0.025, 8, 8);
  function pulse(from: THREE.Vector3, to: THREE.Vector3, color: THREE.Color, delay: number, dur: number, done?: () => void) {
    const mesh = new THREE.Mesh(pulseGeo, new THREE.MeshBasicMaterial({ color }));
    mesh.add(halo(color, 0.45, 0.9));
    mesh.visible = false; world.add(mesh);
    pulses.push({ mesh, from: from.clone(), to: to.clone(), t0: performance.now() + delay, dur, done });
  }

  let flare = 0;
  function addBlock(height: number, txs: number, animate: boolean) {
    const color = txs > 0 ? GOLD : ICE;
    const mesh = new THREE.Mesh(crystalGeo, new THREE.MeshBasicMaterial({ color, transparent: true, opacity: 0.28, depthWrite: false }));
    mesh.add(new THREE.LineSegments(crystalEdges, new THREE.LineBasicMaterial({ color, transparent: true, opacity: 0.95 })));
    mesh.userData = { kind: "block", height, txs };
    const b: Block = { mesh, height, txs, target: new THREE.Vector3(), born: performance.now() };
    const land = () => {
      if (blocks.some((o) => o.height === height)) return;
      world.add(mesh); blocks.push(b); relayout();
      if (blocks.length > MAX_BLOCKS) { world.remove(blocks.shift()!.mesh); relayout(); }
      flare = 1;
    };
    if (!animate || reduce || nodes.length === 0) { mesh.position.set(0, 0, 0); land(); mesh.position.copy(b.target); return; }
    // Proposal fans out to the committee, then the votes converge on commit.
    const n = nodes.length, origin = nodes[height % n].position;
    const O = new THREE.Vector3();
    nodes.forEach((m) => { if (m.position !== origin) pulse(origin, m.position, TEAL, 0, 420); });
    let votes = 0;
    nodes.forEach((m) => pulse(m.position, O, color, 460, 360, () => { if (++votes === n) { mesh.position.set(0, 0, 0); land(); } }));
    (nodes[height % n].material as THREE.MeshBasicMaterial).color.copy(ICE);
  }

  // Bloom on desktops: the glow is the look, and the costly part.
  let composer: EffectComposer | null = null;
  const light = () => document.documentElement.dataset.theme === "light";
  function theme() {
    scene.background = new THREE.Color(light() ? 0xf4f7fa : 0x05060d);
    starMat.color.set(light() ? 0x2a8a96 : 0x9fdfe6);
  }
  theme();
  new MutationObserver(theme).observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });

  function resize() {
    const w = Math.max(1, canvas.clientWidth), h = Math.max(1, canvas.clientHeight);
    const phone = w < PHONE;
    renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, phone ? 1 : 1.5));
    renderer.setSize(w, h, false);
    camera.aspect = w / h; camera.updateProjectionMatrix();
    // Wide: the scene sits right of the words; phones: above them. It orbits
    // its own centre and is shifted on screen by the view offset, so turning
    // it never swings it across the headline.
    world.scale.setScalar(phone ? 0.62 : 0.82);
    controls.target.set(0, 0, 0);
    if (phone) camera.setViewOffset(w, h, 0, h * 0.26, w, h);
    else camera.setViewOffset(w, h, -w * 0.25, h * 0.07, w, h);
    if (!phone && !composer) {
      composer = new EffectComposer(renderer);
      composer.addPass(new RenderPass(scene, camera));
      composer.addPass(new UnrealBloomPass(new THREE.Vector2(w, h), 0.55, 0.4, 0.2));
      composer.addPass(new OutputPass());
    }
    composer?.setSize(w, h);
  }
  resize();
  window.addEventListener("resize", resize);

  // Picking: a tap that did not drag opens the card for what it hit.
  // Nearest object on screen within a finger's reach: the glow is far larger
  // than the geometry, so a ray through the mesh alone misses what people aim at.
  const REACH = 28;
  const v = new THREE.Vector3();
  let downAt = { x: 0, y: 0 }, hovered: THREE.Object3D | null = null;
  const hit = (e: PointerEvent) => {
    const r = canvas.getBoundingClientRect();
    let best: THREE.Object3D | null = null, bestD = REACH;
    for (const o of [shell, ...nodes, ...blocks.map((b) => b.mesh)]) {
      o.getWorldPosition(v).project(camera);
      const d = Math.hypot((v.x + 1) / 2 * r.width - (e.clientX - r.left), (1 - v.y) / 2 * r.height - (e.clientY - r.top));
      if (d < bestD && v.z < 1) { best = o; bestD = d; }
    }
    return best;
  };
  canvas.addEventListener("pointerdown", (e) => { downAt = { x: e.clientX, y: e.clientY }; });
  canvas.addEventListener("pointermove", (e) => {
    const o = hit(e);
    if (o !== hovered) { hovered?.scale.setScalar(1); hovered = o; o?.scale.setScalar(1.6); canvas.style.cursor = o ? "pointer" : "grab"; }
  });
  canvas.addEventListener("pointerup", (e) => {
    if (Math.hypot(e.clientX - downAt.x, e.clientY - downAt.y) > 6) return;
    const o = hit(e), r = stage.getBoundingClientRect(), x = e.clientX - r.left, y = e.clientY - r.top;
    if (!o) { onPick(null); return; }
    const d = o.userData;
    onPick(d.kind === "chain" ? { kind: "chain", x, y }
      : d.kind === "block" ? { kind: "block", height: d.height, txs: d.txs, x, y }
      : { kind: "validator", index: d.index, of: nodes.length, x, y });
  });

  let visible = true, last = performance.now();
  const loop = (now: number) => {
    if (!visible) return;
    const dt = Math.min(0.05, (now - last) / 1000); last = now;
    controls.update();
    // Breathing: the whole scene rises and falls on one slow rhythm.
    const breath = 0.5 + 0.5 * Math.sin(now / 1900);
    (links.material as THREE.LineBasicMaterial).opacity = 0.1 + 0.14 * breath;
    (chain.material as THREE.LineBasicMaterial).opacity = 0.2 + 0.18 * breath;
    (heart.material as THREE.SpriteMaterial).opacity = 0.35 + 0.25 * breath + flare * 0.4;
    nodes.forEach((m, i) => m.scale.setScalar(1 + 0.18 * Math.sin(now / 900 + i * 1.3)));
    starfield.rotation.y += dt * 0.004;
    flare = Math.max(0, flare - dt * 1.8);
    heart.scale.setScalar(2.6 * (1 + flare * 0.5 + breath * 0.08));
    nodes.forEach((m) => (m.material as THREE.MeshBasicMaterial).color.lerp(TEAL, dt * 2.5));
    blocks.forEach((b) => {
      b.mesh.position.lerp(b.target, 1 - Math.pow(0.02, dt));
      b.mesh.rotation.x += dt * 0.35; b.mesh.rotation.y += dt * 0.5;
    });
    for (let i = pulses.length - 1; i >= 0; i--) {
      const p = pulses[i], t = (now - p.t0) / p.dur;
      if (t < 0) continue;
      p.mesh.visible = true;
      p.mesh.position.lerpVectors(p.from, p.to, Math.min(1, t)).y += Math.sin(Math.min(1, t) * Math.PI) * 0.35;
      if (t >= 1) { world.remove(p.mesh); (p.mesh.material as THREE.Material).dispose(); pulses.splice(i, 1); p.done?.(); }
    }
    if (emblem) {
      v.set(0, 0, 0).applyMatrix4(world.matrixWorld).project(camera);
      emblem.style.transform = `translate(${((v.x + 1) / 2) * canvas.clientWidth}px, ${((1 - v.y) / 2) * canvas.clientHeight}px) translate(-50%, -50%) scale(${world.scale.x * (1 + flare * 0.06)})`;
    }
    if (composer && !light()) composer.render(); else renderer.render(scene, camera);
    requestAnimationFrame(loop);
  };
  const wake = () => { last = performance.now(); requestAnimationFrame(loop); };
  document.addEventListener("visibilitychange", () => { const was = visible; visible = !document.hidden; if (visible && !was) wake(); });
  new IntersectionObserver(([e]) => { const was = visible; visible = e.isIntersecting && !document.hidden; if (visible && !was) wake(); }).observe(stage);
  requestAnimationFrame(loop);

  return { addBlock, setValidators };
}
