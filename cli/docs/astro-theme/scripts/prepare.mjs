import { readFile, writeFile, mkdir, rm } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
export function validate(pages, scopes, site) {
  if (!Array.isArray(pages) || !Array.isArray(scopes) || !scopes.length) throw Error('pages and nonempty scopes must be arrays');
  if (typeof site.title !== 'string' || !site.title) throw Error('site.title is required');
  const safeUrl = value => typeof value === 'string' && (value.startsWith('/') && !value.startsWith('//') || /^https?:\/\//.test(value));
  scopes.forEach((scope, i) => {
    for (const field of ['language', 'module', 'label', 'version', 'root']) if (typeof scope[field] !== 'string') throw Error(`scope ${i}: ${field} must be a string`);
    if (!safeUrl(scope.root) || !Array.isArray(scope.pages)) throw Error(`scope ${i}: invalid root/pages`);
    scope.pages.forEach(page => {
      if (typeof page.slug !== 'string' || typeof page.title !== 'string' || !safeUrl(page.href) || page.raw && !safeUrl(page.raw)) throw Error(`scope ${i}: invalid page`);
    });
  });
  const paths = new Set();
  pages.forEach(page => {
    if (typeof page.path !== 'string' || !/^[\p{L}\p{N}_-]+(?:\/[\p{L}\p{N}_.-]+)*$/u.test(page.path) || page.path.split('/').some(p => p === '.' || p === '..')) throw Error(`unsafe page path: ${page.path}`);
    if (paths.has(page.path)) throw Error(`duplicate page path: ${page.path}`);
    paths.add(page.path);
    if (typeof page.body !== 'string' || typeof page.title !== 'string' || !Number.isInteger(page.scope_index) || !scopes[page.scope_index]) throw Error(`invalid page: ${page.path}`);
  });
  if (site.prefix && (!site.prefix.startsWith('/') || site.prefix.startsWith('//') || site.prefix.includes('..') || /[?#]/.test(site.prefix))) throw Error('invalid site.prefix');
  if (site.repository && !/^https?:\/\//.test(site.repository) && !/^[a-z\d_.-]+\/[a-z\d_.-]+$/i.test(site.repository)) throw Error('invalid site.repository');
  if (site.base_url && !/^https?:\/\//.test(site.base_url)) throw Error('invalid site.base_url');
}
export async function prepare(root = process.cwd()) {
  const [pages, scopes, site] = await Promise.all(['pages', 'scopes', 'site'].map(async name => JSON.parse(await readFile(resolve(root, `${name}.json`), 'utf8'))));
  validate(pages, scopes, site);
  const docs = resolve(root, 'src/content/docs');
  await rm(docs, { recursive: true, force: true });
  await mkdir(docs, { recursive: true });
  for (const page of pages) {
    const file = resolve(docs, `${page.path}.md`);
    await mkdir(resolve(file, '..'), { recursive: true });
    await writeFile(file, `---\ntitle: ${JSON.stringify(page.title)}\nslug: ${JSON.stringify(page.path)}\neditUrl: false\nprev: false\nnext: false\n---\n\n${page.body}\n`);
  }
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) await prepare();
