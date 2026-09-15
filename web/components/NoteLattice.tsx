"use client";

import { useEffect, useRef } from "react";
import * as THREE from "three";

/**
 * The pool, and one deal moving through it.
 *
 * STRK20 is a shared note-based pool: every shielded position in it is a note.
 * The drifting field is that set. On top of it the scene plays one deal —
 * channel open, offers crossing, settlement, a scoped grant.
 *
 * The rule that makes this a diagram rather than an ornament: **only what
 * actually leaks is drawn.** The channel edge is cinnabar because the
 * counterparty is public calldata (F38). Each offer shows as a pulse because
 * timing and shape are public, and carries no value because the terms are not.
 * Settlement blooms exactly seven notes because that count is public. The grant
 * lights one deal, because a wire-v3 grant is scoped to one.
 *
 * So the animation is the observer's view. It does not change when you drop the
 * viewing key, and that is the whole argument of the page.
 *
 * Flat on purpose: orthographic camera, unlit constant-size marks, no lighting
 * model, no post-processing. It should reward a stare and disappear on a glance.
 */

const CYCLE = 15; // seconds

// phase boundaries, as fractions of one cycle
const OPEN = [0.1, 0.26] as const;
const OFFERS = [0.26, 0.54] as const;
const SETTLE = [0.54, 0.76] as const;
const GRANT = [0.76, 0.92] as const;

const ACTORS = 24; // A, B, C, seven settlement notes, pulses, headroom
const clamp01 = (v: number) => Math.min(1, Math.max(0, v));
const ramp = (t: number, a: number, b: number) => clamp01((t - a) / (b - a));
const ease = (t: number) => 1 - Math.pow(1 - t, 3);

export function NoteLattice({
  variant = "void",
  className = "",
  density = 14,
  highlight = true,
  story = false,
}: {
  variant?: "light" | "void";
  className?: string;
  density?: number;
  /** mark the settlement's seven notes in the static field */
  highlight?: boolean;
  /** play one deal through the pool */
  story?: boolean;
}) {
  const host = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = host.current;
    if (!el) return;

    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    const small = window.matchMedia("(max-width: 850px)").matches;
    const n = small ? Math.max(8, density - 4) : density;

    let renderer: THREE.WebGLRenderer;
    try {
      renderer = new THREE.WebGLRenderer({ alpha: true, antialias: true });
    } catch {
      return; // no WebGL: the section reads fine without it
    }

    renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    renderer.setClearAlpha(0);
    el.appendChild(renderer.domElement);
    Object.assign(renderer.domElement.style, { display: "block", width: "100%", height: "100%" });

    const isVoid = variant === "void";
    const INK = new THREE.Color(isVoid ? 0xd8d4cb : 0x0b0b0c);
    const LEAK = new THREE.Color(isVoid ? 0xe2492f : 0xc0301a);

    const scene = new THREE.Scene();
    const group = new THREE.Group();
    scene.add(group);

    const frustum = 2.15;
    const camZ = 10;
    const camera = new THREE.OrthographicCamera(-frustum, frustum, frustum, -frustum, 0.1, 40);
    camera.position.set(0, 0, camZ);

    // ── the anonymity set ────────────────────────────────────────────────
    const count = n * n * n;
    const offsets = new Float32Array(count * 3);
    const tint = new Float32Array(count);
    const seed = new Float32Array(count);
    const span = 3.2;

    let i = 0;
    for (let x = 0; x < n; x += 1)
      for (let y = 0; y < n; y += 1)
        for (let z = 0; z < n; z += 1) {
          const j = (v: number) =>
            (v / (n - 1) - 0.5) * span + (Math.random() - 0.5) * (span / n) * 0.7;
          offsets[i * 3] = j(x);
          offsets[i * 3 + 1] = j(y);
          offsets[i * 3 + 2] = j(z);
          seed[i] = Math.random();
          i += 1;
        }

    if (highlight && !story) {
      const chosen = new Set<number>();
      while (chosen.size < 7) chosen.add(Math.floor(Math.random() * count));
      chosen.forEach((k) => (tint[k] = 1));
    }

    const quad = new THREE.PlaneGeometry(1, 1);
    const fieldGeo = new THREE.InstancedBufferGeometry();
    fieldGeo.index = quad.index;
    fieldGeo.attributes.position = quad.attributes.position;
    fieldGeo.setAttribute("offset", new THREE.InstancedBufferAttribute(offsets, 3));
    fieldGeo.setAttribute("tint", new THREE.InstancedBufferAttribute(tint, 1));
    fieldGeo.setAttribute("alpha", new THREE.InstancedBufferAttribute(new Float32Array(count).fill(1), 1));
    fieldGeo.setAttribute("seed", new THREE.InstancedBufferAttribute(seed, 1));
    fieldGeo.instanceCount = count;

    const markShader = {
      vertexShader: /* glsl */ `
        attribute vec3 offset;
        attribute float tint;
        attribute float alpha;
        attribute float seed;
        uniform float uSize;
        uniform float uTime;
        uniform float uCamZ;
        uniform float uSpreadX;
        varying float vTint;
        varying float vFade;
        varying float vAlpha;
        varying vec2 vUv;
        varying float vStar;
        varying float vSeed;
        void main() {
          vTint = tint; vAlpha = alpha;
          vUv = position.xy;
          vSeed = seed;

          // A minority of the field reads as a fixed star rather than a note:
          // bigger, brighter, gently twinkling, independent of the settlement
          // tint. Deterministic off seed so it doesn't reshuffle every frame.
          float star = step(0.86, seed);
          vStar = star;

          vec3 p = offset;
          p.x *= uSpreadX;
          p.y += sin(uTime * 0.18 + seed * 6.2831) * 0.012;
          vec4 mv = modelViewMatrix * vec4(p, 1.0);

          float s = uSize * (1.0 + tint * 1.35) * (0.4 + 0.6 * alpha) * (1.0 + star * 0.85);
          mv.xy += position.xy * s;
          vFade = smoothstep(-1.75, 1.75, mv.z + uCamZ);
          gl_Position = projectionMatrix * mv;
        }
      `,
      fragmentShader: /* glsl */ `
        uniform vec3 uBase;
        uniform vec3 uLeak;
        uniform float uOpacity;
        uniform float uTime;
        varying float vTint;
        varying float vFade;
        varying float vAlpha;
        varying vec2 vUv;
        varying float vStar;
        varying float vSeed;
        void main() {
          // A glowing point, not a soft blob: a small bright core plus its own
          // separate, tighter-falling halo around it, instead of one wide
          // gradient that reads as haze once a few marks overlap.
          float d = length(vUv) * 2.0;
          float core = smoothstep(0.42, 0.0, d);
          float halo = pow(clamp(1.0 - d, 0.0, 1.0), 4.0);
          float glow = clamp(core + halo * 0.55, 0.0, 1.0);
          if (glow < 0.02) discard;

          vec3 c = mix(uBase, uLeak, vTint);
          float a = uOpacity * mix(0.28, 1.0, vFade);
          a = mix(a, 1.0, vTint) * vAlpha * glow;

          // stars sit brighter and drift in and out slowly, each on its own
          // clock (phased off its seed, not its screen position)
          float twinkle = 0.55 + 0.45 * sin(uTime * 1.3 + vSeed * 6.2831);
          a *= mix(1.0, 1.5 + 0.4 * twinkle, vStar);

          if (a < 0.004) discard;
          gl_FragColor = vec4(c, min(a, 1.0));
        }
      `,
    };

    const uniforms = () => ({
      uSize: { value: small ? 0.03 : 0.026 },
      uTime: { value: 0 },
      uCamZ: { value: camZ },
      uSpreadX: { value: 1 },
      uBase: { value: INK.clone() },
      uLeak: { value: LEAK.clone() },
      uOpacity: { value: isVoid ? 0.58 : 0.55 },
    });

    const fieldMat = new THREE.ShaderMaterial({
      ...markShader,
      transparent: true,
      depthWrite: false,
      uniforms: uniforms(),
    });
    const field = new THREE.Mesh(fieldGeo, fieldMat);
    field.frustumCulled = false;
    group.add(field);

    // ── the deal ─────────────────────────────────────────────────────────
    let actorGeo: THREE.InstancedBufferGeometry | null = null;
    let actorMat: THREE.ShaderMaterial | null = null;
    let aPos: THREE.InstancedBufferAttribute | null = null;
    let aTint: THREE.InstancedBufferAttribute | null = null;
    let aAlpha: THREE.InstancedBufferAttribute | null = null;
    let edges: THREE.LineSegments | null = null;
    let edgePos: THREE.BufferAttribute | null = null;
    let edgeMat: THREE.LineBasicMaterial | null = null;

    // A pays, B is paid, C is the grant recipient. Placed off the lattice axes
    // so the edge between them never lies along a row of the field.
    const A = new THREE.Vector3(-1.15, -0.42, 0.35);
    const B = new THREE.Vector3(1.2, 0.5, -0.3);
    const C = new THREE.Vector3(0.1, -1.32, 0.6);
    const notes = Array.from({ length: 7 }, (_, k) => {
      const a = (k / 7) * Math.PI * 2;
      return new THREE.Vector3(
        A.x + Math.cos(a) * 0.66,
        A.y + Math.sin(a) * 0.66,
        A.z + (k % 2 ? 0.3 : -0.3),
      );
    });

    if (story) {
      const pos = new Float32Array(ACTORS * 3);
      const tn = new Float32Array(ACTORS);
      const al = new Float32Array(ACTORS);
      actorGeo = new THREE.InstancedBufferGeometry();
      actorGeo.index = quad.index;
      actorGeo.attributes.position = quad.attributes.position;
      aPos = new THREE.InstancedBufferAttribute(pos, 3);
      aTint = new THREE.InstancedBufferAttribute(tn, 1);
      aAlpha = new THREE.InstancedBufferAttribute(al, 1);
      actorGeo.setAttribute("offset", aPos);
      actorGeo.setAttribute("tint", aTint);
      actorGeo.setAttribute("alpha", aAlpha);
      actorGeo.setAttribute("seed", new THREE.InstancedBufferAttribute(new Float32Array(ACTORS), 1));
      actorGeo.instanceCount = ACTORS;

      const u = uniforms();
      u.uSize.value = small ? 0.038 : 0.032;
      u.uOpacity.value = 1;
      actorMat = new THREE.ShaderMaterial({
        ...markShader,
        transparent: true,
        depthWrite: false,
        uniforms: u,
      });
      const actors = new THREE.Mesh(actorGeo, actorMat);
      actors.frustumCulled = false;
      group.add(actors);

      // two edges: the channel (A–B) and the grant (settlement–C)
      const eg = new THREE.BufferGeometry();
      edgePos = new THREE.BufferAttribute(new Float32Array(4 * 3), 3);
      eg.setAttribute("position", edgePos);
      edgeMat = new THREE.LineBasicMaterial({ transparent: true, opacity: 0, color: LEAK.clone() });
      edges = new THREE.LineSegments(eg, edgeMat);
      edges.frustumCulled = false;
      group.add(edges);
    }

    const setActor = (k: number, p: THREE.Vector3, t: number, a: number) => {
      if (!aPos || !aTint || !aAlpha) return;
      aPos.setXYZ(k, p.x, p.y, p.z);
      aTint.setX(k, t);
      aAlpha.setX(k, a);
    };

    const tmp = new THREE.Vector3();

    const playDeal = (t: number) => {
      if (!aPos || !aTint || !aAlpha || !edgePos || !edgeMat) return;

      for (let k = 0; k < ACTORS; k += 1) setActor(k, tmp.set(0, 0, 0), 0, 0);

      // the two parties. present from the moment the channel opens, and cinnabar,
      // because the pair is what the chain shows.
      const open = ease(ramp(t, OPEN[0], OPEN[1]));
      setActor(0, A, 1, open);
      setActor(1, B, 1, open);

      // the channel edge, drawn from A toward B
      tmp.copy(A).lerp(B, open);
      edgePos.setXYZ(0, A.x, A.y, A.z);
      edgePos.setXYZ(1, tmp.x, tmp.y, tmp.z);

      // offers. three crossings, alternating direction. a pulse and nothing else:
      // the traffic is public, the terms in it are not.
      let slot = 2;
      for (let m = 0; m < 3; m += 1) {
        const w = (OFFERS[1] - OFFERS[0]) / 3;
        const p = ramp(t, OFFERS[0] + m * w, OFFERS[0] + (m + 1) * w);
        if (p > 0 && p < 1) {
          const from = m % 2 === 0 ? A : B;
          const to = m % 2 === 0 ? B : A;
          tmp.copy(from).lerp(to, ease(p));
          setActor(slot, tmp, 1, Math.sin(p * Math.PI));
        }
        slot += 1;
      }

      // settlement: seven notes bloom out of the payer.
      const st = ease(ramp(t, SETTLE[0], SETTLE[1]));
      for (let k = 0; k < 7; k += 1) {
        tmp.copy(A).lerp(notes[k], st);
        setActor(slot + k, tmp, 1, st * (1 - ramp(t, GRANT[1], 1)));
      }

      // the grant: one thread, to one recipient, for one deal.
      const gr = ease(ramp(t, GRANT[0], GRANT[1]));
      setActor(slot + 7, C, 0, gr * 0.9);
      tmp.copy(notes[0]).lerp(C, gr);
      edgePos.setXYZ(2, notes[0].x, notes[0].y, notes[0].z);
      edgePos.setXYZ(3, tmp.x, tmp.y, tmp.z);

      edgeMat.opacity = 0.72 * open * (1 - ramp(t, GRANT[1], 1) * 0.75);
      edgePos.needsUpdate = true;
      aPos.needsUpdate = true;
      aTint.needsUpdate = true;
      aAlpha.needsUpdate = true;
    };

    const resize = () => {
      const { clientWidth: w, clientHeight: h } = el;
      if (!w || !h) return;
      const aspect = w / h;
      camera.left = -frustum * aspect;
      camera.right = frustum * aspect;
      camera.updateProjectionMatrix();
      renderer.setSize(w, h, false);
      // the field's own horizontal spread is narrower than the frustum by
      // design at aspect 1; stretch it out so the ambient dots reach both
      // edges of the screen instead of leaving bare margins on a wide viewport
      // (this is what shows behind the header). The actor mesh — the deal's
      // fixed A/B/C geometry — is left unscaled.
      fieldMat.uniforms.uSpreadX.value = (frustum * aspect) / (span / 2);
    };
    resize();
    const ro = new ResizeObserver(resize);
    ro.observe(el);

    group.rotation.set(-0.32, 0.5, 0);

    // the pool tilts toward the cursor. target set on pointermove, eased
    // toward each frame so a fast flick doesn't snap the field — this is
    // meant to feel like weight, not like a cursor-follower toy.
    const pointer = { x: 0, y: 0 };
    const pointerEased = { x: 0, y: 0 };
    const onPointerMove = (e: PointerEvent) => {
      pointer.x = (e.clientX / window.innerWidth) * 2 - 1;
      pointer.y = (e.clientY / window.innerHeight) * 2 - 1;
    };
    if (!reduced) window.addEventListener("pointermove", onPointerMove, { passive: true });

    let raf = 0;
    let visible = true;
    const io = new IntersectionObserver((e) => (visible = !!e[0]?.isIntersecting));
    io.observe(el);

    const clock = new THREE.Clock();
    const draw = (t: number) => {
      fieldMat.uniforms.uTime.value = t;
      if (actorMat) actorMat.uniforms.uTime.value = t;
      pointerEased.x += (pointer.x - pointerEased.x) * 0.045;
      pointerEased.y += (pointer.y - pointerEased.y) * 0.045;
      group.rotation.y = 0.5 + t * 0.045 + pointerEased.x * 0.22;
      group.rotation.x = -0.32 + Math.sin(t * 0.11) * 0.06 - pointerEased.y * 0.16;
      if (story) playDeal((t % CYCLE) / CYCLE);
      renderer.render(scene, camera);
    };

    const loop = () => {
      raf = requestAnimationFrame(loop);
      if (visible) draw(clock.getElapsedTime());
    };

    if (reduced) {
      // one still frame, taken at settlement, where the diagram says the most
      draw(CYCLE * (SETTLE[1] - 0.01));
    } else {
      loop();
    }

    return () => {
      cancelAnimationFrame(raf);
      window.removeEventListener("pointermove", onPointerMove);
      io.disconnect();
      ro.disconnect();
      fieldGeo.dispose();
      quad.dispose();
      fieldMat.dispose();
      actorGeo?.dispose();
      actorMat?.dispose();
      edges?.geometry.dispose();
      edgeMat?.dispose();
      renderer.dispose();
      el.removeChild(renderer.domElement);
    };
  }, [variant, density, highlight, story]);

  return <div ref={host} className={className} aria-hidden />;
}
