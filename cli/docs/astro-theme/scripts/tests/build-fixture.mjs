import assert from 'node:assert/strict';
import { mkdtemp,cp,writeFile,mkdir,readFile,rm,symlink } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve,join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { parse } from 'parse5';
import { verifySearch } from './search-index.mjs';
const root=fileURLToPath(new URL('../../',import.meta.url));
const temp=await mkdtemp(join(tmpdir(),'android-repo-astro-test-'));
const text=node=>node.nodeName==='#text'?node.value:(node.childNodes||[]).map(text).join('');
const nodes=root=>[root,...(root.childNodes||[]).flatMap(nodes)];
const attr=(node,name)=>node.attrs?.find(a=>a.name===name)?.value;
try{
 for(const prefix of ['/repo','']){
  const app=join(temp,prefix?'repo':'root');
  await cp(root,app,{recursive:true,filter:source=>!['node_modules','dist','.astro'].some(part=>source.split('/').includes(part))});
  await cp(resolve(root,'node_modules'),join(app,'node_modules'),{recursive:true});
  const scopes=['1.2.0','1.10.0','development'].flatMap(version=>['en','es'].map(language=>({language,module:'core',label:'Core',version,root:`${prefix}/${language}/core/${version}/`,source:'abc123',documentation_ref:'docs-v1',development:version==='development',images:[{src:`${prefix}/media/${version}/catalog-light.png`,alt:'Immutable catalog'}],pages:[{slug:'',title:'Overview',href:`${prefix}/${language}/core/${version}/`,raw:`${prefix}/${language}/core/${version}/raw/index.md`},{slug:'quickstart',title:'Quickstart',href:`${prefix}/${language}/core/${version}/quickstart/`,raw:`${prefix}/${language}/core/${version}/raw/quickstart.md`}]})));
  const pages=scopes.flatMap((scope,scope_index)=>scope.pages.map(guide=>({path:guide.href.slice(prefix.length+1).replace(/\/$/,''),title:guide.title,scope_index,guide:guide.slug,fallback:scope.language==='es',title_anchor:'guide-title',body:`## Install {#stable-install}\n\n[Next](${scope.pages[1].href}#stable-install)\n\n<script>throw Error('historical')</script>\n\n\`\`\`js\nconsole.log('example');\n\`\`\``})));
  await Promise.all(Object.entries({pages,scopes,site:{title:'Fixture Docs',repository:'https://github.com/example/core',base_url:'https://example.github.io',prefix}}).map(([name,value])=>writeFile(join(app,`${name}.json`),JSON.stringify(value))));
  for(const scope of scopes){const image=join(app,'public','media',scope.version,'catalog-light.png');await mkdir(resolve(image,'..'),{recursive:true});await writeFile(image,Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aD1sAAAAASUVORK5CYII=','base64'));const raw=join(app,'public',scope.root.slice(prefix.length+1),'raw');await mkdir(raw,{recursive:true});await Promise.all(['index','quickstart'].map(name=>writeFile(join(raw,`${name}.md`),'# Immutable guide')));}
  const out=join(app,'output');
  const result=spawnSync('npm',['run','build'],{cwd:app,encoding:'utf8',env:{...process.env,ANDROID_REPO_DOCS_OUTPUT:out},maxBuffer:10*1024*1024});
  if(result.status!==0)throw Error(result.stdout+'\n'+result.stderr);
  const missing=await readFile(join(out,'404.html'),'utf8');assert.ok(!nodes(parse(missing)).some(n=>n.tagName==='link'&&['canonical','alternate'].includes(attr(n,'rel'))),'404 has no nonexistent canonical route');assert.ok(!nodes(parse(missing)).some(n=>/\/(?:en|es)\/404\/?$/.test(attr(n,'href')||'')),'404 has no nonexistent localized route links');await readFile(join(out,'favicon.svg'));
  const home=await readFile(join(out,'index.html'),'utf8');assert.ok(home.includes(`url=${prefix}/en/core/1.10.0/`),'home selects highest stable semver');
  const html=await readFile(join(out,'es/core/1.10.0/quickstart/index.html'),'utf8');const tree=nodes(parse(html));const search=tree.find(n=>n.tagName==='scope-search');assert.deepEqual(JSON.parse(attr(search,'data-filters')),{language:'es',module:'core',version:'1.10.0'});for(const filter of ['language:es','module:core','version:1.10.0'])assert.ok(tree.some(n=>attr(n,'data-pagefind-filter')===filter));const overview=nodes(parse(await readFile(join(out,'es/core/1.10.0/index.html'),'utf8')));assert.ok(overview.some(n=>n.tagName==='img'&&attr(n,'src')===`${prefix}/media/1.10.0/catalog-light.png`&&attr(n,'alt')==='Immutable catalog'),'overview immutable image');assert.ok(!tree.some(n=>n.tagName==='img'&&attr(n,'alt')==='Immutable catalog'),'guide omits overview screenshots');assert.ok(tree.some(n=>n.tagName==='html'&&attr(n,'lang')==='es'),'Spanish page language');const languageHome=await readFile(join(out,'es/index.html'),'utf8');assert.ok(languageHome.includes(`url=${prefix}/es/core/1.10.0/`),'Spanish home latest release');
  assert.ok(tree.some(n=>attr(n,'id')==='stable-install'),'explicit heading survives build');assert.ok(tree.some(n=>attr(n,'id')==='guide-title'),'title anchor survives build');
  assert.ok(html.includes('Traducción no disponible'),'fallback visible');assert.ok(!html.includes("throw Error('historical')"),'historical scripts omitted');
  assert.ok(tree.some(n=>n.tagName==='a'&&attr(n,'href')===`${prefix}/es/core/1.10.0/raw/quickstart.md`&&text(n)==='Markdown original'));
  const sidebar=tree.find(n=>n.tagName==='nav'&&attr(n,'aria-label')==='Guías del módulo');assert.equal(nodes(sidebar).filter(n=>n.tagName==='a').length,3,'sidebar only selected scope');
  const language=tree.find(n=>n.tagName==='select'&&attr(n,'aria-label')==='Idioma');assert.ok(nodes(language).some(n=>n.tagName==='option'&&attr(n,'value')===`${prefix}/en/core/1.10.0/quickstart/`));
  for(const page of pages){const doc=nodes(parse(await readFile(join(out,page.path,'index.html'),'utf8')));for(const n of doc.filter(n=>n.tagName==='a')){const href=attr(n,'href');if(!href?.startsWith(prefix+'/')||href.startsWith('//'))continue;const url=new URL(href,'https://fixture.invalid');const target=url.pathname.slice(prefix.length+1);if(!target||target.startsWith('_'))continue;await readFile(join(out,target,target.endsWith('.md')?'':'index.html'));}}
  await verifySearch(out,prefix);
  console.log(`Fixture build passed: ${prefix||'/'} (locale homes, scoped Pagefind WASM, screenshots, anchors, provenance, fallback, selectors, sidebar, links)`);
 }
}finally{if(process.env.ANDROID_REPO_DOCS_KEEP_FIXTURE)console.log(`Fixture retained: ${temp}`);else await rm(temp,{recursive:true,force:true});}
