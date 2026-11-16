import * as THREE from './vendor/three.module.min.js';

// Z-up metres, +Y forward, and horizontal FOV match ptz-scene::render::Camera.
THREE.Object3D.DEFAULT_UP.set(0, 0, 1);
const $ = (id) => document.getElementById(id);
const frames = {
  home: { eye: [0, 0, 1.2], target: [0, 12, 1.2], hfov: 60 },
  lectern: { eye: [0, 0, 1.2], target: [5.3, 12.45, 1.8], hfov: 24 },
  overview: { eye: [15, -11, 11], target: [0, 8, 1.8], hfov: 60 },
  rig: { eye: [3, 3, 2.7], target: [0, 0, 1.65], hfov: 50 },
};
const state = { view: 'home', geometry: 'detailed', lighting: 'studio', custom: false };
const stats = { frames: 0, time: 0, samples: [], submittedTriangles: 0 };
const scene = new THREE.Scene();
scene.background = new THREE.Color('#0f0f0f');
const camera = new THREE.PerspectiveCamera(40, 16 / 9, 0.05, 250);
let renderer;
try {
  renderer = new THREE.WebGLRenderer({ canvas: $('candidate-canvas'), antialias: true, alpha: false });
} catch (error) {
  $('error').hidden = false;
  $('error').textContent = 'WebGL could not start in this browser. The CPU reference remains available. ' + error.message;
  throw error;
}
renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 1.5));
renderer.shadowMap.enabled = true;
renderer.shadowMap.type = THREE.PCFSoftShadowMap;
renderer.shadowMap.autoUpdate = false;
renderer.outputColorSpace = THREE.SRGBColorSpace;
const vector = (p) => new THREE.Vector3(...p);
const skyColor = new THREE.Color('#0f0f0f');
let model = new THREE.Group();
let geometryTriangles = 0;
let stageBaseline;
let rigBaseline;

// This shader uses the CPU renderer's existing lighting equation. It is a useful
// control for the richer material experiment, not a replacement CPU benchmark.
function simpleMaterial(rgb) {
  return new THREE.ShaderMaterial({
    uniforms: { baseColor: { value: new THREE.Vector3(...rgb.map((c) => c / 255)) } },
    side: THREE.DoubleSide,
    vertexShader: `varying vec3 n; varying vec3 world; varying vec3 eye;
      void main() { vec4 p=modelMatrix*vec4(position,1.0); world=p.xyz;
      vec3 vn=normalMatrix*normal;
      n=normalize(vec3(dot(viewMatrix[0].xyz,vn),dot(viewMatrix[1].xyz,vn),dot(viewMatrix[2].xyz,vn))); eye=cameraPosition;
      gl_Position=projectionMatrix*viewMatrix*p; }`,
    fragmentShader: `uniform vec3 baseColor; varying vec3 n; varying vec3 world; varying vec3 eye;
      void main() { vec3 normal=normalize(n); if(dot(normal,eye-world)<0.0) normal=-normal;
      float light=.38+.42*max(dot(normal,normalize(vec3(.35,-.5,.8))),0.0)+.20*(.5+.5*normal.z);
      float fog=.6*clamp((length(eye-world)-18.0)/142.0,0.0,1.0);
      gl_FragColor=vec4(mix(baseColor*light,vec3(15.0/255.0),fog),1.0); }`,
    toneMapped: false,
  });
}

function material(rgb, options = {}) {
  if (state.lighting === 'simple') return simpleMaterial(rgb);
  return new THREE.MeshStandardMaterial({
    color: new THREE.Color(`rgb(${rgb.join(',')})`), roughness: 0.74, metalness: 0.05,
    side: THREE.DoubleSide, ...options,
  });
}
function mesh(geometry, rgb, group = model, options = {}) {
  const object = new THREE.Mesh(geometry, material(rgb, options));
  object.castShadow = true;
  object.receiveShadow = true;
  group.add(object);
  geometryTriangles += (geometry.index ? geometry.index.count : geometry.attributes.position.count) / 3;
  return object;
}
function box(min, max, color, group = model, options) {
  const size = min.map((v, i) => max[i] - v);
  const object = mesh(new THREE.BoxGeometry(...size), color, group, options);
  object.position.set(...min.map((v, i) => (max[i] + v) / 2));
  return object;
}
function ellipsoid(at, radii, color, group = model, options) {
  const object = mesh(new THREE.SphereGeometry(1, 20, 12), color, group, options);
  object.position.set(...at);
  object.scale.set(...radii);
  return object;
}
function cylinder(from, to, radii, color, group = model, options) {
  const delta = vector(to).sub(vector(from));
  const object = mesh(new THREE.CylinderGeometry(radii[1], radii[0], delta.length(), 24), color, group, options);
  object.position.copy(vector(from).add(vector(to)).multiplyScalar(0.5));
  object.quaternion.setFromUnitVectors(new THREE.Vector3(0, 1, 0), delta.normalize());
  return object;
}
function person(x, y, color) {
  const at = (dx, dy, z) => [x + dx, y + dy, 0.5 + z];
  const dark = color.map((c) => Math.round(c * 0.52));
  const skin = [205, 181, 162];
  ellipsoid(at(0, 0, 1.16), [.23, .13, .30], color);
  ellipsoid(at(0, 0, .88), [.18, .12, .14], dark);
  cylinder(at(0, 0, 1.37), at(0, 0, 1.50), [.055, .055], skin);
  ellipsoid(at(0, 0, 1.61), [.12, .11, .15], skin);
  for (const sign of [-1, 1]) {
    cylinder(at(sign * .10, 0, .83), at(sign * .13, 0, .10), [.075, .055], dark);
    ellipsoid(at(sign * .13, -.045, .065), [.075, .14, .065], [39, 43, 48]);
    cylinder(at(sign * .20, 0, 1.34), at(sign * .29, -.015, .91), [.065, .045], color);
    ellipsoid(at(sign * .29, -.015, .86), [.045, .04, .07], skin);
  }
}
function stage() {
  for (let i = 0; i < 14; i++) {
    const y = -5 + i;
    box([-14, y, -.025], [14, y + 1, 0], i % 2 === 0 ? [44, 44, 44] : [36, 36, 36]);
  }
  box([-10, 9, 0], [10, 16, .5], [101, 94, 85]);
  box([-10, 8.975, 0], [10, 9, .46], [38, 41, 46]);
  box([-10, 8.96, .46], [10, 9, .5], [164, 156, 139]);
  for (let step = 0; step < 3; step++) {
    const rise = (step + 1) * .5 / 3;
    const front = 9 - (3 - step) * .32;
    box([7.8, front, 0], [9.2, 9, rise], [105, 102, 97]);
    box([7.8, front - .01, rise - .02], [9.2, front + .04, rise], [181, 176, 163]);
  }
  box([-10, 16, 0], [10, 16.2, 7], [54, 59, 67]);
  box([-4.12, 15.90, 2.48], [4.12, 16, 6.32], [24, 27, 33]);
  box([-4, 15.88, 2.6], [4, 15.9, 6.2], [157, 170, 185], model, { emissive: 0x344654, emissiveIntensity: .45 }).receiveShadow = false;
  for (let i = -4; i <= 4; i++) box([i * 2.5 - .03, 15.97, .5], [i * 2.5 + .03, 16, 7], [84, 84, 84]);
  for (const dx of [-.55, .55]) {
    const x = -3.9 + dx;
    box([x - .3, 11.7, .5], [x + .3, 12.3, .51], [210, 210, 210]);
    person(x, 12, [168, 172, 180]);
  }
  person(5.3, 12.45, [186, 164, 150]);
  box([4.92, 11.96, .5], [5.68, 12.3, 1.65], [112, 112, 112]);
  box([4.88, 11.92, 1.65], [5.72, 12.34, 1.70], [168, 168, 168]);
}
function ptz(x, y, attachment, inverted, yaw) {
  const sign = inverted ? -1 : 1;
  const lens = attachment + sign * 1.2;
  const color = [114, 184, 158];
  cylinder([x, y, attachment], [x, y, attachment + sign * .06], [.30, .27], [53, 58, 66]);
  cylinder([x, y, attachment + sign * .06], [x, y, lens - sign * .30], [.055, .055], [121, 128, 139], model, { metalness: .7, roughness: .28 });
  const head = new THREE.Group();
  model.add(head);
  head.position.set(x, y, lens);
  head.rotation.z = yaw * Math.PI / 180;
  if (inverted) head.rotateY(Math.PI);
  cylinder([0, 0, -.30], [0, 0, -.23], [.18, .20], [39, 43, 50], head);
  cylinder([0, 0, -.235], [0, 0, -.215], [.20, .20], color, head);
  box([-.25, -.10, -.22], [.25, .10, -.16], [47, 52, 61], head);
  for (const side of [-1, 1]) {
    const cx = side * .22;
    box([cx - .03, -.10, -.18], [cx + .03, .10, .025], [47, 52, 61], head);
    cylinder([side * .18, 0, 0], [side * .265, 0, 0], [.075, .075], color, head);
  }
  ellipsoid([0, -.025, 0], [.185, .21, .16], [65, 72, 83], head, { metalness: .2, roughness: .4 });
  cylinder([0, .10, 0], [0, .30, 0], [.105, .115], [31, 35, 42], head);
  cylinder([0, .30, 0], [0, .335, 0], [.115, .115], [130, 141, 154], head, { metalness: .8, roughness: .22 });
  cylinder([0, .335, 0], [0, .35, 0], [.098, .098], [17, 23, 31], head);
  cylinder([0, .351, 0], [0, .355, 0], [.082, .082], [32, 71, 96], head, { metalness: .65, roughness: .12 });
  cylinder([-.025, .356, .03], [-.025, .357, .03], [.017, .017], [114, 161, 177], head);
  box([-.025, .035, .145], [.025, .07, .165], color, head);
}

function original(data) {
  // Preserve the original triangles and face colours exported alongside the CPU captures.
  const groups = new Map();
  data.triangles.forEach((triangle, i) => {
    const key = data.colors[i].join(',');
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(...triangle.flatMap((index) => data.positions[index]));
  });
  for (const [color, positions] of groups) {
    const geometry = new THREE.BufferGeometry();
    geometry.setAttribute('position', new THREE.Float32BufferAttribute(positions, 3));
    geometry.computeVertexNormals();
    mesh(geometry, color.split(',').map(Number));
  }
}

const lights = new THREE.Group();
const ambient = new THREE.HemisphereLight(0xd8e5f1, 0x34302a, 1.3);
ambient.position.set(0, 0, 10);
lights.add(ambient);
const key = new THREE.DirectionalLight(0xfff2dc, 3.0);
key.position.set(-4, 3, 12);
key.target.position.set(0, 11, 0);
key.castShadow = true;
key.shadow.mapSize.set(2048, 2048);
Object.assign(key.shadow.camera, { left: -18, right: 18, top: 18, bottom: -18, near: .5, far: 50 });
key.shadow.bias = -.0002;
key.shadow.normalBias = .035;
lights.add(key, key.target);
const fill = new THREE.DirectionalLight(0xbed9e7, .65);
fill.position.set(8, 8, 6);
fill.target.position.set(0, 12, 1);
lights.add(fill, fill.target);
scene.add(lights);

function release(group) {
  group.traverse((object) => {
    object.geometry?.dispose();
    if (Array.isArray(object.material)) object.material.forEach((m) => m.dispose());
    else object.material?.dispose();
  });
}
function rebuild() {
  scene.remove(model);
  release(model);
  model = new THREE.Group();
  geometryTriangles = 0;
  const rig = state.view === 'rig';
  if (state.geometry === 'simple') original(rig ? rigBaseline : stageBaseline);
  else if (rig) {
    box([-4, -4, -.05], [4, 4, 0], [52, 56, 62]);
    ptz(-.55, 0, 0, false, 0);
    ptz(.7, .25, 3.3, true, -20);
  } else stage();
  scene.add(model);
  lights.visible = state.lighting === 'studio';
  renderer.shadowMap.enabled = lights.visible;
  renderer.toneMapping = lights.visible ? THREE.ACESFilmicToneMapping : THREE.NoToneMapping;
  renderer.toneMappingExposure = 1.05;
  scene.fog = lights.visible ? new THREE.Fog(skyColor, 35, 150) : null;
  key.position.set(...(rig ? [-3, 4, 6] : [-4, 3, 12]));
  key.target.position.set(...(rig ? [0, 0, 1] : [0, 11, 0]));
  Object.assign(key.shadow.camera, { left: rig ? -5 : -18, right: rig ? 5 : 18, top: rig ? 5 : 18, bottom: rig ? -5 : -18 });
  key.shadow.camera.updateProjectionMatrix();
  renderer.shadowMap.needsUpdate = true;
  $('triangles').textContent = Math.round(geometryTriangles).toLocaleString() + ' triangles';
  $('candidate-tag').textContent = `${state.geometry === 'detailed' ? 'Rounded' : 'Blocks'} + ${lights.visible ? 'materials' : 'simple'}`;
  stats.samples = [];
}
function resetView() {
  const frame = frames[state.view];
  camera.position.set(...frame.eye);
  camera.fov = 2 * Math.atan(Math.tan(frame.hfov * Math.PI / 360) / (16 / 9)) * 180 / Math.PI;
  camera.lookAt(...frame.target);
  camera.updateProjectionMatrix();
  state.custom = false;
  $('framing').textContent = `${frame.hfov}° HFOV · [${frame.eye.join(', ')}] m`;
  $('interaction').textContent = 'Drag candidate to orbit · reset to match';
  $('view-label').textContent = $('view').selectedOptions[0].textContent;
  $('candidate-caption').textContent = 'Same scene dimensions, subject marks, camera position and field of view.';
}
function chooseView() {
  state.view = $('view').value;
  $('cpu-image').src = `cpu-${state.view}.png`;
  $('reference-caption').textContent = state.view === 'rig'
    ? 'The current CPU renderer drawing the existing block camera geometry in a matching test scene.'
    : 'An actual still from the current Rust renderer, at exactly the same lens and position.';
  rebuild();
  resetView();
}
function resize() {
  const rect = $('candidate').getBoundingClientRect();
  renderer.setSize(Math.max(1, Math.round(rect.width)), Math.max(1, Math.round(rect.height)), false);
}
let drag;
$('candidate-canvas').addEventListener('pointerdown', (event) => {
  if (event.button !== 0) return;
  drag = { x: event.clientX, y: event.clientY };
  event.currentTarget.setPointerCapture(event.pointerId);
});
$('candidate-canvas').addEventListener('pointermove', (event) => {
  if (!drag) return;
  const target = vector(frames[state.view].target);
  const offset = camera.position.clone().sub(target);
  let azimuth = Math.atan2(offset.y, offset.x);
  let altitude = Math.asin(offset.z / offset.length());
  azimuth -= (event.clientX - drag.x) * .005;
  altitude = THREE.MathUtils.clamp(altitude + (event.clientY - drag.y) * .005, -.1, Math.PI / 2 - .05);
  const radius = offset.length();
  camera.position.copy(target).add(new THREE.Vector3(Math.cos(azimuth) * Math.cos(altitude), Math.sin(azimuth) * Math.cos(altitude), Math.sin(altitude)).multiplyScalar(radius));
  camera.lookAt(target);
  drag = { x: event.clientX, y: event.clientY };
  state.custom = true;
  $('view-label').textContent = 'Custom orbit · reference stays fixed';
  $('framing').textContent = 'Custom candidate view';
  $('candidate-caption').textContent = 'Candidate camera moved. Reset view restores the framing of the CPU reference.';
});
for (const event of ['pointerup', 'pointercancel', 'lostpointercapture']) $('candidate-canvas').addEventListener(event, () => { drag = null; });
$('reset').addEventListener('click', resetView);
$('view').addEventListener('change', chooseView);
document.querySelectorAll('input[type=radio]').forEach((input) => input.addEventListener('change', () => {
  state[input.name] = input.value;
  rebuild();
}));
new ResizeObserver(resize).observe($('candidate'));
try {
  [stageBaseline, rigBaseline] = await Promise.all(['stage-baseline.json', 'rig-baseline.json'].map(async (url) => {
    const response = await fetch(url);
    if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
    return response.json();
  }));
  chooseView();
  resize();
} catch (error) {
  $('error').hidden = false;
  $('error').textContent = 'The comparison assets could not be loaded. Run the local server described in README.md. ' + error.message;
  throw error;
}

let previous = performance.now();
function animate(now) {
  const interval = now - previous;
  previous = now;
  if (interval > 0 && interval < 1000) stats.samples.push(interval);
  if (stats.samples.length > 90) stats.samples.shift();
  renderer.render(scene, camera);
  stats.frames++;
  stats.submittedTriangles = renderer.info.render.triangles;
  if (now - stats.time > 750 && stats.samples.length) {
    const ordered = [...stats.samples].sort((a, b) => a - b);
    const median = ordered[Math.floor(ordered.length / 2)];
    $('timing').textContent = `${median.toFixed(1)} ms / frame · browser`;
    stats.time = now;
  }
  requestAnimationFrame(animate);
}
requestAnimationFrame(animate);
// Read-only state for the local smoke test; timings are animation cadence, not GPU render duration.
window.renderStudy = { state, stats, get triangles() { return geometryTriangles; }, revision: THREE.REVISION };
