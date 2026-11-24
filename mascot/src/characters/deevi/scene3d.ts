// Deevi in 3D: the logo mark as a three.js character. Ported from the handoff `duowave-mascot.js` (three.js r128, global
// THREE, 23.11.2026) to the three package as ES modules, so a bundler loads it as its own chunk, only where 3D is used.
//
// What changed in the port, and why the picture stays the same:
// - Colours. r128 took hex colours of materials and lights as they were (linear); colours from CSS went through
//   convertSRGBToLinear. Since r152 three converts every colour from sRGB by itself. So CSS colours are set plainly,
//   and the few hex colours r128 used raw go through `raw()`, which keeps the old values — the eyes, cheeks and lights
//   look as in the handoff.
// - Lights. r155 dropped the «legacy» light units, in which three multiplied light by π. The intensities here are the
//   handoff's times π.
// - The motion (poses, springs, blinking, the wink) moved to rig.ts, shared with the flat body; this file only builds
//   the scene and draws the rig's state. Time, the pointer, pausing off screen and sound are the engine's
//   (engine/mount.ts), the same for every character.
// - Dragging sideways turns him; the engine lets the page scroll vertically over him (`touch-action: pan-y`), so a big
//   Deevi on a phone doesn't trap the scroll.
// - A lost WebGL context hands over to the flat body (BodyHost.lost).
// - The bubble (handoff 1.2.0, 24.11.2026): a clear shell, back and front layer, with an iridescent rim and a window
//   glare, and three balls of light inside; the eyes and the mouth are opaque and sit on the shell. The shell's shader
//   patch is the handoff's, put where three 0.187 has the same values (before tone mapping and the sRGB output). The
//   balls' colours and drift are shared with the flat body (bubble.ts, rig.ts `innerOf`). Draw order: back wall, the
//   light, front wall, cheeks.
// - The canvas reaches past the box (`resize`): the waves are wider than a narrow box, and a canvas the size of the
//   box cut them at its edges, where the flat body's SVG simply overflows. The camera renders a wider view around
//   the same frustum, so Deevi stays exactly where he was in the box.

import {
    BackSide,
    BufferAttribute,
    BufferGeometry,
    CanvasTexture,
    CircleGeometry,
    Color,
    DirectionalLight,
    DoubleSide,
    FrontSide,
    Group,
    HemisphereLight,
    LinearSRGBColorSpace,
    type Material,
    Mesh,
    MeshBasicMaterial,
    MeshPhysicalMaterial,
    MeshStandardMaterial,
    PerspectiveCamera,
    PlaneGeometry,
    SRGBColorSpace,
    Scene,
    ShaderMaterial,
    Shape,
    ShapeGeometry,
    SphereGeometry,
    TorusGeometry,
    type WebGLProgramParametersWithUniforms,
    WebGLRenderer,
} from 'three';

import type { Body, BodyHost } from '../../engine/types.ts';
import { type RGB, bubbleOf, mixLinear } from './bubble.ts';
import { CHEEK, DEEVI_COLORS } from './looks.ts';
import { type DeeviState, dotRadius, eyeOf, innerOf, mouthOf, ringOf, shadowOf } from './rig.ts';

/** A hex colour taken as it is, the way r128 read material and light colours (see the top of the file). */
const raw = (hex: number): Color =>
    new Color().setRGB(((hex >> 16) & 255) / 255, ((hex >> 8) & 255) / 255, (hex & 255) / 255, LinearSRGBColorSpace);
const INK = parseInt(DEEVI_COLORS.ink.slice(1), 16);

/** Legacy light units multiplied light by π; the handoff's intensities are kept times π. */
const PI = Math.PI;

/** The shell's look (the handoff's patch): fresnel rim with a rainbow, the specular light, two window glares. The back
 * wall is a faint tint; the front one is clear in the middle and pale toward the rim. */
const shellShader = (shader: WebGLProgramParametersWithUniforms): void => {
    shader.fragmentShader = shader.fragmentShader.replace(
        '#include <tonemapping_fragment>',
        [
            'vec3 shN = normalize(normal), shV = normalize(vViewPosition);',
            'float shF = pow(1.0 - clamp(abs(dot(shN, shV)), 0.0, 1.0), 2.0);',
            'vec3 shS = reflectedLight.directSpecular + reflectedLight.indirectSpecular;',
            'vec3 shI = 0.5 + 0.5 * cos(6.2832 * (shF * 1.3 + vec3(0.0, 0.33, 0.67)));',
            'float shG = smoothstep(0.8, 0.99, dot(shN, normalize(vec3(-0.5, 0.62, 0.6)))) * 0.75;',
            'float shG2 = smoothstep(0.93, 0.995, dot(shN, normalize(vec3(0.55, -0.45, 0.7)))) * 0.35;',
            '#ifdef SHELL_BACK',
            'gl_FragColor = vec4(gl_FragColor.rgb + shI * 0.15, 0.05 + 0.22 * shF);',
            '#else',
            'gl_FragColor.rgb += shI * shF * 0.45 + vec3(shG + shG2);',
            'gl_FragColor.a = clamp(0.12 + 0.72 * shF + dot(shS, vec3(0.3, 0.6, 0.1)) * 1.6 + 0.8 * (shG + shG2), 0.0, 1.0);',
            '#endif',
            '#include <tonemapping_fragment>',
        ].join('\n'),
    );
};

const shellMaterial = (back: boolean): MeshPhysicalMaterial => {
    const m = new MeshPhysicalMaterial({
        color: 0xffffff,
        roughness: 0.12,
        metalness: 0,
        clearcoat: 1,
        clearcoatRoughness: 0.05,
        transparent: true,
        premultipliedAlpha: true,
        depthWrite: false,
        side: back ? BackSide : FrontSide,
    });
    if (back) m.defines = { ...m.defines, SHELL_BACK: '' };
    m.onBeforeCompile = shellShader;
    return m;
};

/** A ball of light: brightest in the middle, fading to nothing at its edge. It writes its colour as given (the sRGB
 * numbers, as the handoff's r128 shader did), premultiplied. */
const INNER_VERTEX =
    'varying vec3 vN; varying vec3 vV; void main() { vec4 mv = modelViewMatrix * vec4(position, 1.0); vN = normalMatrix * normal; vV = -mv.xyz; gl_Position = projectionMatrix * mv; }';
const INNER_FRAGMENT =
    'uniform vec3 uColor; uniform float uAlpha; varying vec3 vN; varying vec3 vV; void main() { float d = clamp(dot(normalize(vN), normalize(vV)), 0.0, 1.0); float a = pow(d, 1.1) * uAlpha; gl_FragColor = vec4(uColor * a, a); }';

/** Throws when WebGL is not available: the engine keeps the flat body then. */
export function create3d(host: BodyHost): Body<DeeviState> {
    const { container } = host;
    const canvas = document.createElement('canvas');
    // Placed and sized by `resize`, past the box's edges; the box itself takes the pointer.
    canvas.style.cssText = 'position:absolute;display:block;pointer-events:none';
    canvas.setAttribute('aria-hidden', 'true');
    // The renderer throws here when WebGL is missing; nothing is in the container yet.
    const renderer = new WebGLRenderer({ canvas, antialias: true, alpha: true });
    container.insertBefore(canvas, container.firstChild);

    // ---------- scene ----------
    const scene = new Scene();
    const camera = new PerspectiveCamera(30, 1, 0.1, 50);
    camera.position.set(0, 1.55, 9.4);
    camera.lookAt(0, 1.2, 0);

    scene.add(new HemisphereLight(raw(0xffffff), raw(0x8fae62), 1.0 * PI));
    const key = new DirectionalLight(raw(0xffffff), 0.9 * PI);
    key.position.set(-3, 5, 6);
    scene.add(key);
    const rim = new DirectionalLight(raw(0xffffff), 0.35 * PI);
    rim.position.set(4, 2, -4);
    scene.add(rim);

    // Body: the logo silhouette (160 x 176, corner radius 68, in units of 80) as a sphere-swept box.
    const B = 1.1;
    const R = 0.85;
    const HX = 1 - R;
    const HY = B - R;
    /** Front surface height at (x, y). */
    const surfZ = (x: number, y: number): number => {
        const qx = Math.max(Math.abs(x) - HX, 0);
        const qy = Math.max(Math.abs(y) - HY, 0);
        return Math.sqrt(Math.max(0, R * R - qx * qx - qy * qy));
    };
    const sgn = (v: number): number => (Math.abs(v) < 1e-6 ? 0 : v > 0 ? 1 : -1);
    const bodyGeo = new SphereGeometry(1, 96, 72);
    {
        const p = bodyGeo.attributes.position;
        for (let i = 0; i < p.count; i++) {
            const x = p.getX(i);
            const y = p.getY(i);
            const z = p.getZ(i);
            p.setXYZ(i, HX * sgn(x) + R * x, HY * sgn(y) + R * y, R * z); // sphere normals stay valid
        }
    }
    const shellFront = shellMaterial(false);
    const shellBack = shellMaterial(true);
    const rig = new Group(); // jump + squash, pivot on the ground
    scene.add(rig);
    const head = new Group(); // rotation, pivot in the centre
    head.position.y = B;
    rig.add(head);
    const back = new Mesh(bodyGeo, shellBack);
    back.renderOrder = 1;
    const front = new Mesh(bodyGeo, shellFront);
    front.renderOrder = 3;
    head.add(back, front);

    // The light inside: three soft balls (bubble.ts), drawn between the walls.
    const innerGeo = new SphereGeometry(1, 48, 32);
    const inner = [0, 1, 2].map(() => {
        const uniforms = { uColor: { value: new Color(0xffffff) }, uAlpha: { value: 1 } };
        const m = new Mesh(
            innerGeo,
            new ShaderMaterial({
                uniforms,
                vertexShader: INNER_VERTEX,
                fragmentShader: INNER_FRAGMENT,
                transparent: true,
                premultipliedAlpha: true,
                depthWrite: false,
            }),
        );
        m.renderOrder = 2;
        head.add(m);
        return { m, uniforms };
    });

    const ink = new MeshStandardMaterial({ color: raw(INK), roughness: 0.25 });
    const face = new Group();
    head.add(face);
    const unit = new SphereGeometry(1, 32, 24);

    // Eyes: the logo's own rig. Each eye is a stroked quadratic curve with round caps (logo units / 80): a vertical bar
    // at rest, an arch when it smiles. Printed flat on the body.
    const EX = 0.375;
    const EY = 0.05;
    const K = 22;
    const M = 10;
    const W = 6;
    const RN = 3;
    const BAR = [0, 0.175, 0, 0, 0, -0.175, 0.42]; // x1,y1, cx,cy, x2,y2, stroke width
    const ARCH = [-0.2125, -0.075, 0, 0.2, 0.2125, -0.075, 0.245];
    const inkFlat = new MeshBasicMaterial({
        color: raw(INK),
        side: DoubleSide,
        polygonOffset: true,
        polygonOffsetFactor: -2,
        polygonOffsetUnits: -2,
    });
    interface EyeMesh {
        geo: BufferGeometry;
        side: number;
    }
    const makeEye = (side: number): EyeMesh => {
        const strip = K * (W + 1);
        const cap = 1 + RN * (M + 1);
        const count = strip + 2 * cap;
        const idx: number[] = [];
        for (let i = 0; i < K - 1; i++)
            for (let j = 0; j < W; j++) {
                const a = i * (W + 1) + j;
                const b = a + 1;
                const c = a + W + 1;
                const d = c + 1;
                idx.push(a, b, c, b, d, c);
            }
        for (const s0 of [strip, strip + cap]) {
            for (let j = 0; j < M; j++) idx.push(s0, s0 + 1 + j, s0 + 2 + j);
            for (let r = 0; r < RN - 1; r++)
                for (let j = 0; j < M; j++) {
                    const a = s0 + 1 + r * (M + 1) + j;
                    const b = a + 1;
                    const c = a + M + 1;
                    const d = c + 1;
                    idx.push(a, b, c, b, d, c);
                }
        }
        const geo = new BufferGeometry();
        geo.setAttribute('position', new BufferAttribute(new Float32Array(count * 3), 3));
        geo.setIndex(idx);
        const mesh = new Mesh(geo, inkFlat);
        mesh.frustumCulled = false;
        head.add(mesh);
        return { geo, side };
    };
    /** sh: [x1,y1,cx,cy,x2,y2,w]; sy squashes vertically (blink); rot tilts; (ox,oy) is the gaze offset. */
    const drawEye = (e: EyeMesh, sh: number[], sy: number, rot: number, ox: number, oy: number): void => {
        const pos = e.geo.attributes.position as BufferAttribute;
        let n = 0;
        const cs = Math.cos(rot);
        const sn = Math.sin(rot);
        const cx0 = e.side * EX + ox;
        const cy0 = EY + oy;
        const put = (u: number, v: number): void => {
            v *= sy;
            const x = cx0 + u * cs - v * sn;
            const y = cy0 + u * sn + v * cs;
            pos.setXYZ(n++, x, y, surfZ(x, y) + 0.004);
        };
        const hw = sh[6] / 2;
        const ends: number[][] = [];
        for (let i = 0; i < K; i++) {
            const t = i / (K - 1);
            const a = (1 - t) * (1 - t);
            const b = 2 * t * (1 - t);
            const c = t * t;
            const px = a * sh[0] + b * sh[2] + c * sh[4];
            const py = a * sh[1] + b * sh[3] + c * sh[5];
            let tx = 2 * (1 - t) * (sh[2] - sh[0]) + 2 * t * (sh[4] - sh[2]);
            let ty = 2 * (1 - t) * (sh[3] - sh[1]) + 2 * t * (sh[5] - sh[3]);
            const l = Math.hypot(tx, ty) || 1;
            tx /= l;
            ty /= l;
            for (let j = 0; j <= W; j++) {
                const k = ((j / W) * 2 - 1) * hw;
                put(px - ty * k, py + tx * k);
            }
            if (i === 0 || i === K - 1) ends.push([px, py, tx, ty]);
        }
        ends.forEach((q, which) => {
            const dir = which === 0 ? -1 : 1;
            put(q[0], q[1]);
            for (let r = 1; r <= RN; r++)
                for (let j = 0; j <= M; j++) {
                    const ang = (Math.PI * j) / M;
                    const co = Math.cos(ang);
                    const si = Math.sin(ang);
                    const rr = (hw * r) / RN;
                    put(q[0] + (-q[3] * co + dir * q[2] * si) * rr, q[1] + (q[2] * co + dir * q[3] * si) * rr);
                }
        });
        pos.needsUpdate = true;
    };
    const eyes = [makeEye(-1), makeEye(1)];
    const shape = [0, 0, 0, 0, 0, 0, 0];

    // Cheeks (visible only when blush > 0)
    const cheekMat = new MeshBasicMaterial({ color: raw(parseInt(CHEEK.slice(1), 16)), transparent: true, opacity: 0, depthWrite: false });
    for (const s of [-1, 1]) {
        const c = new Mesh(new CircleGeometry(0.15, 32), cheekMat);
        c.position.set(s * 0.6, -0.27, surfZ(0.6, -0.27) + 0.012);
        c.rotation.y = s * 0.56;
        c.scale.y = 0.62;
        c.renderOrder = 4;
        face.add(c);
    }

    // Mouths: hidden at rest (the logo has none); shown only when the mood needs one.
    const MY = -0.47;
    const MZ = surfZ(0, MY);
    const frown = new Mesh(new TorusGeometry(0.13, 0.04, 12, 32, Math.PI), ink);
    frown.position.set(0, MY - 0.08, MZ);
    const oh = new Mesh(unit, ink);
    oh.position.set(0, MY, MZ);
    face.add(frown, oh);

    // Effects: the thinking dots and the wink sparkles are the effect colour (setEffectColor); the sound waves are the
    // mood's colour nudged 15 % toward it, so they read on light and dark pages alike.
    const fxColor = raw(INK);
    let fxRgb: RGB = [0, 0, 0];
    const dots: Mesh<SphereGeometry, MeshBasicMaterial>[] = [];
    for (let d = 0; d < 3; d++) {
        const dm = new Mesh(unit, new MeshBasicMaterial({ color: fxColor }));
        dm.position.set(0.55 + d * 0.27, B + 0.32 + d * 0.16, 0.2);
        head.add(dm);
        dots.push(dm);
    }
    const starShape = new Shape();
    for (let sp = 0; sp < 8; sp++) {
        const sr = sp % 2 ? 0.3 : 1;
        const sa = (sp * Math.PI) / 4;
        if (sp === 0) starShape.moveTo(Math.sin(sa) * sr, Math.cos(sa) * sr);
        else starShape.lineTo(Math.sin(sa) * sr, Math.cos(sa) * sr);
    }
    const starMat = new MeshBasicMaterial({ color: fxColor, side: DoubleSide });
    const stars = [
        [0.9, 0.5, 0.2, 0],
        [1.16, 0.2, 0.1, 0.12],
        [0.7, 0.86, 0.085, 0.22],
    ].map(([x, y, size, delay]) => {
        const m = new Mesh(new ShapeGeometry(starShape), starMat);
        m.scale.setScalar(0.0001);
        head.add(m);
        return { m, x, y, size, delay };
    });
    const rings: Mesh<TorusGeometry, MeshBasicMaterial>[] = [];
    for (let r = 0; r < 3; r++) {
        const rm = new Mesh(
            new TorusGeometry(1, 0.02, 8, 128),
            new MeshBasicMaterial({ color: fxColor, transparent: true, opacity: 0, depthWrite: false }),
        );
        rm.position.set(0, B, -0.4);
        scene.add(rm);
        rings.push(rm);
    }
    const setEffectColor = (css: string): void => {
        try {
            fxColor.set(css);
            for (const m of dots) m.material.color.copy(fxColor);
            starMat.color.copy(fxColor);
            const c = fxColor.clone().convertLinearToSRGB();
            fxRgb = [c.r, c.g, c.b];
        } catch {
            /* a colour three cannot read: keep the old one */
        }
    };

    // Ground shadow
    const sc = document.createElement('canvas');
    sc.width = sc.height = 128;
    const sx = sc.getContext('2d');
    if (sx) {
        const grd = sx.createRadialGradient(64, 64, 4, 64, 64, 64);
        grd.addColorStop(0, 'rgba(0,0,0,0.42)');
        grd.addColorStop(1, 'rgba(0,0,0,0)');
        sx.fillStyle = grd;
        sx.fillRect(0, 0, 128, 128);
    }
    const shadow = new Mesh(
        new PlaneGeometry(3.2, 3.2),
        new MeshBasicMaterial({ map: new CanvasTexture(sc), transparent: true, depthWrite: false }),
    );
    shadow.rotation.x = -Math.PI / 2;
    shadow.position.y = 0.01;
    scene.add(shadow);
    shadow.visible = host.shadow;

    // Turning: a body of its own view, not of the character's state. Dragging sideways turns him, and he turns back.
    let rotY = 0;
    let rotX = 0;
    const ease = (a: number, b: number, k: number, dt: number): number => a + (b - a) * (1 - Math.exp(-k * dt));
    const clamp = (v: number, limit: number): number => Math.max(-limit, Math.min(limit, v));

    const lost = (event: Event): void => {
        event.preventDefault();
        host.lost();
    };
    canvas.addEventListener('webglcontextlost', lost);

    return {
        resize(w, h) {
            // Room for the effects around the box: the widest wave reaches 0.47 of the box's height from his centre,
            // a hop lifts it by up to 0.08 more. So 0.15 of the height above and below, and at the sides enough for a
            // half-width of 0.52 of the height.
            const ey = Math.ceil(0.15 * h);
            const ex = Math.ceil(Math.max(0.15 * h, 0.52 * h - w / 2));
            const cw = w + 2 * ex;
            const ch = h + 2 * ey;
            renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
            renderer.setSize(cw, ch, false);
            Object.assign(canvas.style, { left: `${-ex}px`, top: `${-ey}px`, width: `${cw}px`, height: `${ch}px` });
            // The camera keeps the box's view and draws past it on every side: the body doesn't move.
            camera.aspect = w / h;
            camera.setViewOffset(w, h, -ex, -ey, cw, ch);
            camera.updateProjectionMatrix();
        },

        setEffectColor,

        draw(s, dt, pointer) {
            const p = s.pose;
            for (const st of stars) {
                const k = s.spark > 0 ? Math.max(0, Math.min(1, (s.spark - st.delay) / (1 - st.delay))) : 0;
                st.m.scale.setScalar(Math.sin(k * Math.PI) * st.size + 0.0001);
                st.m.position.set(st.x + k * 0.22, st.y + k * 0.2, 0.8);
                st.m.rotation.z = -k * 1.6;
            }
            const look = bubbleOf(s.color);
            shellFront.color.setRGB(look.shell[0], look.shell[1], look.shell[2], SRGBColorSpace);
            shellBack.color.copy(shellFront.color);
            inner.forEach((ball, i) => {
                const b = innerOf(s, i);
                const c = look.inner[i];
                ball.m.position.set(b.x, b.y, b.z);
                ball.m.scale.setScalar(b.r);
                ball.uniforms.uColor.value.setRGB(c[0], c[1], c[2], LinearSRGBColorSpace); // the numbers as they are
                ball.uniforms.uAlpha.value = b.alpha;
            });
            const wave = mixLinear([s.color[0] / 255, s.color[1] / 255, s.color[2] / 255], fxRgb, 0.15);
            rig.scale.set(1 + s.squash * 0.8, 1 - s.squash, 1 + s.squash * 0.8);
            rig.position.y = s.jump;

            const drag = pointer.drag;
            rotY = ease(rotY, drag ? clamp(drag.dx * 0.011, 1.1) : 0, drag ? 18 : 5, dt);
            rotX = ease(rotX, drag ? clamp(drag.dy * 0.008, 0.45) : 0, drag ? 18 : 5, dt);
            head.rotation.set(rotX - p.ly * 0.05, rotY + p.lx * 0.12 + s.pointerX * 0.1, s.lean);

            face.position.set(s.lookX, s.lookY, 0);
            for (const eye of eyes) {
                const e = eyeOf(s, eye.side > 0);
                for (let q = 0; q < 7; q++) shape[q] = BAR[q] + (ARCH[q] - BAR[q]) * e.arc;
                shape[6] *= e.width;
                drawEye(eye, shape, e.scaleY, e.tilt, s.lookX, s.lookY);
            }
            cheekMat.opacity = 0.5 * p.blush;

            frown.scale.setScalar(p.frown + 0.0001);
            const m = mouthOf(s);
            oh.scale.set(m.w + 0.0001, m.h + 0.0001, (p.oh + p.talk) * 0.03 + 0.0001);

            dots.forEach((dot, i) => dot.scale.setScalar(dotRadius(s, i) + 0.0001));
            rings.forEach((ring, i) => {
                const r = ringOf(s, i);
                ring.scale.set(r.radius, r.radius * 1.08, 1);
                ring.position.y = B + s.jump * 0.5;
                ring.material.opacity = r.opacity;
                ring.material.color.setRGB(wave[0], wave[1], wave[2], SRGBColorSpace);
            });

            const sh = shadowOf(s);
            shadow.material.opacity = 0.75 * sh.lift;
            shadow.scale.set(sh.spread, 0.55 * sh.spread, 1);

            renderer.render(scene, camera);
        },

        dispose() {
            canvas.removeEventListener('webglcontextlost', lost);
            scene.traverse((o) => {
                const mesh = o as Mesh;
                if (mesh.geometry) mesh.geometry.dispose();
                const material = mesh.material as (Material & { map?: { dispose(): void } | null }) | undefined;
                if (material) {
                    material.map?.dispose();
                    material.dispose();
                }
            });
            renderer.dispose();
            // Free the context now: a page has room for about 16, and the engine may swap bodies.
            renderer.forceContextLoss();
            canvas.remove();
        },
    };
}
