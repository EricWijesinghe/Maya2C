// three.js renderer for the propagation scene. Loaded with a dynamic import, so
// phones and reduced-motion visitors never download three. Every frame is drawn
// from the pure model in propagation-model.ts; this file only places pixels.
//
// Cost: one instanced mesh for all nodes, one line-segments buffer for all edges,
// one for beams, and the shell is a single quad drawn in-shader. Post effects
// (shimmer, bloom swell) live in shader and sprite math, not in an extra pass.
import * as THREE from "three";
import {
  buildModel, beamAt, cameraPosition, contactFlash, edgeLevel, finalityAt, lockAt, liftAt,
  nodeLevel, phaseAt, ringAt, yawAt, PERIOD,
} from "./propagation-model.ts";

const DPR_CAP = 1.5;
const FRAME_MS = 1000 / 30; // 30 fps: enough for slow, heavy motion
const CLOCK_STEP = 0.1; // a backgrounded tab advances the clock at most this much per frame
const SHELL_HALF_MARGIN = 0.9; // the shell quad reaches this far past the crest
const COL = {
  dim: new THREE.Color("#1d3c44"),
  peer: new THREE.Color("#7eeaf0"),
  validator: new THREE.Color("#e6b260"),
  mlDsa: new THREE.Color("#2fd9e6"),
  slhDsa: new THREE.Color("#e6b260"),
  core: new THREE.Color("#fff3d6"),
  white: new THREE.Color("#ffffff"),
  fog: new THREE.Color("#050b0f"),
};

const shellVert = /* glsl */ `
  varying vec2 vP;
  void main() {
    vP = uv * 2.0 - 1.0;
    gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
  }`;

// The wavefront: a thick bright shell with a heat-shimmer edge. The edge is
// wobbled by two angular waves that drift with time, so it reads as pressure
// bending the air rather than as a clean ring.
const shellFrag = /* glsl */ `
  uniform float uR;
  uniform float uHalf;
  uniform float uW;
  uniform float uA;
  uniform float uTime;
  uniform vec3 uTeal;
  uniform vec3 uCream;
  varying vec2 vP;
  void main() {
    float d = length(vP) * uHalf;
    float ang = atan(vP.y, vP.x);
    float shimmer = 0.09 * sin(ang * 14.0 + uTime * 4.0) + 0.04 * sin(ang * 31.0 - uTime * 7.0);
    float dd = d + shimmer;
    float band = exp(-pow((dd - uR) / uW, 2.0));
    float haze = 0.22 * exp(-pow((dd - uR + uW * 1.9) / (uW * 2.4), 2.0));
    float k = band + haze;
    vec3 col = mix(uTeal, uCream, clamp(band * band, 0.0, 1.0));
    gl_FragColor = vec4(col * k * uA * 0.6, k * uA * 0.6);
  }`;

export function startPropagation(canvas: HTMLCanvasElement, onPhase: (i: number) => void): void {
  const m = buildModel();
  const renderer = new THREE.WebGLRenderer({ canvas, antialias: true, alpha: true });
  renderer.setPixelRatio(Math.min(window.devicePixelRatio, DPR_CAP));
  const camera = new THREE.PerspectiveCamera(34, 1, 0.1, 60);
  const scene = new THREE.Scene();
  scene.fog = new THREE.Fog(COL.fog, 9, 26);
  const world = new THREE.Group();
  scene.add(world);

  const N = m.points.length;
  const isValidator = new Set(m.validators);
  const radius = m.points.map((_, i) => (isValidator.has(i) ? 0.075 : i === m.core ? 0.12 : 0.04));

  // Nodes: one instanced draw.
  const nodes = new THREE.InstancedMesh(new THREE.SphereGeometry(1, 8, 6), new THREE.MeshBasicMaterial(), N);
  world.add(nodes);

  // Edges: one line-segments draw, with positions rewritten each frame as the lattice lifts.
  const E = m.edges.length;
  const edgeGeo = new THREE.BufferGeometry();
  edgeGeo.setAttribute("position", new THREE.BufferAttribute(new Float32Array(E * 6), 3));
  edgeGeo.setAttribute("color", new THREE.BufferAttribute(new Float32Array(E * 6), 3));
  const edgeMat = new THREE.LineBasicMaterial({ vertexColors: true, transparent: true, blending: THREE.AdditiveBlending, depthWrite: false });
  world.add(new THREE.LineSegments(edgeGeo, edgeMat));

  // Signature beams: two per validator, one ML-DSA (teal) and one SLH-DSA (bronze).
  const B = m.validators.length * 2;
  const beamGeo = new THREE.BufferGeometry();
  beamGeo.setAttribute("position", new THREE.BufferAttribute(new Float32Array(B * 6), 3));
  beamGeo.setAttribute("color", new THREE.BufferAttribute(new Float32Array(B * 6), 3));
  world.add(new THREE.LineSegments(beamGeo, new THREE.LineBasicMaterial({ vertexColors: true, transparent: true, blending: THREE.AdditiveBlending, depthWrite: false })));

  // The core: a small bright sphere and an additive halo that swells at finality.
  const core = new THREE.Mesh(new THREE.SphereGeometry(1, 16, 12), new THREE.MeshBasicMaterial({ color: COL.core }));
  world.add(core);
  const glowCanvas = document.createElement("canvas");
  glowCanvas.width = glowCanvas.height = 128;
  const g = glowCanvas.getContext("2d");
  if (!g) throw new Error("2d context unavailable");
  const grad = g.createRadialGradient(64, 64, 0, 64, 64, 64);
  grad.addColorStop(0, "rgba(255,243,214,1)");
  grad.addColorStop(0.35, "rgba(126,234,240,0.45)");
  grad.addColorStop(1, "rgba(126,234,240,0)");
  g.fillStyle = grad;
  g.fillRect(0, 0, 128, 128);
  const halo = new THREE.Sprite(new THREE.SpriteMaterial({ map: new THREE.CanvasTexture(glowCanvas), transparent: true, blending: THREE.AdditiveBlending, depthWrite: false }));
  world.add(halo);

  // The shell: one quad, shaded per pixel.
  const shellMat = new THREE.ShaderMaterial({
    uniforms: {
      uR: { value: 0 }, uHalf: { value: 1 }, uW: { value: 0.36 }, uA: { value: 0 }, uTime: { value: 0 },
      uTeal: { value: new THREE.Color("#7eeaf0") }, uCream: { value: new THREE.Color("#fff3d6") },
    },
    vertexShader: shellVert, fragmentShader: shellFrag,
    transparent: true, blending: THREE.AdditiveBlending, depthWrite: false,
  });
  const shell = new THREE.Mesh(new THREE.PlaneGeometry(1, 1), shellMat);
  shell.rotation.x = -Math.PI / 2;
  shell.visible = false;
  world.add(shell);

  const nodePos = new Float32Array(N * 3);
  const tmp = new THREE.Object3D();
  const cc = new THREE.Color();
  const from = new THREE.Vector3(), head = new THREE.Vector3(), tail = new THREE.Vector3();
  const corePos = new THREE.Vector3(...m.points[m.core]);
  let sim = 0;
  let last = performance.now();
  let lastDraw = 0;
  let raf = 0;
  let visible = true;
  let lastPhase = -1;
  let yaw = 0;
  let cam = new THREE.Vector3();

  function resize() {
    const w = canvas.clientWidth, h = canvas.clientHeight;
    if (!w || !h) return;
    renderer.setSize(w, h, false);
    camera.aspect = w / h;
    // Keep the whole field in frame on narrow windows.
    camera.fov = w / h < 1.2 ? 44 : 34;
    // The canvas spans the stage; on wide screens the field sits right of the copy.
    const shift = w >= 900 ? -w * 0.3 : 0;
    camera.setViewOffset(w, h, shift, 0, w, h);
    camera.updateProjectionMatrix();
  }

  // Positions of all nodes for this frame: the lift under the shell, then a slow
  // draw-in toward the core on lock.
  function placeNodes(t: number, lock: number) {
    const c = 1 - 0.06 * lock;
    for (let i = 0; i < N; i++) {
      const p = m.points[i];
      nodePos[i * 3] = corePos.x + (p[0] - corePos.x) * c;
      nodePos[i * 3 + 1] = p[1] + liftAt(m, i, t);
      nodePos[i * 3 + 2] = corePos.z + (p[2] - corePos.z) * c;
    }
  }

  function drawNodes(t: number, fin: ReturnType<typeof finalityAt>) {
    for (let i = 0; i < N; i++) {
      const lv = nodeLevel(m, i, t);
      const fl = contactFlash(m, i, t);
      const locked = i === m.core || isValidator.has(i);
      const pulse = locked && t >= m.finalAt ? fin.pulse : 1;
      const s = radius[i] * (1 + 0.7 * fl) * (i === m.core ? 1 + 0.3 * fin.flash : 1);
      tmp.position.set(nodePos[i * 3], nodePos[i * 3 + 1], nodePos[i * 3 + 2]);
      tmp.scale.setScalar(s);
      tmp.updateMatrix();
      nodes.setMatrixAt(i, tmp.matrix);
      const base = isValidator.has(i) ? COL.validator : COL.peer;
      cc.copy(COL.dim).lerp(i === m.core ? COL.core : base, lv).lerp(COL.white, fl * 0.6).multiplyScalar(pulse);
      nodes.setColorAt(i, cc);
    }
    nodes.instanceMatrix.needsUpdate = true;
    if (nodes.instanceColor) nodes.instanceColor.needsUpdate = true;
  }

  function drawEdges(t: number) {
    const pos = edgeGeo.getAttribute("position") as THREE.BufferAttribute;
    const col = edgeGeo.getAttribute("color") as THREE.BufferAttribute;
    for (let e = 0; e < E; e++) {
      const [a, b] = m.edges[e];
      pos.setXYZ(e * 2, nodePos[a * 3], nodePos[a * 3 + 1], nodePos[a * 3 + 2]);
      pos.setXYZ(e * 2 + 1, nodePos[b * 3], nodePos[b * 3 + 1], nodePos[b * 3 + 2]);
      cc.copy(COL.dim).lerp(COL.peer, Math.min(1, edgeLevel(m, e, t)));
      col.setXYZ(e * 2, cc.r, cc.g, cc.b);
      col.setXYZ(e * 2 + 1, cc.r, cc.g, cc.b);
    }
    pos.needsUpdate = true;
    col.needsUpdate = true;
  }

  function drawBeams(t: number) {
    const bp = beamGeo.getAttribute("position") as THREE.BufferAttribute;
    const bc = beamGeo.getAttribute("color") as THREE.BufferAttribute;
    m.validators.forEach((v, k) => {
      from.set(nodePos[v * 3], nodePos[v * 3 + 1], nodePos[v * 3 + 2]);
      for (const kind of [0, 1] as const) {
        const b = beamAt(m, k, kind, t);
        const idx = k * 2 + kind;
        head.copy(from).lerp(corePos, b.p);
        tail.copy(from).lerp(corePos, Math.max(0, b.p - 0.2));
        bp.setXYZ(idx * 2, tail.x, tail.y, tail.z);
        bp.setXYZ(idx * 2 + 1, head.x, head.y, head.z);
        cc.copy(kind === 0 ? COL.mlDsa : COL.slhDsa).multiplyScalar(b.alpha);
        bc.setXYZ(idx * 2, 0, 0, 0);
        bc.setXYZ(idx * 2 + 1, cc.r, cc.g, cc.b);
      }
    });
    bp.needsUpdate = true;
    bc.needsUpdate = true;
  }

  function frame(now: number) {
    raf = 0;
    if (!visible || document.hidden) return;
    if (now - lastDraw < FRAME_MS) { raf = requestAnimationFrame(frame); return; }
    const dt = Math.min((now - last) / 1000, CLOCK_STEP);
    last = now;
    lastDraw = now;
    sim += dt;
    const t = sim % PERIOD;
    const fin = finalityAt(m, t);
    const lock = lockAt(m, t);
    const phase = phaseAt(m, t);
    if (phase !== lastPhase) { lastPhase = phase; onPhase(phase); }

    // Heavy camera: the push-in follows the model; yaw eases toward its target.
    yaw += (yawAt(t) - yaw) * (1 - Math.exp(-dt * 2.2));
    world.rotation.y = yaw;
    const [cx, cy, cz] = cameraPosition(m, t);
    const shake = fin.shake;
    cam.set(cx + shake * Math.sin(t * 61), cy + shake * 0.6 * Math.cos(t * 47), cz);
    camera.position.copy(cam);
    camera.lookAt(0, 0, 0);

    placeNodes(t, lock);
    drawNodes(t, fin);
    drawEdges(t);
    drawBeams(t);

    const rg = ringAt(m, t);
    shell.visible = rg.a > 0.001;
    shell.position.copy(corePos);
    shell.scale.setScalar(2 * (rg.r + SHELL_HALF_MARGIN));
    shellMat.uniforms.uR.value = rg.r;
    shellMat.uniforms.uHalf.value = rg.r + SHELL_HALF_MARGIN;
    shellMat.uniforms.uA.value = rg.a;
    shellMat.uniforms.uTime.value = t;

    core.position.copy(corePos);
    core.scale.setScalar(0.12 + 0.05 * fin.flash);
    halo.position.copy(corePos);
    halo.scale.setScalar(0.5 + 1.8 * fin.flash);
    (halo.material as THREE.SpriteMaterial).opacity = 0.3 + 0.7 * fin.flash;

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
  canvas.parentElement?.classList.add("gl");
  run();
}

