import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {runInNewContext} from 'node:vm';
const script = readFileSync(new URL('../static/docs.js', import.meta.url), 'utf8');
for (const path of ['', '/', '/repository', '/repository/', '/nested/repository']) {
  test(`catalog, language navigation and search retain base ${path || '(origin)'}`, async () => {
    const root = `https://example.com${path.replace(/\/$/, '')}/`;
    const handlers = {}, requests = [], navigations = [];
    const element = id => ({addEventListener: (event, fn) => { handlers[`${id}:${event}`] = fn; }, replaceChildren() {}, append() {}, value: 'permission'});
    const elements = Object.fromEntries(['#version-select', '#language-select', '#search-input', '#search-results'].map(id => [id, element(id)]));
    const scopes = ['en', 'es'].map(language => ({language, module: 'permission', version: '0.2.6', root: `${root}${language}/permission/0.2.6/`, pages: [{slug: 'api', href: `${root}${language}/permission/0.2.6/api/`}]}));
    runInNewContext(script, {URL, document: {body: {dataset: {base: `https://example.com${path}`, scope: '0', guide: 'api', language: 'en'}, hasAttribute: () => true}, querySelector: id => elements[id], querySelectorAll: () => [], addEventListener() {}, createElement: () => element('new')}, location: {href: `${root}en/permission/0.2.6/api/`, origin: 'https://example.com', assign: url => navigations.push(url)}, window: {}, sessionStorage: {getItem: () => null}, fetch: async url => { requests.push(url.href); return {ok: true, json: async () => url.pathname.endsWith('versions.json') ? scopes : []}; }});
    await handlers['#version-select:change']({target: {value: '1'}});
    await handlers['#language-select:change']({target: {value: 'es'}});
    await handlers['#search-input:input']();
    assert.deepEqual(requests, [`${root}versions.json`, `${root}search_index.en.json`]);
    assert.deepEqual(navigations, [scopes[1].pages[0].href, scopes[1].pages[0].href]);
  });
}
