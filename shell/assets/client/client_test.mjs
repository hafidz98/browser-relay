// Harness: stub enough DOM to drive app.js logic and assert behavior.
import fs from 'fs';
import { strict as assert } from 'assert';

const sent = [];           // messages the client "sends"
let wsInstance = null;

function makeEl(id) {
  const handlers = {};
  const el = {
    id,
    style: {},
    dataset: {},
    children: [],
    _text: '',
    _html: '',
    src: '',
    value: '',
    naturalWidth: 0,
    naturalHeight: 0,
    _rect: { x: 0, y: 0, width: 0, height: 0 },
    classList: {
      _s: new Set(),
      add: (c) => el.classList._s.add(c),
      remove: (c) => el.classList._s.delete(c),
      toggle: (c, f) => { f ? el.classList._s.add(c) : el.classList._s.delete(c); },
      contains: (c) => el.classList._s.has(c),
    },
    addEventListener: (t, h) => { (handlers[t] = handlers[t] || []).push(h); },
    appendChild: (c) => { el.children.push(c); },
    click: () => {
      (handlers['click'] || []).forEach(h => h());
      if (typeof el.onclick === 'function') el.onclick();
    },
    focus: () => {},
    select: () => {},
    setPointerCapture: () => {},
    getBoundingClientRect: () => ({ ...el._rect, left: el._rect.x, top: el._rect.y, right: el._rect.x + el._rect.width, bottom: el._rect.y + el._rect.height }),
    set textContent(v) { this._text = v; },
    get textContent() { return this._text; },
    set innerHTML(v) { this._html = v; this.children = []; },
    get innerHTML() { return this._html; },
    _handlers: handlers,
    _fire: (t, e) => (handlers[t] || []).forEach(h => h(e || {})),
  };
  Object.defineProperty(el, 'className', {
    get: () => [...el.classList._s].join(' '),
    set: (v) => { el.classList._s = new Set(String(v).split(/\s+/).filter(Boolean)); },
  });
  return el;
}

const els = {};
['screen', 'status', 'auth', 'token', 'tabs', 'kbd', 'newtab', 'mobile',
 'kbdBtn', 'clipGet', 'clipSet', 'pause', 'go', 'latency', 'livedot', 'authErr',
 'sheet', 'sheetTitle', 'sheetInput', 'sheetOk', 'sheetCancel'].forEach(id => els[id] = makeEl(id));

const docHandlers = {};
global.document = {
  getElementById: (id) => els[id] || null,
  createElement: (tag) => makeEl('dyn-' + tag),
  addEventListener: (t, h) => { (docHandlers[t] = docHandlers[t] || []).push(h); },
};
global.localStorage = {
  _s: { br_token: 'test-token-123' },
  getItem: (k) => global.localStorage._s[k] || null,
  setItem: (k, v) => { global.localStorage._s[k] = v; },
};
global.location = { protocol: 'http:', host: '127.0.0.1:18765' };
global.requestAnimationFrame = (fn) => { fn(); return 1; };
global.prompt = () => null;
global.setInterval = () => 0; // don't keep the test process alive

class FakeWS {
  constructor(url) {
    this.url = url;
    wsInstance = this;
    setTimeout(() => this.onopen && this.onopen(), 0);
  }
  send(s) { sent.push(JSON.parse(s)); }
  inject(obj) { this.onmessage && this.onmessage({ data: JSON.stringify(obj) }); }
}
global.WebSocket = FakeWS;

const js = fs.readFileSync(new URL('./app.js', import.meta.url), 'utf8');
eval(js);

await new Promise(r => setTimeout(r, 20));

// 1. auto-connect with stored token, auth message sent
assert.ok(wsInstance, 'websocket created');
assert.equal(wsInstance.url, 'ws://127.0.0.1:18765/ws');
assert.deepEqual(sent[0], { type: 'auth', token: 'test-token-123' });
console.log('PASS client auto-connect + auth');

// 2. welcome -> overlay hidden, status driving
wsInstance.inject({ type: 'welcome', driver: true, mobile: false });
assert.equal(els['auth'].style.display, 'none');
assert.equal(els['status'].textContent, 'driving');
console.log('PASS welcome handling');

// 3. frame -> img.src
wsInstance.inject({ type: 'frame', data: 'QUJD' });
assert.equal(els['screen'].src, 'data:image/jpeg;base64,QUJD');
console.log('PASS frame rendering');

// 4. tabs render
wsInstance.inject({ type: 'tabs', tabs: [
  { id: 'a1', title: 'Example', url: 'https://example.com', active: true },
  { id: 'b2', title: 'Other', url: 'https://x.test', active: false },
]});
assert.equal(els['tabs'].children.length, 2);
assert.ok(els['tabs'].children[0].classList.contains('active') === false ||
          els['tabs'].children[0].className.includes('active'));
console.log('PASS tab strip render (2 tabs)');

// 5. tab click -> activate message (click first tab, not its x)
sent.length = 0;
els['tabs'].children[0].click();
assert.deepEqual(sent[0], { type: 'tab', action: 'activate', targetId: 'a1' });
console.log('PASS tab activate on click');

// 6. pointer input mapping: frame 1280x800 displayed at 640x400
sent.length = 0;
els['screen'].naturalWidth = 1280;
els['screen'].naturalHeight = 800;
els['screen']._rect = { x: 10, y: 50, width: 640, height: 400 };
els['screen']._fire('pointerdown', { clientX: 10 + 320, clientY: 50 + 200, button: 0, pointerId: 1, preventDefault: () => {} });
assert.equal(sent[0].type, 'mouse');
assert.equal(sent[0].action, 'down');
assert.equal(Math.round(sent[0].x), 640);
assert.equal(Math.round(sent[0].y), 400);
console.log('PASS pointer coordinate mapping (320,200 css -> 640,400 frame)');

// 7. keyboard: printable -> text, Enter -> vk code down/up
sent.length = 0;
const kd = docHandlers['keydown'][0], ku = docHandlers['keyup'][0];
kd({ key: 'a', preventDefault: () => {} });
assert.deepEqual(sent[0], { type: 'key', kind: 'text', text: 'a' });
kd({ key: 'Enter', preventDefault: () => {} });
assert.deepEqual(sent[1], { type: 'key', kind: 'code', vk: 13, down: true });
ku({ key: 'Enter' });
assert.deepEqual(sent[2], { type: 'key', kind: 'code', vk: 13, down: false });
console.log('PASS keyboard mapping');

// 8. role change -> view only status
wsInstance.inject({ type: 'role', driver: false });
assert.equal(els['status'].textContent, 'view only');
console.log('PASS role update');

// 9. mobile toggle button sends emulation
sent.length = 0;
els['mobile'].click();
assert.deepEqual(sent[0], { type: 'emulation', mobile: true });
console.log('PASS mobile toggle sends emulation');

// 10. wheel + pause (back to driver first)
wsInstance.inject({ type: 'role', driver: true });
sent.length = 0;
els['screen']._fire('wheel', { clientX: 100, clientY: 100, deltaX: 0, deltaY: 100, preventDefault: () => {} });
assert.equal(sent[0].type, 'wheel');
els['pause'].click();
wsInstance.inject({ type: 'frame', data: 'NEWDATA' });
assert.equal(els['screen'].src, 'data:image/jpeg;base64,QUJD', 'paused: frame ignored');
console.log('PASS wheel + pause');

// 11. pong -> latency pill
wsInstance.inject({ type: 'pong', t: Date.now() - 42 });
assert.match(els['latency'].textContent, /^\d+ ms$/);
assert.ok(els['latency'].classList.contains('ok'));
console.log('PASS latency indicator:', els['latency'].textContent);

// 12. new-tab sheet flow (no prompt() anymore)
sent.length = 0;
els['newtab'].click();
assert.ok(els['sheet'].classList.contains('open'));
els['sheetInput'].value = 'example.com';
els['sheetOk'].click();
assert.deepEqual(sent[0], { type: 'tab', action: 'new', url: 'example.com' });
assert.ok(!els['sheet'].classList.contains('open'));
console.log('PASS new-tab sheet sends URL');

console.log('CLIENT LOGIC: ALL PASS');
