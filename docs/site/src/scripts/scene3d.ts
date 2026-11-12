// The two below-the-fold 3D scenes: the supply coin and the network
// constellation. Same three.js chunk as the hero (src/scripts/hero3d.ts), loaded
// only when a scene nears the screen (gate3d.ts), paused off-screen and in a
// hidden tab. Data drives every value that means something: the ring's arc is
// get_supply's own ratio, node counts are get_bft_status and the status
// service, and a pulse is a block the explorer streamed. Positions are art.
import * as THREE from "three";
import { dpr, onRunnable } from "./gate3d";

const TEAL = 0x2cc3cf;
const BRONZE = 0xd9b97a;
const ICE = 0xbff6fa;
const STEEL = 0x8a9ca3;

function glowTexture(): THREE.Texture {
  const c = document.createElement("canvas"); c.width = c.height = 64;
  const g = c.getContext("2d")!, r = g.createRadialGradient(32, 32, 0, 32, 32, 32);
  r.addColorStop(0, "rgba(255,255,255,1)"); r.addColorStop(0.25, "rgba(191,246,250,0.5)"); r.addColorStop(1, "rgba(0,0,0,0)");
  g.fillStyle = r; g.fillRect(0, 0, 64, 64);
  return new THREE.CanvasTexture(c);
}

/** Cheap bloom: an additive sprite behind a bright thing, no post-processing pass. */
function halo(tex: THREE.Texture, color: number, size: number, opacity: number): THREE.Sprite {
  const s = new THREE.Sprite(new THREE.SpriteMaterial({ map: tex, color, transparent: true, opacity, depthWrite: false, blending: THREE.AdditiveBlending }));
  s.scale.setScalar(size);
  return s;
}

/** Renderer, camera and a paused-when-hidden loop: the part every scene shares. */
function stage(canvas: HTMLCanvasElement, fov: number, z: number, frame: (t: number, dt: number) => void) {
  const renderer = new THREE.WebGLRenderer({ canvas, antialias: true, alpha: true, powerPreference: "low-power" });
  const scene = new THREE.Scene();
  const camera = new THREE.PerspectiveCamera(fov, 1, 0.1, 100);
  camera.position.set(0, 0, z);
  const resize = () => {
    const w = Math.max(1, canvas.clientWidth), h = Math.max(1, canvas.clientHeight);
    renderer.setPixelRatio(dpr()); renderer.setSize(w, h, false);
    camera.aspect = w / h; camera.updateProjectionMatrix();
  };
  resize();
  new ResizeObserver(resize).observe(canvas);
  // Pointer parallax: the scene leans toward the cursor, a few degrees at most.
  const lean = { x: 0, y: 0 };
  canvas.parentElement?.addEventListener("pointermove", (e) => {
    const r = canvas.getBoundingClientRect();
    lean.x = ((e.clientX - r.left) / r.width - 0.5) * 0.5; lean.y = ((e.clientY - r.top) / r.height - 0.5) * 0.35;
  });
  let running = false, last = 0;
  const loop = (t: number) => {
    if (!running) return;
    const dt = Math.min(0.05, (t - last) / 1000); last = t;
    scene.rotation.y += (lean.x - scene.rotation.y) * 0.05;
    scene.rotation.x += (lean.y - scene.rotation.x) * 0.05;
    frame(t, dt);
    renderer.render(scene, camera);
    requestAnimationFrame(loop);
  };
  onRunnable(canvas, (on) => { if (on && !running) { running = true; last = performance.now(); requestAnimationFrame(loop); } else running = on; });
  return { scene, camera };
}

export interface SupplyCoin { setShare(circulating: number | null, burned: number | null): void }

/** A bronze coin with the emblem on its face, inside a ring whose teal arc is circulating / total. */
export function mountSupplyCoin(canvas: HTMLCanvasElement, emblemUrl: string): SupplyCoin {
  const tex = glowTexture();
  const coin = new THREE.Group();
  const face = new THREE.TextureLoader().load(emblemUrl);
  face.colorSpace = THREE.SRGBColorSpace;
  const metal = new THREE.MeshStandardMaterial({ color: BRONZE, metalness: 0.85, roughness: 0.32 });
  const faceMat = new THREE.MeshStandardMaterial({ map: face, metalness: 0.4, roughness: 0.45, transparent: true });
  // Cylinder groups: 0 side, 1 top, 2 bottom; the emblem goes on both faces.
  const body = new THREE.Mesh(new THREE.CylinderGeometry(1, 1, 0.16, 64), [metal, faceMat, faceMat]);
  body.rotation.x = Math.PI / 2;
  coin.add(halo(tex, TEAL, 3.4, 0.35), body);

  const track = new THREE.Mesh(new THREE.TorusGeometry(1.75, 0.035, 12, 160), new THREE.MeshBasicMaterial({ color: STEEL, transparent: true, opacity: 0.25 }));
  const arcMat = new THREE.MeshStandardMaterial({ color: TEAL, emissive: TEAL, emissiveIntensity: 0.6, metalness: 0.3, roughness: 0.4 });
  const burnMat = new THREE.MeshStandardMaterial({ color: BRONZE, emissive: BRONZE, emissiveIntensity: 0.35 });
  let arc: THREE.Mesh | null = null, burn: THREE.Mesh | null = null;
  let target = 0, shown = 0, burnFrac = 0;

  const { scene } = stage(canvas, 38, 6, (t, dt) => {
    body.rotation.z = Math.sin(t / 3000) * 0.6; // a slow sway: the face stays toward the viewer
    coin.position.y = Math.sin(t / 1400) * 0.06;
    // The arc grows from 0 to the real ratio once, then holds it.
    if (Math.abs(target - shown) > 0.0005) { shown += (target - shown) * Math.min(1, dt * 2.2); build(); }
  });
  scene.add(new THREE.AmbientLight(0xffffff, 0.35));
  const key = new THREE.PointLight(ICE, 30, 20); key.position.set(3, 3, 4);
  const rim = new THREE.PointLight(TEAL, 18, 20); rim.position.set(-4, -1, 2);
  scene.add(key, rim, coin, track);

  function build() {
    if (arc) { scene.remove(arc); arc.geometry.dispose(); }
    if (burn) { scene.remove(burn); burn.geometry.dispose(); burn = null; }
    // Torus arcs start at 3 o'clock; turn them so the ring fills from 12.
    arc = new THREE.Mesh(new THREE.TorusGeometry(1.75, 0.07, 16, 200, Math.max(0.0001, shown) * Math.PI * 2), arcMat);
    arc.rotation.z = Math.PI / 2;
    scene.add(arc);
    if (burnFrac > 0) {
      burn = new THREE.Mesh(new THREE.TorusGeometry(1.75, 0.07, 16, 60, Math.max(0.01, burnFrac) * Math.PI * 2), burnMat);
      burn.rotation.z = Math.PI / 2 + shown * Math.PI * 2;
      scene.add(burn);
    }
  }
  return {
    setShare(circulating, burned) {
      target = circulating ?? 0; burnFrac = burned ?? 0;
      if (circulating === null) shown = 0;
      build();
    },
  };
}

export interface Constellation { setValidators(n: number): void; setPeers(n: number): void; block(height: number): void }

/** Fibonacci sphere: an even, art-only spread of n points. */
function sphere(i: number, n: number, r: number, out = new THREE.Vector3()) {
  const y = 1 - (2 * (i + 0.5)) / n, rad = Math.sqrt(1 - y * y), a = i * 2.399963;
  return out.set(Math.cos(a) * rad * r, y * r, Math.sin(a) * rad * r);
}

/** Validators (inner shell) and peers (outer shell); each streamed block sends pulses from a proposer to the others. */
export function mountConstellation(canvas: HTMLCanvasElement): Constellation {
  const tex = glowTexture();
  const group = new THREE.Group();
  const validators: THREE.Object3D[] = [];
  const peerGroup = new THREE.Group();
  const links = new THREE.LineSegments(new THREE.BufferGeometry(), new THREE.LineBasicMaterial({ color: TEAL, transparent: true, opacity: 0.2 }));
  group.add(links, peerGroup);
  const nodeGeo = new THREE.IcosahedronGeometry(0.09, 1);
  const pulses: { m: THREE.Sprite; a: THREE.Vector3; b: THREE.Vector3; t0: number }[] = [];
  const PULSE_MS = 900;

  const { scene } = stage(canvas, 40, 7.5, (t, dt) => {
    group.rotation.y += dt * 0.06;
    for (let i = pulses.length - 1; i >= 0; i--) {
      const p = pulses[i], k = (t - p.t0) / PULSE_MS;
      if (k >= 1) { group.remove(p.m); p.m.material.dispose(); pulses.splice(i, 1); continue; }
      p.m.position.lerpVectors(p.a, p.b, k);
      p.m.material.opacity = Math.sin(k * Math.PI);
    }
  });
  scene.add(group);

  return {
    setValidators(count) {
      const n = Math.max(0, Math.min(64, count | 0));
      if (n === validators.length) return;
      validators.forEach((v) => group.remove(v)); validators.length = 0;
      const pts: number[] = [];
      for (let i = 0; i < n; i++) {
        const m = new THREE.Mesh(nodeGeo, new THREE.MeshBasicMaterial({ color: ICE }));
        m.add(halo(tex, TEAL, 0.9, 0.6));
        sphere(i, n, 1.6, m.position);
        validators.push(m); group.add(m);
        for (let j = 0; j < i; j++) pts.push(...m.position.toArray(), ...validators[j].position.toArray());
      }
      links.geometry.dispose();
      links.geometry = new THREE.BufferGeometry().setAttribute("position", new THREE.Float32BufferAttribute(pts, 3));
    },
    setPeers(count) {
      const n = Math.max(0, Math.min(200, count | 0));
      peerGroup.clear();
      const pos = new Float32Array(n * 3), v = new THREE.Vector3();
      for (let i = 0; i < n; i++) pos.set(sphere(i, n, 2.8, v).toArray(), i * 3);
      const g = new THREE.BufferGeometry().setAttribute("position", new THREE.BufferAttribute(pos, 3));
      peerGroup.add(new THREE.Points(g, new THREE.PointsMaterial({ color: BRONZE, size: 0.09, transparent: true, opacity: 0.85 })));
    },
    block(height) {
      const n = validators.length;
      if (n < 2) return;
      const from = validators[height % n].position;
      validators.forEach((v) => {
        if (v.position === from) return;
        const m = halo(tex, BRONZE, 0.35, 0);
        group.add(m);
        pulses.push({ m, a: from.clone(), b: v.position.clone(), t0: performance.now() });
      });
    },
  };
}
