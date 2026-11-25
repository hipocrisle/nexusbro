// Actual updater.ts; Tauri/updater boundaries are explicit bounded adapters.
// Native shutdown/install and real streamed browser coverage run separately.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require('../desktop/node_modules/typescript');

const source = fs.readFileSync(path.join(__dirname, '../desktop/src/updater.ts'), 'utf8');
const transformed = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  reportDiagnostics: true,
});
assert.equal(transformed.diagnostics?.length ?? 0, 0, 'updater source parses');

function fixture({ failure = '', accepted = true, available = true } = {}) {
  const calls = [], progress = [], messages = [], storage = new Map();
  let visible = true, prepared = false, draining = false, currentFailure = failure;
  const update = {
    version: 'test',
    async close() { calls.push('close'); },
    async download(onEvent) {
      calls.push('download');
      assert.equal(visible, true, 'sites remain visible before and during download');
      onEvent({ event: 'Started', data: { contentLength: 100 } });
      await new Promise(resolve => setImmediate(resolve));
      assert.equal(visible, true, 'network wait does not hide pages');
      onEvent({ event: 'Progress', data: { chunkLength: 50 } });
      assert.equal(visible, true, 'progress does not hide pages');
      if (currentFailure === 'download') throw Error('download failed');
      onEvent({ event: 'Finished', data: {} });
    },
    async install() {
      calls.push('install');
      assert.equal(visible, false);
      assert.equal(prepared, true, 'install requires successful awaited preparation');
      assert.equal(draining, true);
      if (['install', 'install-and-restart'].includes(currentFailure)) throw Error('install failed');
    },
  };
  const modules = {
    '@tauri-apps/plugin-updater': { check: async () => {
      calls.push('check');
      if (currentFailure === 'check') throw Error('check failed');
      return available ? update : null;
    } },
    '@tauri-apps/plugin-dialog': { ask: async () => {
      calls.push('ask');
      assert.equal(visible, false, 'confirmation is not covered by CEF');
      if (currentFailure === 'dialog') throw Error('dialog failed');
      return accepted;
    } },
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      if (command === 'pokazat_dvizhki') {
        // Root rejects even visibility commands once native draining began.
        if (args.vidno && draining) {
          calls.push('show-rejected');
          throw Error('restore rejected while draining');
        }
        visible = args.vidno;
        calls.push(visible ? 'show' : 'hide');
        return;
      }
      if (command === 'app_prepare_update') {
        calls.push('prepare');
        assert.equal(visible, false);
        if (currentFailure === 'prepare-before-drain') throw Error('prepare failed before drain');
        draining = true;
        await new Promise(resolve => setImmediate(resolve));
        if (currentFailure === 'prepare') throw Error('prepare failed');
        prepared = true;
        return;
      }
      if (command === 'app_restart') {
        calls.push('restart');
        assert.equal(prepared, true, 'restart follows actual successful preparation');
        if (['restart', 'install-and-restart'].includes(currentFailure)) throw Error('restart failed');
        return;
      }
      assert.fail(`unexpected command ${command}`);
    } },
    // No plugin-process adapter: importing it is a contract violation.
  };
  const context = {
    exports: {}, require: name => {
      assert.ok(modules[name], `unexpected module ${name}`);
      return modules[name];
    },
    localStorage: { getItem: key => storage.get(key) ?? null, setItem: (key, value) => storage.set(key, value) },
    console: { log() {}, warn() {} },
  };
  vm.runInNewContext(transformed.outputText, context, { filename: 'updater.ts', timeout: 1000 });
  return {
    calls, progress, messages, storage,
    visible: () => visible, prepared: () => prepared,
    setFailure: value => { currentFailure = value; },
    run: (force = true, quiet = false) => context.exports.proverit_obnovlenie(force, quiet,
      text => messages.push(text), (received, total) => progress.push([received, total])),
  };
}
const bounded = { timeout: 2000 };
const count = (f, name) => f.calls.filter(value => value === name).length;
function noResumeAfterPrepare(f) {
  const after = f.calls.slice(f.calls.indexOf('prepare') + 1);
  assert.ok(!after.includes('show') && !after.includes('show-rejected'), 'no attempt to resume drained writers');
  assert.equal(f.visible(), false);
  assert.equal(f.prepared(), true);
}

test('download remains visible; awaited prepare precedes install and native restart', bounded, async () => {
  const f = fixture(); await f.run();
  assert.deepEqual(f.calls, ['check', 'hide', 'ask', 'show', 'download', 'hide', 'prepare', 'install', 'restart', 'close']);
  assert.deepEqual(f.progress, [[50, 100], [100, 100]]);
  assert.equal(count(f, 'prepare'), 1); assert.equal(count(f, 'restart'), 1);
  noResumeAfterPrepare(f);
  assert.ok(!f.messages.some(value => value.includes('Не удалось')));
});

for (const failure of ['dialog', 'download']) {
  test(`${failure} failure before preparation restores pages without install/restart`, bounded, async () => {
    const f = fixture({ failure }); await f.run();
    assert.equal(f.visible(), true); assert.equal(f.prepared(), false);
    assert.match(f.messages.at(-1), new RegExp(`${failure} failed`));
    for (const name of ['prepare', 'install', 'restart']) assert.equal(count(f, name), 0);
    assert.equal(count(f, 'close'), 1);
  });
}

test('failed prepare blocks install/restart and a rejected restore cannot mask its error', bounded, async () => {
  const f = fixture({ failure: 'prepare' }); await f.run();
  assert.equal(f.prepared(), false);
  assert.equal(count(f, 'prepare'), 1); assert.equal(count(f, 'show-rejected'), 1);
  assert.equal(count(f, 'install'), 0); assert.equal(count(f, 'restart'), 0);
  assert.match(f.messages.at(-1), /prepare failed/);
  assert.doesNotMatch(f.messages.at(-1), /restore rejected/);
  assert.equal(count(f, 'close'), 1);
});

test('prepare rejected before draining can restore pages but never install/restart', bounded, async () => {
  const f = fixture({ failure: 'prepare-before-drain' }); await f.run();
  assert.equal(f.visible(), true); assert.equal(f.prepared(), false);
  assert.equal(count(f, 'show-rejected'), 0);
  assert.equal(count(f, 'install'), 0); assert.equal(count(f, 'restart'), 0);
  assert.match(f.messages.at(-1), /prepare failed before drain/);
});

test('install failure after clean prepare requests exactly one restart of installed app', bounded, async () => {
  const f = fixture({ failure: 'install' }); await f.run();
  assert.equal(count(f, 'install'), 1); assert.equal(count(f, 'restart'), 1);
  assert.ok(f.calls.indexOf('prepare') < f.calls.indexOf('install'));
  assert.ok(f.calls.indexOf('install') < f.calls.indexOf('restart'));
  assert.match(f.messages.at(-1), /install failed/);
  assert.equal(count(f, 'close'), 1); noResumeAfterPrepare(f);
});

test('failed restart after successful install is not retried and does not resume pages', bounded, async () => {
  const f = fixture({ failure: 'restart' }); await f.run();
  assert.equal(count(f, 'install'), 1); assert.equal(count(f, 'restart'), 1);
  assert.match(f.messages.at(-1), /restart failed/);
  assert.equal(count(f, 'close'), 1); noResumeAfterPrepare(f);
});

test('install and restart failures both remain visible; no second restart or resume', bounded, async () => {
  const f = fixture({ failure: 'install-and-restart' }); await f.run();
  assert.equal(count(f, 'install'), 1); assert.equal(count(f, 'restart'), 1);
  assert.match(f.messages.at(-1), /install failed/); assert.match(f.messages.at(-1), /restart failed/);
  assert.equal(count(f, 'close'), 1); noResumeAfterPrepare(f);
});

test('declining closes updater resource and never downloads/prepares', bounded, async () => {
  const f = fixture({ accepted: false }); await f.run();
  assert.deepEqual(f.calls, ['check', 'hide', 'ask', 'show', 'close']);
  assert.equal(f.visible(), true);
});

test('no update never hides pages or enters draining', bounded, async () => {
  const f = fixture({ available: false }); await f.run();
  assert.deepEqual(f.calls, ['check']); assert.equal(f.visible(), true);
});

test('check failure remains quiet and does not suppress immediate manual retry', bounded, async () => {
  const f = fixture({ failure: 'check' }); await f.run(false, true);
  assert.deepEqual(f.calls, ['check']); assert.deepEqual(f.messages, []); assert.equal(f.storage.size, 0);
  f.setFailure(''); await f.run(true, false);
  assert.equal(count(f, 'check'), 2); assert.equal(count(f, 'prepare'), 1); assert.equal(count(f, 'install'), 1);
});

test('singleflight duplicate startup/manual requests share one prepare/install/restart', bounded, async () => {
  const f = fixture(); const startup = f.run(false, true), manual = f.run(true, false);
  assert.equal(startup, manual); await startup;
  for (const name of ['check', 'ask', 'download', 'prepare', 'install', 'restart', 'close']) assert.equal(count(f, name), 1);
});
