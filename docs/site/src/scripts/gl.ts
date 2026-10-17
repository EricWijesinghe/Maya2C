// Links a WebGL program without blocking the page. Where the browser offers
// KHR_parallel_shader_compile, compiling runs on driver threads and we poll for
// completion on animation frames; otherwise the link waits as usual. onReady
// runs only for a program that linked, so a failed shader leaves the caller's
// CSS fallback on screen.
export function linkAsync(
  gl: WebGLRenderingContext,
  vsSrc: string,
  fsSrc: string,
  onReady: (prog: WebGLProgram) => void,
): void {
  const vs = gl.createShader(gl.VERTEX_SHADER);
  const fs = gl.createShader(gl.FRAGMENT_SHADER);
  const prog = gl.createProgram();
  if (!vs || !fs || !prog) return;
  gl.shaderSource(vs, vsSrc);
  gl.compileShader(vs);
  gl.shaderSource(fs, fsSrc);
  gl.compileShader(fs);
  gl.attachShader(prog, vs);
  gl.attachShader(prog, fs);
  gl.linkProgram(prog);
  const ext = gl.getExtension("KHR_parallel_shader_compile") as { COMPLETION_STATUS_KHR: number } | null;
  const poll = () => {
    if (ext && !gl.getProgramParameter(prog, ext.COMPLETION_STATUS_KHR)) {
      requestAnimationFrame(poll);
      return;
    }
    if (gl.getProgramParameter(prog, gl.LINK_STATUS)) onReady(prog);
  };
  poll();
}
