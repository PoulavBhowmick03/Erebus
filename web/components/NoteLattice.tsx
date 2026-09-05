"use client";

import { useEffect, useRef } from "react";
import * as THREE from "three";

/**
 * The anonymity set, drawn as what it actually is.
 *
 * STRK20 is a shared note-based pool: every shielded position in it is a note.
 * This is a lattice of them. Seven are cinnabar, because a settlement on wire v3
 * always creates seven notes and that count is public (privacy-model.md, step 5).
 * The rest are the set they hide in.
 *
 * Deliberately flat: an orthographic camera, unlit constant-size marks, no
 * lighting model, no bloom, no post-processing. It is a diagram that happens to
 * be rendered in WebGL, not an atmosphere.
 */
export function NoteLattice({
  variant = "light",
  className = "",
  density = 14,
  highlight = true,
}: {
  variant?: "light" | "void";
  className?: string;
  density?: number;
  highlight?: boolean;
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
    renderer.domElement.style.display = "block";
    renderer.domElement.style.width = "100%";
    renderer.domElement.style.height = "100%";

    const scene = new THREE.Scene();
    const group = new THREE.Group();
    scene.add(group);

    const frustum = 2.15;
    const camZ = 10;
    const camera = new THREE.OrthographicCamera(-frustum, frustum, frustum, -frustum, 0.1, 40);
    camera.position.set(0, 0, camZ);

    // ── a jittered cubic lattice. regular enough to read as a structure,
    //    irregular enough to avoid a screen-door moiré.
    const count = n * n * n;
    const offsets = new Float32Array(count * 3);
    const tint = new Float32Array(count);
    const seed = new Float32Array(count);

    const span = 3.2;
    let i = 0;
    for (let x = 0; x < n; x += 1) {
      for (let y = 0; y < n; y += 1) {
        for (let z = 0; z < n; z += 1) {
          const j = (v: number) => (v / (n - 1) - 0.5) * span + (Math.random() - 0.5) * (span / n) * 0.7;
          offsets[i * 3] = j(x);
          offsets[i * 3 + 1] = j(y);
          offsets[i * 3 + 2] = j(z);
          tint[i] = 0;
          seed[i] = Math.random();
          i += 1;
        }
      }
    }

    // seven notes. the settlement's own.
    if (highlight) {
      const chosen = new Set<number>();
      while (chosen.size < 7) chosen.add(Math.floor(Math.random() * count));
      chosen.forEach((k) => (tint[k] = 1));
    }

    const quad = new THREE.PlaneGeometry(1, 1);
    const geo = new THREE.InstancedBufferGeometry();
    geo.index = quad.index;
    geo.attributes.position = quad.attributes.position;
    geo.setAttribute("offset", new THREE.InstancedBufferAttribute(offsets, 3));
    geo.setAttribute("tint", new THREE.InstancedBufferAttribute(tint, 1));
    geo.setAttribute("seed", new THREE.InstancedBufferAttribute(seed, 1));
    geo.instanceCount = count;

    const isVoid = variant === "void";

    const material = new THREE.ShaderMaterial({
      transparent: true,
      depthWrite: false,
      uniforms: {
        uSize: { value: small ? 0.019 : 0.016 },
        uTime: { value: 0 },
        uBase: { value: new THREE.Color(isVoid ? 0xd8d4cb : 0x0b0b0c) },
        uLeak: { value: new THREE.Color(isVoid ? 0xe2492f : 0xc0301a) },
        uOpacity: { value: isVoid ? 0.62 : 0.55 },
        uCamZ: { value: camZ },
      },
      vertexShader: /* glsl */ `
        attribute vec3 offset;
        attribute float tint;
        attribute float seed;
        uniform float uSize;
        uniform float uTime;
        uniform float uCamZ;
        varying float vTint;
        varying float vFade;

        void main() {
          vTint = tint;
          vec3 p = offset;
          // a slow, tiny drift. notes are not still, but they are not swimming either.
          p.y += sin(uTime * 0.18 + seed * 6.2831) * 0.012;
          vec4 mv = modelViewMatrix * vec4(p, 1.0);
          float s = uSize * (1.0 + tint * 2.2);
          mv.xy += position.xy * s;
          vFade = smoothstep(-1.75, 1.75, mv.z + uCamZ);
          gl_Position = projectionMatrix * mv;
        }
      `,
      fragmentShader: /* glsl */ `
        uniform vec3 uBase;
        uniform vec3 uLeak;
        uniform float uOpacity;
        varying float vTint;
        varying float vFade;

        void main() {
          vec3 c = mix(uBase, uLeak, vTint);
          float a = uOpacity * mix(0.28, 1.0, vFade);
          a = mix(a, 1.0, vTint);
          gl_FragColor = vec4(c, a);
        }
      `,
    });

    const mesh = new THREE.Mesh(geo, material);
    mesh.frustumCulled = false;
    group.add(mesh);

    const resize = () => {
      const { clientWidth: w, clientHeight: h } = el;
      if (!w || !h) return;
      const aspect = w / h;
      camera.left = -frustum * aspect;
      camera.right = frustum * aspect;
      camera.updateProjectionMatrix();
      renderer.setSize(w, h, false);
    };
    resize();
    const ro = new ResizeObserver(resize);
    ro.observe(el);

    group.rotation.set(-0.32, 0.5, 0);

    let raf = 0;
    let visible = true;
    const io = new IntersectionObserver((e) => (visible = !!e[0]?.isIntersecting));
    io.observe(el);

    const clock = new THREE.Clock();
    const loop = () => {
      raf = requestAnimationFrame(loop);
      if (!visible) return;
      const t = clock.getElapsedTime();
      material.uniforms.uTime.value = t;
      group.rotation.y = 0.5 + t * 0.045;
      group.rotation.x = -0.32 + Math.sin(t * 0.11) * 0.06;
      renderer.render(scene, camera);
    };

    if (reduced) {
      renderer.render(scene, camera);
    } else {
      loop();
    }

    return () => {
      cancelAnimationFrame(raf);
      io.disconnect();
      ro.disconnect();
      geo.dispose();
      quad.dispose();
      material.dispose();
      renderer.dispose();
      el.removeChild(renderer.domElement);
    };
  }, [variant, density, highlight]);

  return <div ref={host} className={className} aria-hidden />;
}
