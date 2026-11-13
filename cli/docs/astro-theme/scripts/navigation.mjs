/** Route helpers preserve URLs supplied by the content stage, including deployment prefixes. */
const releaseVersion = scope => scope.version.replace(/-(?:maven-central|jitpack)$/, '');
const providerRank = scope => scope.version.endsWith('-jitpack') ? 0 : scope.version.endsWith('-maven-central') ? 1 : 2;
export function searchFilters(scope) { return { language: scope.language, module: scope.module, version: scope.version }; }
export function compareVersions(a, b) {
  const parse = value => /^v?(\d+)\.(\d+)\.(\d+)(?:-([\da-z.-]+))?(?:\+[\da-z.-]+)?$/i.exec(value);
  const aa = parse(releaseVersion(a)), bb = parse(releaseVersion(b));
  if (!aa || !bb) return aa ? 1 : bb ? -1 : a.version.localeCompare(b.version);
  for (let i = 1; i <= 3; i++) {
    const av = BigInt(aa[i]), bv = BigInt(bb[i]);
    if (av !== bv) return av > bv ? 1 : -1;
  }
  if (!aa[4] || !bb[4]) return aa[4] ? -1 : bb[4] ? 1 : providerRank(a) - providerRank(b);
  const ap = aa[4].split('.'), bp = bb[4].split('.');
  for (let i = 0; i < Math.max(ap.length, bp.length); i++) {
    if (ap[i] === undefined) return -1;
    if (bp[i] === undefined) return 1;
    if (ap[i] === bp[i]) continue;
    const an = /^\d+$/.test(ap[i]), bn = /^\d+$/.test(bp[i]);
    if (an && bn) return BigInt(ap[i]) > BigInt(bp[i]) ? 1 : -1;
    if (an !== bn) return an ? -1 : 1;
    return ap[i] > bp[i] ? 1 : -1;
  }
  return 0;
}
export const isDevelopment = scope => scope.development === true || scope.version === 'development';
export function latestScope(scopes) {
  const releases = scopes.filter(s => !isDevelopment(s) && /^v?\d+\.\d+\.\d+(?:\+[\da-z.-]+)?$/i.test(releaseVersion(s)));
  return releases.sort(compareVersions).at(-1)
    ?? scopes.filter(s => !isDevelopment(s)).sort(compareVersions).at(-1)
    ?? scopes.find(isDevelopment);
}
export function versionScopes(scopes, current) {
  return scopes.filter(s => s.language === current.language && s.module === current.module)
    .sort((a, b) => Number(isDevelopment(a)) - Number(isDevelopment(b)) || compareVersions(b, a));
}
export function destination(scope, guide = '') {
  return scope.pages.find(p => p.slug === guide)?.href
    ?? scope.pages.find(p => ['', 'index', 'overview'].includes(p.slug))?.href
    ?? scope.pages[0]?.href ?? scope.root;
}
export function currentPage(pages, pathname, prefix = '') {
  const clean = value => decodeURI(value).replace(/\/+$/, '');
  const path = clean(pathname);
  return pages.find(p => clean(`${prefix}/${p.path}`) === path);
}
export function selectorGroups(scopes, current, guide) {
  const modules = [...new Set(scopes.map(s => s.module))].map(module => {
    const target = module === current.module ? current : latestScope(scopes.filter(s => s.module === module && s.language === current.language)) ?? latestScope(scopes.filter(s => s.module === module));
    return { label: module, href: destination(target, guide), selected: module === current.module };
  });
  const versions = versionScopes(scopes, current).map(s => ({ label: isDevelopment(s) ? 'Development' : s.version, href: destination(s, guide), selected: s === current }));
  const languages = [...new Set(scopes.filter(s => s.module === current.module).map(s => s.language))].map(language => {
    const candidates = scopes.filter(s => s.module === current.module && s.language === language);
    const target = candidates.find(s => s.version === current.version) ?? latestScope(candidates);
    return { label: language, href: destination(target, guide), selected: language === current.language };
  });
  return { Module: modules, Version: versions, Language: languages };
}
export function homeTargets(scopes) {
  const language = scopes.some(s => s.language === 'en') ? 'en' : scopes[0]?.language;
  return [...new Set(scopes.map(s => s.module))].map(module => {
    const target = latestScope(scopes.filter(s => s.module === module && s.language === language)) ?? latestScope(scopes.filter(s => s.module === module));
    return { module, scope: target, href: destination(target) };
  });
}

export function repositoryUrl(value) {
  if (!value) return undefined;
  return /^https?:\/\//.test(value) ? value.replace(/\/$/, "") : `https://github.com/${value}`;
}
