// GLSL for the shared Ambient field. Two programs share one WebGL context:
// point sprites for the home lattice and the sunrise particles, and a single
// full-screen pass for the aurora, blueprint and circuit motifs. Colours come
// in as uniforms from the section tokens in maya-living.css, so the shaders
// hold no palette of their own.

export const POINT_VS = `attribute vec4 a;
uniform float uT; uniform float uS; uniform float uDpr; uniform float uSpeed; uniform float uSunrise;
varying float vA; varying float vK;
void main(){
  float z = a.z;
  float tt = uT * uSpeed;
  float x; float y;
  if (uSunrise < 0.5) {
    // Lattice: slow drift, scroll parallax (near points move more than far ones).
    x = a.x + sin(tt * 0.05 + a.y * 3.0) * 0.04 / z + tt * 0.02 / z;
    y = fract((a.y + 1.0) * 0.5 - uS * 0.12 / z) * 2.0 - 1.0;
  } else {
    // Sunrise: particles rise, each at its own depth, and sway a little.
    x = a.x + sin(tt * 0.2 + a.y * 6.0) * 0.03 / z;
    y = fract((a.y + 1.0) * 0.5 + tt * 0.03 / z - uS * 0.05 / z) * 2.0 - 1.0;
  }
  gl_Position = vec4(fract((x + 1.0) * 0.5) * 2.0 - 1.0, y, 0.0, 1.0);
  gl_PointSize = (1.6 + 4.2 / z) * uDpr;
  vA = 0.25 + 0.45 / z;
  vK = a.w;
}`;

export const POINT_FS = `precision mediump float;
uniform vec3 uA; uniform vec3 uB; uniform vec3 uC; uniform float uAlpha;
varying float vA; varying float vK;
void main(){
  vec2 d = gl_PointCoord - 0.5;
  float r = length(d);
  if (r > 0.5) discard;
  float core = smoothstep(0.5, 0.0, r);
  vec3 c = vK < 0.5 ? uA : (vK < 1.5 ? uB : uC);
  gl_FragColor = vec4(c, core * vA * 0.9 * uAlpha);
}`;

export const FIELD_VS = `attribute vec2 a;
void main(){ gl_Position = vec4(a, 0.0, 1.0); }`;

export const FIELD_FS = `precision highp float;
uniform vec2 uRes; uniform float uDpr; uniform float uT; uniform float uS; uniform float uSpeed;
uniform float uAlpha; uniform float uMotif;
uniform vec3 uA; uniform vec3 uB; uniform vec3 uC;

float hash(vec2 p){ return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453); }

// One soft band of light; p.y is in screen heights, so bands keep their shape on any aspect.
float ribbon(vec2 p, float seed, float t){
  float c = 0.5 + 0.16 * sin(p.x * 2.1 + t * 0.30 + seed) + 0.06 * sin(p.x * 4.7 - t * 0.21 + seed * 2.0);
  float w = 0.09 + 0.04 * sin(p.x * 2.9 + seed + t * 0.08);
  float band = exp(-pow((p.y - c) / w, 2.0));
  return band * (0.6 + 0.4 * sin(p.x * 6.0 + t * 0.45 + seed * 3.0));
}

vec4 aurora(vec2 p, float t){
  float a = ribbon(p, 0.0, t) * 0.5 + ribbon(p - vec2(0.0, 0.05), 1.7, t * 1.1) * 0.35
          + ribbon(p + vec2(0.0, 0.04), 3.1, t * 0.9) * 0.25;
  vec3 col = mix(uA, uB, clamp(p.y * 1.4 - 0.2, 0.0, 1.0));
  // Warm bronze haze low in the field keeps the violet-teal soft.
  col = mix(col, uC, 0.25 * smoothstep(0.6, 0.0, p.y));
  return vec4(col, clamp(a, 0.0, 1.0) * 0.6);
}

// Blueprint: a 48px grid, with a fraction of the corners holding nodes that breathe.
vec4 blueprint(vec2 px, float t){
  float cell = 48.0;
  vec2 m = mod(px, cell);
  vec2 d = min(m, cell - m);
  float grid = 1.0 - smoothstep(0.4, 1.1, min(d.x, d.y));
  vec2 cid = floor(px / cell + 0.5);
  float h = hash(cid);
  float dist = length(px - cid * cell);
  float pulse = 0.5 + 0.5 * sin(t * 0.55 + h * 6.2831);
  float node = step(0.82, h) * (1.0 - smoothstep(2.0 + pulse, 2.8 + pulse, dist));
  vec3 col = mix(uA, uB, node);
  return vec4(col, clamp(grid * 0.13 + node * (0.45 + 0.4 * pulse), 0.0, 1.0));
}

// Circuit: 64px cells, each holding one straight trace; a current pulse runs along it.
vec4 circuit(vec2 px, float t){
  float cell = 64.0;
  vec2 cid = floor(px / cell);
  vec2 l = px - cid * cell;
  float h = hash(cid);
  float hz = step(h, 0.5);
  float along = mix(l.y, l.x, hz);
  float across = mix(l.x, l.y, hz);
  float trace = 1.0 - smoothstep(0.6, 1.4, abs(across - cell * 0.5));
  float pos = fract(h * 13.0 + t * (0.18 + 0.18 * h)) * cell;
  float pulse = exp(-pow((along - pos) / 4.0, 2.0)) * trace;
  vec3 col = mix(uA, uC, clamp(pulse * 1.5, 0.0, 1.0));
  // Kept faint: the traces run behind body text, so they must not cut its contrast.
  return vec4(col, clamp(trace * 0.13 + pulse * 0.7, 0.0, 1.0));
}

void main(){
  vec2 px = gl_FragCoord.xy / uDpr;
  vec2 p = gl_FragCoord.xy / uRes.y;
  p.y += uS * 0.05;
  float t = uT * uSpeed;
  vec4 c;
  if (uMotif < 2.5) c = aurora(p, t);
  else if (uMotif < 3.5) c = blueprint(px, t);
  else c = circuit(px, t);
  gl_FragColor = vec4(c.rgb, c.a * uAlpha);
}`;
