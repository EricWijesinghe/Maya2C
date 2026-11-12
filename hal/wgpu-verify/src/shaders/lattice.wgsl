// ML-DSA-65 verification's lattice step (FIPS 204 Algorithm 8, line 9):
//   w_approx = NTT^-1(A_hat o NTT(z) - NTT(c) o NTT(t1 * 2^d))
// with every intermediate written out so the host can compare it with the CPU.
//
// Q, NEG_QINV, R2, F_MONT, K, L, N, the layout offsets and ZETAS are prepended
// by `shader::source()` from `params.rs`; nothing here restates a number. There
// is no parameter-set argument: K and L are constants (threat model G-7).
//
// One workgroup per signature, 128 invocations, one butterfly each per layer.
// Integer only. WGSL has no 64-bit integer, so the 23x23-bit products are built
// from 16-bit limbs in `mul_wide`.

struct Job {
    count: u32,
    nonce: u32,
    pad0: u32,
    pad1: u32,
}

@group(0) @binding(0) var<uniform> job: Job;
@group(0) @binding(1) var<storage, read> inp: array<u32>;
@group(0) @binding(2) var<storage, read_write> outp: array<u32>;

var<workgroup> poly: array<u32, 256>;

// (lo, hi) of the full 64-bit product a * b.
fn mul_wide(a: u32, b: u32) -> vec2<u32> {
    let a0 = a & 0xffffu;
    let a1 = a >> 16u;
    let b0 = b & 0xffffu;
    let b1 = b >> 16u;
    let p00 = a0 * b0;
    let p01 = a0 * b1;
    let p10 = a1 * b0;
    let p11 = a1 * b1;
    let mid = (p00 >> 16u) + (p01 & 0xffffu) + (p10 & 0xffffu);
    let lo = (p00 & 0xffffu) | (mid << 16u);
    let hi = p11 + (p01 >> 16u) + (p10 >> 16u) + (mid >> 16u);
    return vec2<u32>(lo, hi);
}

// a * b * 2^-32 mod Q for a, b < Q; the same steps as `arith::mont_mul`.
fn mont_mul(a: u32, b: u32) -> u32 {
    let t = mul_wide(a, b);
    let m = t.x * NEG_QINV;
    let u = mul_wide(m, Q);
    let lo = t.x + u.x;
    let carry = select(0u, 1u, lo < t.x);
    let r = t.y + u.y + carry;
    return select(r, r - Q, r >= Q);
}

fn add_mod(a: u32, b: u32) -> u32 {
    let s = a + b;
    return select(s, s - Q, s >= Q);
}

fn sub_mod(a: u32, b: u32) -> u32 {
    return add_mod(a, Q - b);
}

fn mul_mod(a: u32, b: u32) -> u32 {
    return mont_mul(mont_mul(a, b), R2);
}

fn load_poly(base: u32, t: u32) {
    poly[t] = inp[base + t];
    poly[t + 128u] = inp[base + t + 128u];
    workgroupBarrier();
}

fn store_poly(base: u32, t: u32) {
    outp[base + t] = poly[t];
    outp[base + t + 128u] = poly[t + 128u];
    workgroupBarrier();
}

// Algorithm 41. At layer `len`, butterfly `t` is in group t / len, and the
// group's zeta index is the `m` that `arith::ntt` reaches there: 128/len + grp.
fn ntt_poly(t: u32) {
    var len = 128u;
    loop {
        if len == 0u {
            break;
        }
        let grp = t / len;
        let j = grp * 2u * len + t % len;
        let x = mont_mul(ZETAS[128u / len + grp], poly[j + len]);
        let a = poly[j];
        poly[j + len] = sub_mod(a, x);
        poly[j] = add_mod(a, x);
        workgroupBarrier();
        len = len >> 1u;
    }
}

// Algorithm 42. `arith::inv_ntt` counts m down from 256, so at layer `len`
// group g uses m = 256/len - 1 - g.
fn inv_ntt_poly(t: u32) {
    var len = 1u;
    loop {
        if len == 256u {
            break;
        }
        let grp = t / len;
        let j = grp * 2u * len + t % len;
        let a = poly[j];
        let b = poly[j + len];
        poly[j] = add_mod(a, b);
        poly[j + len] = mont_mul(Q - ZETAS[256u / len - 1u - grp], sub_mod(a, b));
        workgroupBarrier();
        len = len << 1u;
    }
    poly[t] = mont_mul(F_MONT, poly[t]);
    poly[t + 128u] = mont_mul(F_MONT, poly[t + 128u]);
    workgroupBarrier();
}

@compute @workgroup_size(128)
fn lattice(
    @builtin(local_invocation_index) t: u32,
    @builtin(workgroup_id) wg: vec3<u32>,
) {
    let item = wg.x;
    if item >= job.count {
        return;
    }
    let ib = item * WORDS_IN;
    let ob = item * WORDS_OUT;
    if t == 0u {
        outp[ob] = job.nonce;
        outp[ob + 1u] = item;
    }

    for (var l = 0u; l < L; l = l + 1u) {
        load_poly(ib + IN_Z + l * N, t);
        ntt_poly(t);
        store_poly(ob + OUT_Z_HAT + l * N, t);
    }
    load_poly(ib + IN_C, t);
    ntt_poly(t);
    store_poly(ob + OUT_C_HAT, t);
    for (var k = 0u; k < K; k = k + 1u) {
        load_poly(ib + IN_T1 + k * N, t);
        ntt_poly(t);
        store_poly(ob + OUT_T1_HAT + k * N, t);
    }
    storageBarrier();

    for (var k = 0u; k < K; k = k + 1u) {
        for (var h = 0u; h < 2u; h = h + 1u) {
            let n = t + h * 128u;
            var sum = 0u;
            for (var l = 0u; l < L; l = l + 1u) {
                let a = inp[ib + IN_A_HAT + (k * L + l) * N + n];
                sum = add_mod(sum, mul_mod(a, outp[ob + OUT_Z_HAT + l * N + n]));
            }
            let ct = mul_mod(outp[ob + OUT_C_HAT + n], outp[ob + OUT_T1_HAT + k * N + n]);
            poly[n] = sub_mod(sum, ct);
        }
        workgroupBarrier();
        inv_ntt_poly(t);
        store_poly(ob + OUT_W_APPROX + k * N, t);
    }
}
