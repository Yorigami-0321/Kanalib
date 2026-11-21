#!/usr/bin/env node
/**
 * 输入层取证探针（只读）：注入鼠标离散滚轮 / 触控板连续垂直 / 水平 / 对角 /
 * 捏合序列，逐帧采样 .world 变换，量化：
 * - 回滚：与总位移反号且 >1px 的帧数；
 * - 失速：首末运动之间连续零位移的最长时长（ms）；
 * - 对角：x/y 两轴位移是否都发生；
 * - 平滑度：帧速度 p95/p50（越接近 1 越匀）。
 * 用法：node scripts/wall_probe_wheel.mjs --base=http://127.0.0.1:4318/
 */
import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';

const CANDIDATES = [
  'C:/Program Files/Google/Chrome/Application/chrome.exe',
  'C:/Program Files (x86)/Google/Chrome/Application/chrome.exe',
  'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',
  'C:/Program Files/Microsoft/Edge/Application/msedge.exe',
];

class Cdp {
  constructor(url) { this.url = url; this.nextId = 1; this.pending = new Map(); }
  async connect() {
    this.ws = new WebSocket(this.url);
    await new Promise((res, rej) => {
      this.ws.addEventListener('open', res, { once: true });
      this.ws.addEventListener('error', () => rej(new Error('CDP 连接失败')), { once: true });
    });
    this.ws.addEventListener('message', (event) => {
      const message = JSON.parse(event.data);
      if (message.id && this.pending.has(message.id)) {
        const { resolve: done, reject } = this.pending.get(message.id);
        this.pending.delete(message.id);
        if (message.error) reject(new Error(message.method + ': ' + message.error.message));
        else done(message.result);
      }
    });
  }
  send(method, params = {}) {
    const id = this.nextId++;
    return new Promise((res, rej) => {
      this.pending.set(id, { resolve: res, reject: rej });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }
  close() { try { this.ws.close(); } catch { /* noop */ } }
}

async function waitForDevtools(port, timeoutMs = 12000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try { const r = await fetch(`http://127.0.0.1:${port}/json/version`); if (r.ok) return; } catch { /* retry */ }
    await sleep(150);
  }
  throw new Error('Chrome 调试端口没起来');
}

function hardDelete(dir) {
  if (!dir || !existsSync(dir)) return;
  if (process.platform === 'win32') {
    spawnSync('cmd', ['/c', 'rd', '/s', '/q', dir], { stdio: 'ignore' });
    if (!existsSync(dir)) return;
  }
  try { rmSync(dir, { recursive: true, force: true }); } catch { /* noop */ }
}

const SAMPLER = `(() => {
  window.__trace = [];
  window.__wheels = [];
  window.__muts = [];
  window.__sampling = true;
  const w0 = document.querySelector('.world');
  if (w0 && !window.__mo) {
    window.__mo = new MutationObserver(() => {
      const w = document.querySelector('.world');
      if (!w) return;
      const s = new DOMMatrixReadOnly(w.style.transform || 'none');
      window.__muts.push([+performance.now().toFixed(1), +s.e.toFixed(2), +s.f.toFixed(2)]);
    });
    window.__mo.observe(w0, { attributes: true, attributeFilter: ['style'] });
    window.addEventListener('wheel', (e) => {
      window.__wheels.push([+performance.now().toFixed(1), e.deltaX, e.deltaY]);
    }, { capture: true, passive: true });
  }
  const tick = (t) => {
    const w = document.querySelector('.world');
    if (w) {
      const m = new DOMMatrixReadOnly(getComputedStyle(w).transform);
      const s = new DOMMatrixReadOnly(w.style.transform || 'none');
      window.__trace.push([+t.toFixed(1), +m.e.toFixed(2), +m.f.toFixed(2), +m.a.toFixed(3), +s.e.toFixed(2), +s.f.toFixed(2)]);
    }
    if (window.__sampling) requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
  return performance.now();
})()`;

function analyze(trace, axis, wheels = []) {
  // axis: 'y' | 'x' | 'zoom'；trace 行 = [t, e, f, a, styleE, styleF]
  const idx = axis === 'y' ? 2 : axis === 'x' ? 1 : 3;
  const vals = trace.map((r) => r[idx]);
  const ts = trace.map((r) => r[0]);
  const total = vals[vals.length - 1] - vals[0];
  const dir = Math.sign(total) || 1;
  // 输入活跃期：首末 wheel 之间 + 150ms 收尾；探针投递间隙（>60ms 无 wheel）
  // 不算失速——输入暂停时相机收敛是正确行为。
  const w0 = wheels.length ? wheels[0][0] : ts[0];
  const w1 = wheels.length ? wheels[wheels.length - 1][0] + 150 : ts[ts.length - 1];
  const wheelGapAt = (t) => {
    let prev = w0;
    for (const w of wheels) {
      if (w[0] > t) break;
      prev = w[0];
    }
    let next = w1;
    for (let i = wheels.length - 1; i >= 0; i -= 1) {
      if (wheels[i][0] <= t) { next = wheels[i + 1] ? wheels[i + 1][0] : w1; break; }
    }
    return Math.max(t - prev, next - t);
  };
  let reversals = 0;
  let stallMs = 0;
  let run = 0;
  let runStart = -1;
  let firstMove = -1;
  const speeds = [];
  for (let i = 1; i < vals.length; i += 1) {
    const d = vals[i] - vals[i - 1];
    const dt = Math.max(0.1, ts[i] - ts[i - 1]);
    const active = ts[i] >= w0 && ts[i] <= w1;
    if (Math.abs(d) > 0.05) {
      if (firstMove < 0) firstMove = i;
      if (active) speeds.push(Math.abs(d) / dt * 16.7);
    }
    if (Math.abs(d) > 1 && Math.sign(d) !== dir) reversals += 1;
    if (Math.abs(d) < 0.05) {
      if (runStart < 0) runStart = ts[i - 1];
      run += dt;
      if (firstMove >= 0 && active && wheelGapAt(ts[i]) < 60) stallMs = Math.max(stallMs, run);
    } else {
      run = 0;
      runStart = -1;
    }
  }
  speeds.sort((a, b) => a - b);
  const p = (q) => speeds.length ? speeds[Math.min(speeds.length - 1, Math.floor(q * speeds.length))] : 0;
  // 投递节拍：CDP 逐事件往返会突发+空窗，页内 rAF 驱动才是真实触控板节拍。
  const gaps = [];
  for (let i = 1; i < wheels.length; i += 1) gaps.push(wheels[i][0] - wheels[i - 1][0]);
  gaps.sort((a, b) => a - b);
  const gq = (q) => gaps.length ? +gaps[Math.min(gaps.length - 1, Math.floor(q * gaps.length))].toFixed(1) : null;
  // 跟随误差：输入累计位移 vs 相机实际位移的平均绝对差（跟手程度）。
  // 相机 y = -f，x = -e；输入 dy/dx 直接累加到相机目标。
  const sign = axis === 'zoom' ? 1 : -1;
  const camAt = (i) => sign * (vals[i] - vals[0]);
  let errSum = 0;
  let errN = 0;
  let cum = 0;
  let wi = 0;
  for (let i = 0; i < vals.length; i += 1) {
    while (wi < wheels.length && wheels[wi][0] <= ts[i]) {
      cum += axis === 'y' ? wheels[wi][2] : axis === 'x' ? wheels[wi][1] : 0;
      wi += 1;
    }
    if (ts[i] >= w0 && ts[i] <= w1 && axis !== 'zoom') {
      errSum += Math.abs(camAt(i) - cum);
      errN += 1;
    }
  }
  // 收敛时间：末次输入后相机进入终值 ±2px 所需时长。
  const final = vals[vals.length - 1];
  let settleMs = null;
  const lastW = wheels.length ? wheels[wheels.length - 1][0] : ts[0];
  for (let i = 0; i < vals.length; i += 1) {
    if (ts[i] >= lastW && Math.abs(vals[i] - final) < 2) {
      settleMs = Math.round(ts[i] - lastW);
      break;
    }
  }
  return {
    total: +total.toFixed(1),
    reversals,
    stallMs: Math.round(stallMs),
    followErr: errN ? +(errSum / errN).toFixed(1) : null,
    settleMs,
    vP50: +p(0.5).toFixed(2),
    vP95: +p(0.95).toFixed(2),
    spikiness: p(0.5) > 0.01 ? +(p(0.95) / p(0.5)).toFixed(2) : null,
    frames: vals.length,
    spanMs: Math.round(ts[ts.length - 1] - ts[0]),
    wheelGapP50: gq(0.5),
    wheelGapP95: gq(0.95),
  };
}

async function main() {
  const argv = process.argv.slice(2);
  const base = (argv.find((a) => a.startsWith('--base'))?.split('=')[1] ?? 'http://127.0.0.1:4318/');
  const chrome = CANDIDATES.find(existsSync);
  if (!chrome) { console.error('no chrome'); process.exit(2); }
  const profile = mkdtempSync(join(tmpdir(), 'kanalib-wheel-'));
  const port = 9980 + Math.floor(Math.random() * 20);
  const child = spawn(chrome, [
    '--headless=new', `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`,
    '--no-first-run', '--no-default-browser-check', '--disable-extensions', '--hide-scrollbars',
    '--window-size=1440,900', 'about:blank',
  ], { stdio: 'ignore' });

  let cdp;
  try {
    await waitForDevtools(port);
    const resp = await fetch(`http://127.0.0.1:${port}/json/new?about:blank`, { method: 'PUT' });
    const target = await resp.json();
    cdp = new Cdp(target.webSocketDebuggerUrl);
    await cdp.connect();
    await cdp.send('Page.enable');
    await cdp.send('Runtime.enable');
    await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
    const evaluate = async (expression) => {
      const result = await cdp.send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
      if (result.exceptionDetails) throw new Error(result.exceptionDetails.text);
      return result.result.value;
    };
    const wheel = (deltaX, deltaY, modifiers = 0) =>
      cdp.send('Input.dispatchMouseEvent', { type: 'mouseWheel', x: 1200, y: 450, deltaX, deltaY, modifiers });

    await cdp.send('Page.navigate', { url: base + '#/' });
    for (let i = 0; i < 80; i += 1) {
      try { const n = await evaluate(`document.querySelectorAll('.tile img.is-loaded').length`); if (n > 10) break; } catch { /* nav */ }
      await sleep(200);
    }
    await sleep(1000);

    const scenarios = [
      { name: 'mouse-down', events: 12, dx: 0, dy: 100, gap: 90, axis: 'y' },
      { name: 'pad-vertical', events: 60, dx: 0, dy: 12, gap: 16, axis: 'y' },
      { name: 'pad-horizontal', events: 60, dx: 12, dy: 0, gap: 16, axis: 'x' },
      { name: 'pad-diagonal', events: 60, dx: 8, dy: 8, gap: 16, axis: 'y' },
      { name: 'pad-long-burst', events: 120, dx: 0, dy: 6, gap: 16, axis: 'y' },
      { name: 'pinch-zoom', events: 20, dx: 0, dy: -24, gap: 30, axis: 'zoom', modifiers: 2 },
    ];
    const report = {};
    for (const s of scenarios) {
      await evaluate(`window.__sampling = false; window.__trace = [];`);
      await sleep(120);
      await evaluate(SAMPLER);
      for (let i = 0; i < s.events; i += 1) {
        await wheel(s.dx, s.dy, s.modifiers ?? 0);
        await sleep(s.gap);
      }
      await sleep(600);
      const trace = await evaluate(`window.__sampling = false; window.__trace`);
      const wheels = await evaluate(`window.__wheels`);
      const main = analyze(trace, s.axis, wheels);
      report[s.name] = main;
      if (s.name === 'pad-diagonal') report[s.name].x = analyze(trace, 'x', wheels);
      console.log(s.name + ':', JSON.stringify(report[s.name]));
      if (argv.includes('--dump') && (main.reversals > 0 || main.stallMs > 80)) {
        const idx = s.axis === 'y' ? 2 : s.axis === 'x' ? 1 : 3;
        const dir = Math.sign(main.total) || 1;
        let shown = 0;
        for (let i = 1; i < trace.length && shown < 3; i += 1) {
          const d = trace[i][idx] - trace[i - 1][idx];
          if (Math.abs(d) > 1 && Math.sign(d) !== dir) {
            console.log('  reversal window [t, compE, compF, zoom, styleE, styleF]:', JSON.stringify(trace.slice(Math.max(0, i - 4), i + 4)));
            shown += 1;
          }
        }
        const styleIdx = s.axis === 'y' ? 5 : 4;
        let styleReversals = 0;
        for (let i = 1; i < trace.length; i += 1) {
          const d = trace[i][styleIdx] - trace[i - 1][styleIdx];
          if (Math.abs(d) > 1 && Math.sign(d) !== dir) styleReversals += 1;
        }
        console.log('  style-level reversals:', styleReversals);
        if (s.name === 'pad-vertical' || s.name === 'mouse-down') {
          const muts = await evaluate(`window.__muts`);
          const wheels = await evaluate(`window.__wheels`);
          const idx2 = s.axis === 'y' ? 2 : 1;
          let at = -1;
          for (let i = 1; i < muts.length; i += 1) {
            const d = muts[i][idx2] - muts[i - 1][idx2];
            if (Math.abs(d) > 1 && Math.sign(d) !== dir) { at = muts[i][0]; break; }
          }
          console.log('  wheels around first mut-reversal:', JSON.stringify(wheels.filter((w) => Math.abs(w[0] - at) < 120)));
          console.log('  muts around first mut-reversal:', JSON.stringify(muts.filter((m) => Math.abs(m[0] - at) < 120)));
          // 失速窗口：trace 中最长零位移run 的起止与期间 wheel/mut 活动
          let bestStart = -1;
          let bestLen = 0;
          let runStart = -1;
          for (let i = 1; i < trace.length; i += 1) {
            const d = trace[i][idx2] - trace[i - 1][idx2];
            if (Math.abs(d) < 0.05) {
              if (runStart < 0) runStart = i - 1;
              const len = trace[i][0] - trace[runStart][0];
              if (len > bestLen) { bestLen = len; bestStart = runStart; }
            } else runStart = -1;
          }
          if (bestStart >= 0) {
            const t0 = trace[bestStart][0];
            const t1 = t0 + bestLen;
            console.log('  stall window', Math.round(t0), '-', Math.round(t1),
              'wheels:', JSON.stringify(wheels.filter((w) => w[0] >= t0 - 30 && w[0] <= t1 + 30).length),
              'muts:', JSON.stringify(muts.filter((m) => m[0] >= t0 - 30 && m[0] <= t1 + 30).length),
              'trace:', JSON.stringify(trace.slice(bestStart, bestStart + 6)));
          }
        }
      }
    }
    // 页内等节拍驱动：每帧一个 wheel（真实触控板 ≈ 显示刷新率），避开 CDP
    // 逐事件往返造成的"突发+空窗"。跟随误差在"下一次投递前"读相机，量的是
    // 新输入到达时上一笔输入还差多少没跟上（含渲染提交的固有延迟）。
    const cadence = async (name, events, dx, dy, everyFrames, axis) => {
      await evaluate(`window.__sampling = false; window.__trace = [];`);
      await sleep(120);
      await evaluate(SAMPLER);
      const out = await evaluate(`(async () => {
        const host = document.querySelector('.canvas');
        const read = () => {
          const w = document.querySelector('.world');
          const m = new DOMMatrixReadOnly(w.style.transform || 'none');
          return [m.e, m.f];
        };
        const start = read();
        window.__follow = [];
        let sent = 0; let frame = 0; let cumX = 0; let cumY = 0;
        await new Promise((res) => {
          const tick = (t) => {
            frame += 1;
            if (frame % ${everyFrames} === 0) {
              const cur = read();
              window.__follow.push([+t.toFixed(1), +(start[1] - cur[1]).toFixed(2), cumY, +(start[0] - cur[0]).toFixed(2), cumX]);
              host.dispatchEvent(new WheelEvent('wheel', { deltaX: ${dx}, deltaY: ${dy}, deltaMode: 0, bubbles: true, cancelable: true }));
              cumX += ${dx}; cumY += ${dy}; sent += 1;
            }
            if (sent < ${events}) requestAnimationFrame(tick); else res();
          };
          requestAnimationFrame(tick);
        });
        await new Promise((r) => setTimeout(r, 600));
        window.__sampling = false;
        return { trace: window.__trace, wheels: window.__wheels, follow: window.__follow };
      })()`);
      const stats = analyze(out.trace, axis, out.wheels);
      const col = axis === 'x' ? 3 : 1;
      const errs = out.follow.map((r) => Math.abs(r[col] - r[col + 1])).sort((a, b) => a - b);
      const mean = errs.length ? errs.reduce((a, b) => a + b, 0) / errs.length : 0;
      stats.followAtInput = {
        mean: +mean.toFixed(2),
        p95: errs.length ? +errs[Math.floor(0.95 * (errs.length - 1))].toFixed(2) : null,
        max: errs.length ? +errs[errs.length - 1].toFixed(2) : null,
      };
      delete stats.followErr;
      report[name] = stats;
      if (dx !== 0) report[name].x = analyze(out.trace, 'x', out.wheels);
      console.log(name + ':', JSON.stringify(report[name]));
    };
    await cadence('pad-cadence-vertical', 60, 0, 12, 1, 'y');
    await cadence('pad-cadence-diagonal', 60, 8, 8, 1, 'y');
    await cadence('pad-cadence-fast', 90, 0, 24, 1, 'y');
    await cadence('mouse-cadence', 12, 0, 100, 6, 'y');

    const errors = await evaluate(`window.__consoleErrors ?? []`);
    console.log('consoleErrors:', JSON.stringify(errors));
  } finally {
    cdp?.close();
    try { child.kill(); } catch { /* noop */ }
    await sleep(300);
    hardDelete(profile);
  }
}

main().catch((error) => { console.error(error); process.exit(1); });