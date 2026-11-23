// Проверяем настоящий updater.ts с управляемыми границами Tauri/установщика.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require('../desktop/node_modules/typescript');

function fixture({ failure, accepted = true, available = true } = {}) {
  const calls = [], progress = [], messages = [];
  let visible = true;
  const update = {
    version: 'test',
    async close() {}, // Real Update owns a backend resource and downloaded bytes.
    async download(onEvent) {
      calls.push('download');
      assert.equal(visible, true, 'сайты видны при начале скачивания');
      onEvent({ event: 'Started', data: { contentLength: 100 } });
      await new Promise(resolve => setImmediate(resolve));
      assert.equal(visible, true, 'ожидание сети не скрывает сайты');
      onEvent({ event: 'Progress', data: { chunkLength: 50 } });
      assert.equal(visible, true, 'сайты видны во время показа прогресса');
      if (failure === 'download') throw Error('download failed');
      onEvent({ event: 'Finished', data: {} });
    },
    async install() {
      calls.push('install');
      assert.equal(visible, false, 'движки скрыты только на время установки');
      if (failure === 'install') throw Error('install failed');
    },
  };
  const modules = {
    '@tauri-apps/plugin-updater': { check: async () => available ? update : null },
    '@tauri-apps/plugin-dialog': { ask: async () => {
      calls.push('ask');
      assert.equal(visible, false, 'диалог подтверждения не закрыт CEF');
      if (failure === 'dialog') throw Error('dialog failed');
      return accepted;
    } },
    '@tauri-apps/api/core': { invoke: async (cmd, args) => {
      assert.equal(cmd, 'pokazat_dvizhki');
      visible = args.vidno;
      calls.push(visible ? 'show' : 'hide');
    } },
    '@tauri-apps/plugin-process': { relaunch: async () => {
      calls.push('relaunch');
      if (failure === 'relaunch') throw Error('relaunch failed');
    } },
  };
  const source = fs.readFileSync(path.join(__dirname, '../desktop/src/updater.ts'), 'utf8');
  const compiled = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  }).outputText;
  const context = {
    exports: {}, require: name => {
      assert.ok(modules[name], `неожиданный модуль ${name}`);
      return modules[name];
    },
    localStorage: { getItem: () => null, setItem: () => {} },
    console: { log() {}, warn() {} },
  };
  vm.runInNewContext(compiled, context, { filename: 'updater.ts' });
  return {
    calls, progress, messages,
    visible: () => visible,
    run: () => context.exports.proverit_obnovlenie(true, false,
      text => messages.push(text), (received, total) => progress.push([received, total])),
  };
}

test('сайты доступны при загрузке, установка начинается только после неё', async () => {
  const f = fixture();
  await f.run();
  assert.deepEqual(f.calls, ['hide', 'ask', 'show', 'download', 'hide', 'install', 'relaunch', 'show']);
  assert.deepEqual(f.progress, [[50, 100], [100, 100]]);
  assert.equal(f.visible(), true);
});

for (const failure of ['dialog', 'download', 'install', 'relaunch']) {
  test(`ошибка ${failure} оставляет сайты видимыми`, async () => {
    const f = fixture({ failure });
    await f.run();
    assert.equal(f.visible(), true);
    assert.match(f.messages.at(-1), new RegExp(`${failure} failed`));
    if (failure === 'download' || failure === 'dialog') assert.ok(!f.calls.includes('install'));
    if (failure === 'install') assert.ok(!f.calls.includes('relaunch'));
  });
}

test('отказ от обновления восстанавливает сайты и не скачивает пакет', async () => {
  const f = fixture({ accepted: false });
  await f.run();
  assert.deepEqual(f.calls, ['hide', 'ask', 'show']);
  assert.equal(f.visible(), true);
});

test('отсутствие обновления вообще не скрывает сайты', async () => {
  const f = fixture({ available: false });
  await f.run();
  assert.deepEqual(f.calls, []);
  assert.equal(f.visible(), true);
});
